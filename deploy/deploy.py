#!/usr/bin/env python3
"""Deploy a verified ARM64 peek-server build to production through S3 and SSM, with rollback.

Starter/Ting pattern (BLUEPRINT §5.3):
  1. check peek-server is a static Linux ARM64 ELF (and the optional Caddy binary is Linux ARM64);
  2. build a deterministic release archive: peek-server, [caddy], install.sh, render-env.py,
     backup.py, the systemd units, the Caddyfile and release.json (version, commit, checksums);
  3. aws s3 cp → s3://<ArtifactBucket>/releases/<sha256[:16]>.tar.gz (SSE AES256);
  4. aws ssm send-command AWS-RunShellScript: download, sha256sum -c, extract into
     /opt/peek/releases/<id>, run install.sh (render /etc/peek/runtime.env 0600 from Secrets
     Manager, flip /opt/peek/current, restart, health check, roll back on failure);
  5. poll the command, print its output, then check https://backend.peek.teamofsilicons.com/healthz.

[MUTATING]: steps 3-4 change production. --dry-run builds and describes the release without any
AWS call. Every AWS call passes --region (default us-east-1: the profile's default is us-west-1).
There is no SSH: admin access is `aws --region us-east-1 ssm start-session --target <instance>`.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import shlex
import struct
import subprocess
import sys
import tarfile
import tempfile
import time

try:
    import tomllib
except ModuleNotFoundError:  # Python < 3.11, e.g. the Command Line Tools' /usr/bin/python3 (3.9)
    sys.exit("deploy.py needs Python 3.11 or newer (tomllib); run it with python3.11+")
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
STACK = "silicon-peek-production"
PUBLIC_URL = "https://backend.peek.teamofsilicons.com"
HOST_FILES = (
    ("install.sh", 0o755),
    ("render-env.py", 0o644),
    ("backup.py", 0o644),
    ("peek-server.service", 0o644),
    ("caddy.service", 0o644),
    ("Caddyfile", 0o644),
    ("peek-backup.service", 0o644),
    ("peek-backup.timer", 0o644),
)


class DeployError(Exception):
    pass


def check_linux_arm64(path: Path, *, require_static: bool) -> None:
    """ELF64 little-endian, EM_AARCH64; with require_static, no PT_INTERP (musl static)."""
    if not path.is_file() or path.is_symlink():
        raise DeployError(f"{path} does not exist or is a symlink")
    data = path.read_bytes()
    if data[:6] != b"\x7fELF\x02\x01" or len(data) < 64:
        raise DeployError(f"{path} is not a little-endian ELF64 file")
    if struct.unpack_from("<H", data, 18)[0] != 183:
        raise DeployError(
            f"{path} is not an ARM64 (EM_AARCH64) binary; the host is a t4g Graviton: "
            "cargo zigbuild --release --locked --target aarch64-unknown-linux-musl -p silicon-peek"
        )
    if require_static:
        phoff = struct.unpack_from("<Q", data, 32)[0]
        phentsize, phnum = struct.unpack_from("<HH", data, 54)
        for index in range(phnum):
            offset = phoff + index * phentsize
            if offset + 4 <= len(data) and struct.unpack_from("<I", data, offset)[0] == 3:
                raise DeployError(
                    f"{path} is dynamically linked; build the aarch64-unknown-linux-musl target so it "
                    "does not depend on the host's glibc"
                )


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def package_version(root: Path) -> str:
    data = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    version = data.get("package", {}).get("version")
    if isinstance(version, dict):
        version = data.get("workspace", {}).get("package", {}).get("version")
    if not isinstance(version, str):
        raise DeployError(f"cannot read the silicon-peek version from {root / 'Cargo.toml'}")
    return version


def git_commit(root: Path) -> dict[str, Any]:
    try:
        commit = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "HEAD"], capture_output=True, text=True, check=True
        ).stdout.strip()
        dirty = bool(
            subprocess.run(
                ["git", "-C", str(root), "status", "--porcelain"], capture_output=True, text=True, check=True
            ).stdout.strip()
        )
    except (OSError, subprocess.CalledProcessError):
        return {"commit": None, "dirty": None}
    return {"commit": commit, "dirty": dirty}


def build_archive(
    binary: Path, caddy: Path | None, version: str, source: dict[str, Any], host_dir: Path = HERE
) -> tuple[bytes, dict[str, Any]]:
    """Deterministic tar.gz (sorted, mtime 0, root-owned): identical inputs give identical bytes."""
    files: list[tuple[str, bytes, int]] = [("peek-server", binary.read_bytes(), 0o755)]
    if caddy is not None:
        files.append(("caddy", caddy.read_bytes(), 0o755))
    for name, mode in HOST_FILES:
        path = host_dir / name
        if not path.is_file():
            raise DeployError(f"{path} is missing; the release carries every host file")
        files.append((name, path.read_bytes(), mode))
    manifest = {
        "service": "peek",
        "version": version,
        **source,
        "files": {name: sha256(data) for name, data, _ in sorted(files)},
    }
    files.append(("release.json", (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode(), 0o644))
    buffer = io.BytesIO()
    with (
        gzip.GzipFile(fileobj=buffer, mode="wb", mtime=0, compresslevel=9) as compressed,
        tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive,
    ):
        for name, data, mode in sorted(files):
            info = tarfile.TarInfo(name)
            info.size, info.mode, info.mtime = len(data), mode, 0
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            archive.addfile(info, io.BytesIO(data))
    return buffer.getvalue(), manifest


def remote_script(uri: str, checksum: str, release: str, region: str, bucket: str, require_tls: bool) -> str:
    remote = f"/opt/peek/releases/{release}"
    q = shlex.quote
    lines = [
        "set -eu",
        "umask 022",
        "mkdir -p /opt/peek/releases",
        f"if [ ! -f {remote}/release.json ]; then",
        f"  rm -rf {remote}.tmp {remote}.tgz && mkdir -p {remote}.tmp",
        f"  aws s3 cp {q(uri)} {remote}.tgz --region {q(region)} --only-show-errors",
        f"  echo '{checksum}  {remote}.tgz' | sha256sum -c -",
        f"  tar -xzf {remote}.tgz -C {remote}.tmp --no-same-owner",
        f"  rm -f {remote}.tgz && mv {remote}.tmp {remote}",
        "fi",
        f"bash {remote}/install.sh {remote} {q(region)} {q(bucket)}" + (" --require-tls" if require_tls else ""),
    ]
    return "\n".join(lines)


class Aws:
    def __init__(self, region: str, profile: str | None) -> None:
        self.base = ["aws", "--region", region, "--output", "json"]
        if profile:
            self.base += ["--profile", profile]

    def __call__(self, *parts: str) -> Any:
        command = [*self.base, *parts]
        try:
            result = subprocess.run(command, capture_output=True, text=True, check=False)
        except FileNotFoundError:
            raise DeployError("the aws CLI is not on PATH; install AWS CLI v2") from None
        if result.returncode != 0:
            raise DeployError(f"aws {' '.join(parts[:2])} failed (exit {result.returncode}): {result.stderr.strip()}")
        return json.loads(result.stdout) if result.stdout.strip() else None


def public_health(url: str) -> dict[str, Any]:
    report: dict[str, Any] = {}
    for route in ("healthz", "readyz"):
        try:
            with urllib.request.urlopen(f"{url}/{route}", timeout=10) as response:  # noqa: S310 (fixed https URL)
                report[route] = {"status": response.status, "body": json.loads(response.read() or b"null")}
        except urllib.error.HTTPError as error:
            report[route] = {"status": error.code, "body": error.read().decode("utf-8", "replace")[:500]}
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            report[route] = {"error": str(error)}
    return report


def rollback_report(command_id: str, status: str, host_stderr: str) -> str:
    """What a failed install means, from install.sh's own rollback lines (never assumed)."""
    if "peek install: ROLLBACK INCOMPLETE" in host_stderr:
        return (
            f"SSM command {command_id} ended {status} and the host's rollback is INCOMPLETE: production may be "
            "down; recover by hand (see the output above)"
        )
    if "peek install: restored " in host_stderr:
        return f"SSM command {command_id} ended {status}; the host restored the previous release (see output above)"
    if "peek install: no previous release existed" in host_stderr:
        return f"SSM command {command_id} ended {status}; there was no previous release, peek-server stays stopped"
    return (
        f"SSM command {command_id} ended {status} and the host did not confirm a rollback; check the host before "
        "retrying (see the output above)"
    )


def main(argv: list[str] | None = None) -> int:
    target_dir = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--binary", type=Path, default=target_dir / "aarch64-unknown-linux-musl/release/peek-server")
    parser.add_argument("--caddy", type=Path, help="Linux ARM64 Caddy binary (required on the first deploy)")
    parser.add_argument("--region", default="us-east-1")
    parser.add_argument("--profile", default="default")
    parser.add_argument("--stack-name", default=STACK)
    parser.add_argument("--public-url", default=PUBLIC_URL)
    parser.add_argument(
        "--require-tls", action="store_true", help="fail and roll back unless HTTPS works through Caddy (after DNS, P4)"
    )
    parser.add_argument("--timeout", type=int, default=600, help="seconds to wait for the SSM command")
    parser.add_argument("--dry-run", action="store_true", help="build and describe the release; call no AWS API")
    parser.add_argument("--archive-out", type=Path, help="with --dry-run: also write the archive here")
    parser.add_argument("--yes", action="store_true", help="do not ask for confirmation")
    args = parser.parse_args(argv)
    try:
        if args.region != "us-east-1":
            print(f"deploy: warning: --region {args.region}; production lives in us-east-1", file=sys.stderr)
        check_linux_arm64(args.binary, require_static=True)
        if args.caddy is not None:
            check_linux_arm64(args.caddy, require_static=False)
        version = package_version(ROOT)
        archive, manifest = build_archive(args.binary, args.caddy, version, git_commit(ROOT))
        checksum = sha256(archive)
        release = checksum[:16]
        plan = {
            "release": release,
            "sha256": checksum,
            "version": version,
            "commit": manifest["commit"],
            "dirty": manifest["dirty"],
            "files": sorted(manifest["files"]),
            "bytes": len(archive),
        }
        if manifest["dirty"]:
            print(
                "deploy: warning: the working tree has uncommitted changes; release.json records dirty=true",
                file=sys.stderr,
            )
        if args.dry_run:
            if args.archive_out:
                args.archive_out.write_bytes(archive)
                plan["archive"] = str(args.archive_out)
            plan["remote_script"] = remote_script(
                f"s3://<ArtifactBucket>/releases/{release}.tar.gz",
                checksum,
                release,
                args.region,
                "<ArtifactBucket>",
                args.require_tls,
            )
            print(json.dumps(plan, indent=2))
            return 0
        aws = Aws(args.region, args.profile)
        stack = aws("cloudformation", "describe-stacks", "--stack-name", args.stack_name)["Stacks"][0]
        outputs = {item["OutputKey"]: item["OutputValue"] for item in stack.get("Outputs", [])}
        for key in ("InstanceId", "ArtifactBucket"):
            if key not in outputs:
                raise DeployError(f"stack {args.stack_name} has no {key} output; deploy deploy/stack.yaml first (P3)")
        instance, bucket = outputs["InstanceId"], outputs["ArtifactBucket"]
        if not args.yes:
            if not sys.stdin.isatty():
                raise DeployError("refusing to deploy without confirmation; pass --yes (this changes production)")
            answer = input(
                f"[MUTATING] deploy peek-server {version} ({release}) to {instance} in {args.region}? [y/N] "
            )
            if answer.strip().lower() not in ("y", "yes"):
                print("deploy: cancelled", file=sys.stderr)
                return 1
        uri = f"s3://{bucket}/releases/{release}.tar.gz"
        with tempfile.TemporaryDirectory(prefix="peek-deploy-") as scratch:
            path = Path(scratch) / f"{release}.tar.gz"
            path.write_bytes(archive)
            aws("s3", "cp", str(path), uri, "--sse", "AES256", "--only-show-errors")
            request = Path(scratch) / "send-command.json"
            request.write_text(
                json.dumps(
                    {
                        "DocumentName": "AWS-RunShellScript",
                        "InstanceIds": [instance],
                        "Comment": f"Silicon Peek release {version} {release}",
                        "TimeoutSeconds": 600,
                        "Parameters": {
                            "commands": [remote_script(uri, checksum, release, args.region, bucket, args.require_tls)],
                            "executionTimeout": [str(args.timeout)],
                        },
                    }
                )
            )
            command_id = aws("ssm", "send-command", "--cli-input-json", f"file://{request}")["Command"]["CommandId"]
        print(json.dumps({**plan, "instance": instance, "command_id": command_id}), flush=True)
        deadline = time.monotonic() + args.timeout + 60
        status = "Pending"
        result: dict[str, Any] = {}
        while time.monotonic() < deadline:
            time.sleep(3)
            try:
                result = aws("ssm", "get-command-invocation", "--command-id", command_id, "--instance-id", instance)
            except DeployError as error:
                if "InvocationDoesNotExist" in str(error):
                    continue
                raise
            status = result["Status"]
            if status not in ("Pending", "InProgress", "Delayed"):
                break
        print(result.get("StandardOutputContent", ""), end="")
        print(result.get("StandardErrorContent", ""), end="", file=sys.stderr)
        if status in ("Pending", "InProgress", "Delayed"):
            raise DeployError(
                f"SSM command {command_id} is still {status}; inspect it before retrying: aws --region {args.region} ssm get-command-invocation --command-id {command_id} --instance-id {instance}"
            )
        if status != "Success":
            raise DeployError(rollback_report(command_id, status, result.get("StandardErrorContent", "")))
        health = public_health(args.public_url)
        print(json.dumps({"public": health}, indent=2))
        healthz = health.get("healthz", {})
        if healthz.get("status") != 200 or (healthz.get("body") or {}).get("version") != version:
            message = f"{args.public_url}/healthz does not report version {version} yet"
            if args.require_tls:
                raise DeployError(message)
            print(f"deploy: warning: {message} (DNS or TLS may still be pending; BLUEPRINT §5.4)", file=sys.stderr)
    except DeployError as error:
        print(f"deploy: error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
