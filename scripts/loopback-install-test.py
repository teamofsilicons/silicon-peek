#!/usr/bin/env python3
"""Install a packed peek archive exactly as Stemcell would, against a loopback fake Honeycomb API.

BLUEPRINT §4.4 step 6 / notes/gap-honeycomb-app-distribution §9. Everything happens in a scratch
directory: a fake API on 127.0.0.1 serves only this archive, `honeycomb install 'peek' --json`
runs with PATH removed, stdin at /dev/null, a scratch SILICON_HOME and HOME, and updates,
services, telemetry and PATH edits off. Then:

  1. the install JSON must say status "installed" for this version; on macOS it must also say
     install_script.status "succeeded" (Manifest A: Honeycomb runs install-app.sh itself);
  2. the installed macOS package must hold bin/peek (0755), Peek.app.zip (same bytes),
     Peek.app.info, install-app.sh (0755) and LICENSE;
  3. `peek iam --json` (PATH removed) must print {"app_id":"peek", "version":<version>, ...};
  4. on macOS, the install-app.sh Honeycomb ran (with PEEK_INSTALL_APPLICATIONS_DIR and
     PEEK_INSTALL_SUPPORT_DIR inside the scratch directory and PEEK_INSTALL_NO_LAUNCH=1, passed
     through Honeycomb's environment) must have recorded "[ok] install" and "[skip] launch" in
     install-status.txt and left a Peek.app that passes codesign --verify --deep --strict;
  5. on macOS, the installed CLI's own `peek app install --json` (ensure_app(), the fallback path)
     runs against a second pair of scratch directories with PEEK_INSTALL_NO_LAUNCH=1 and a scratch
     PEEK_DAEMON_SOCKET: it must install and verify the bundled Peek.app, then stop at the launch
     step with peek_service_unavailable (exit 5, launching is forbidden by design); `peek app status
     --json` must then report the installed build, the Peek.app.info bundle id and signature "valid".

Nothing outside the scratch directory is written, nothing is contacted beyond 127.0.0.1, and the
real ~/Applications is never touched. Exit 0 = every check passed.
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.parse
from pathlib import Path
from typing import Any


class LoopbackError(Exception):
    pass


def host_target() -> str | None:
    system = {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}.get(platform.system())
    machine = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "AMD64": "x86_64"}.get(platform.machine())
    return f"{system}-{machine}" if system and machine else None


def archive_identity(archive: Path) -> tuple[str, str]:
    with tarfile.open(archive, "r:gz") as package:
        member = package.getmember("honeycomb.yaml")
        handle = package.extractfile(member)
        if handle is None:
            raise LoopbackError(f"{archive} has no readable honeycomb.yaml")
        manifest = handle.read().decode("utf-8")
    fields = {}
    for key in ("app_id", "version"):
        match = re.search(rf"^{key}:\s*[\"']?([^\s\"'#]+)", manifest, re.MULTILINE)
        if not match:
            raise LoopbackError(f"{archive}: honeycomb.yaml has no top-level {key}")
        fields[key] = match.group(1)
    return fields["app_id"], fields["version"]


class FakeHoneycomb(http.server.ThreadingHTTPServer):
    """Serves GET /api/v2/apps/<id>/releases and /download for one immutable archive."""

    def __init__(self, archive: Path, app_id: str, version: str) -> None:
        self.payload = archive.read_bytes()
        self.app_id, self.version = app_id, version
        self.requests: list[str] = []
        self.unexpected: list[str] = []
        super().__init__(("127.0.0.1", 0), _Handler)

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.server_address[1]}"

    def release(self) -> dict[str, Any]:
        return {
            "app_id": self.app_id,
            "version": self.version,
            "channel": "prod",
            "visibility": "public",
            "sha256": hashlib.sha256(self.payload).hexdigest(),
            "size": len(self.payload),
            "created_at": int(time.time()),
            "configuration_revision": 1,
            "permission_approval_required": False,
        }


class _Handler(http.server.BaseHTTPRequestHandler):
    server: FakeHoneycomb

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002 (stdlib signature)
        return

    def _send(self, status: int, body: bytes, content_type: str) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802 (stdlib naming)
        parsed = urllib.parse.urlsplit(self.path)
        query = urllib.parse.parse_qs(parsed.query)
        self.server.requests.append(self.path)
        base = f"/api/v2/apps/{self.server.app_id}"
        if parsed.path == f"{base}/releases" and query.get("channel") == ["prod"]:
            body = {"items": [self.server.release()], "page": 1, "per_page": 50, "total": 1}
            self._send(200, json.dumps(body).encode(), "application/json")
        elif parsed.path == f"{base}/download" and query.get("version") == [self.server.version]:
            self._send(200, self.server.payload, "application/gzip")
        else:
            self.server.unexpected.append(self.path)
            error = {
                "error": {
                    "code": "not_found",
                    "message": "the loopback fake API serves only the peek release under test",
                }
            }
            self._send(404, json.dumps(error).encode(), "application/json")

    def do_POST(self) -> None:  # noqa: N802
        self.server.unexpected.append(f"POST {self.path}")
        self._send(405, b'{"error":{"code":"method_not_allowed","message":"read-only fake API"}}', "application/json")


def run(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            command,
            env=env,
            cwd=cwd,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise LoopbackError(
            f"{Path(command[0]).name} {' '.join(command[1:])} did not finish within {timeout}s; Stemcell has no "
            "timeout, so this would hang silicon connect"
        ) from error


def loopback(args: argparse.Namespace, scratch: Path) -> dict[str, Any]:
    archive = args.archive.resolve()
    if not archive.is_file():
        raise LoopbackError(f"{archive} does not exist; pack it with scripts/build-release.sh")
    honeycomb = shutil.which(args.honeycomb)
    if honeycomb is None:
        raise LoopbackError(f"Honeycomb CLI {args.honeycomb!r} not found; install Honeycomb >= 0.5.0")
    honeycomb = str(Path(honeycomb).resolve())
    app_id, version = archive_identity(archive)
    if app_id != "peek":
        raise LoopbackError(f"{archive} is app {app_id!r}, not peek")
    target = host_target()

    server = FakeHoneycomb(archive, app_id, version)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        silicon_home = scratch / "si-home"
        packages = silicon_home / ".silicon" / "packages"
        (packages / ".honeycomb" / "dir").mkdir(parents=True)
        for directory in (silicon_home / ".silicon", packages):
            directory.chmod(0o700)
        config = {"api": server.url, "auto_update": False, "telemetry": False}
        (packages / ".honeycomb" / "dir" / "config.json").write_text(json.dumps(config))
        (scratch / "home").mkdir()
        applications, support = scratch / "Applications", scratch / "Support"
        # Stemcell's environment, minus PATH (silicon-stemcell apps.rs); everything rooted in scratch.
        # Honeycomb passes its environment to install-app.sh (Manifest A), so the script's test hooks
        # keep it inside scratch: it resolves the account's real home with dscl otherwise.
        env = {
            "HOME": str(scratch / "home"),
            "TMPDIR": str(scratch),
            "SILICON_HOME": str(packages),
            "SILICON_IAM_HOME": str(packages / ".silicon-iam"),
            "HONEYCOMB_API_URL": server.url,
            "HONEYCOMB_NO_MODIFY_PATH": "1",
            "HONEYCOMB_NO_SERVICE": "1",
            "HONEYCOMB_TELEMETRY": "0",
            "HONEYCOMB_AUTO_UPDATE": "0",
            "SPACE_STATION_TELEMETRY": "0",
            "PEEK_TELEMETRY": "0",
            "PEEK_INSTALL_APPLICATIONS_DIR": str(applications),
            "PEEK_INSTALL_SUPPORT_DIR": str(support),
            "PEEK_INSTALL_NO_LAUNCH": "1",
        }
        installed = run([honeycomb, "install", app_id, "--json"], env=env, cwd=packages, timeout=args.timeout)
        if installed.returncode != 0:
            raise LoopbackError(
                f"honeycomb install exited {installed.returncode}; Stemcell would abort silicon connect.\n"
                f"stdout: {installed.stdout.strip()}\nstderr: {installed.stderr.strip()}"
            )
        try:
            result = json.loads(installed.stdout)
        except json.JSONDecodeError as error:
            raise LoopbackError(f"honeycomb install --json printed non-JSON: {installed.stdout!r}") from error
        if result.get("status") != "installed" or result.get("version") != version:
            raise LoopbackError(f"honeycomb install reported {result}; expected status installed, version {version}")
        on_mac = bool(target and target.startswith("macos-"))
        if on_mac:
            script_result = result.get("install_script")
            if not isinstance(script_result, dict) or script_result.get("status") != "succeeded":
                raise LoopbackError(
                    f"honeycomb install reported install_script {script_result!r}; Manifest A must run "
                    "install-app.sh and it must succeed (status \"succeeded\")"
                )
        elif "install_script" in result:
            raise LoopbackError(f"{target} has no install_script in Manifest A, but Honeycomb ran {result['install_script']}")
        help_command = str(result.get("help_command", ""))
        launcher = Path(help_command.removesuffix(" --help"))
        if not help_command.endswith(" --help") or not launcher.is_file():
            raise LoopbackError(f"cannot find the installed peek launcher from help_command {help_command!r}")

        # Honeycomb 0.5.0 keeps packages under .honeycomb/dir/contexts/<fingerprint>/packages/<app>/<version>-<sha12>/.
        infos = sorted((packages / ".honeycomb" / "dir").glob(f"**/packages/{app_id}/*/Peek.app.info"))
        checks: dict[str, Any] = {"install": "installed", "unexpected_requests": server.unexpected}
        if target and target.startswith("macos-"):
            if len(infos) != 1:
                raise LoopbackError(f"expected one installed Peek.app.info, found {[str(p) for p in infos]}")
            package_dir = infos[0].parent
            for name, executable in (
                ("bin/peek", True),
                ("Peek.app.zip", False),
                ("Peek.app.info", False),
                ("install-app.sh", True),
                ("LICENSE", False),
            ):
                path = package_dir / name
                if not path.is_file() or path.is_symlink():
                    raise LoopbackError(f"installed package lacks {name} ({path})")
                if executable and not os.access(path, os.X_OK):
                    raise LoopbackError(f"installed {name} lost its executable bit")
            with tarfile.open(archive, "r:gz") as package:
                handle = package.extractfile(f"targets/{target}/Peek.app.zip")
                packed = hashlib.sha256(handle.read()).hexdigest() if handle else ""
            installed_zip = hashlib.sha256((package_dir / "Peek.app.zip").read_bytes()).hexdigest()
            if packed != installed_zip:
                raise LoopbackError("Honeycomb changed the Peek.app.zip bytes during install")
            checks["package_dir"] = str(package_dir)
            checks["zip_sha256"] = installed_zip

        iam_env = {
            "HOME": str(scratch / "home"),
            "SILICON_HOME": str(scratch / "peek-home"),
            "PEEK_TELEMETRY": "0",
            "TMPDIR": str(scratch),
        }
        (scratch / "peek-home").mkdir()
        iam = run([str(launcher), "iam", "--json"], env=iam_env, cwd=scratch, timeout=30)
        if iam.returncode != 0:
            raise LoopbackError(f"peek iam --json exited {iam.returncode}: {iam.stderr.strip()}")
        try:
            identity = json.loads(iam.stdout)
        except json.JSONDecodeError as error:
            raise LoopbackError(f"peek iam --json printed non-JSON: {iam.stdout!r}") from error
        if identity.get("app_id") != "peek" or identity.get("version") != version:
            raise LoopbackError(f"peek iam --json returned {identity}; expected app_id peek and version {version}")
        checks["peek_iam"] = {"app_id": identity["app_id"], "version": identity["version"]}

        if args.skip_app_install or not on_mac:
            checks["install_app_sh"] = "skipped" if not on_mac else result.get("install_script")
        else:
            status_path = support / "install-status.txt"
            status_file = status_path.read_text(encoding="utf-8") if status_path.is_file() else ""
            lines = [l for l in status_file.splitlines() if not l.startswith("run\t")]
            if not any(l.startswith("ok\tinstall\t") for l in lines):
                raise LoopbackError(f"the install-app.sh Honeycomb ran did not install Peek.app:\n{status_file}")
            if "skip\tlaunch\tPEEK_INSTALL_NO_LAUNCH=1" not in lines:
                raise LoopbackError(f"install-app.sh did not honour PEEK_INSTALL_NO_LAUNCH=1:\n{status_file}")
            verify = run(
                ["/usr/bin/codesign", "--verify", "--deep", "--strict", str(applications / "Peek.app")],
                env={"PATH": "/usr/bin:/bin"},
                cwd=scratch,
                timeout=60,
            )
            if verify.returncode != 0:
                raise LoopbackError(f"the installed Peek.app fails codesign --verify: {verify.stderr.strip()}")
            checks["install_app_sh"] = {"install_script": result.get("install_script"), "status": lines}

        if args.skip_app_install or not on_mac:
            checks["peek_app_install"] = "skipped"
        else:
            checks["peek_app_install"] = cli_app_install(launcher, Path(checks["package_dir"]), version, scratch)
        return {"archive": str(archive), "version": version, "host_target": target, "checks": checks, "passed": True}
    finally:
        server.shutdown()
        server.server_close()


def read_info(path: Path) -> dict[str, str]:
    fields = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        key, _, value = line.partition("=")
        fields[key.strip()] = value.strip()
    return fields


def cli_app_install(launcher: Path, package_dir: Path, version: str, scratch: Path) -> dict[str, Any]:
    """`peek app install --json` from the installed package, entirely inside scratch (never ~/Applications)."""
    info = read_info(package_dir / "Peek.app.info")
    applications, support = scratch / "cli-Applications", scratch / "cli-Support"
    socket = scratch / "d.sock"  # never the real peekd's socket; sun_path allows 104 bytes
    if len(str(socket).encode()) >= 104:
        raise LoopbackError(f"scratch path {scratch} is too long for a Unix socket path; set a shorter TMPDIR")
    env = {
        "HOME": str(scratch / "home"),
        "TMPDIR": str(scratch),
        "PATH": "/usr/bin:/bin",
        "SILICON_HOME": str(scratch / "peek-home"),
        "PEEK_TELEMETRY": "0",
        "PEEK_DAEMON_SOCKET": str(socket),
        "PEEK_INSTALL_APPLICATIONS_DIR": str(applications),
        "PEEK_INSTALL_SUPPORT_DIR": str(support),
        "PEEK_INSTALL_NO_LAUNCH": "1",
    }
    installed = run([str(launcher), "app", "install", "--json"], env=env, cwd=scratch, timeout=200)
    stderr = installed.stderr.strip()
    try:
        error = json.loads(stderr.splitlines()[-1]).get("error", {}) if stderr else {}
    except (json.JSONDecodeError, AttributeError):
        error = {}
    # Installing succeeds; starting Peek.app is forbidden here, so the command must stop exactly there.
    stopped_at_launch = error.get("code") == "peek_service_unavailable" and "PEEK_INSTALL_NO_LAUNCH" in stderr
    if installed.returncode != 5 or not stopped_at_launch:
        raise LoopbackError(
            f"peek app install --json exited {installed.returncode}; expected exit 5 peek_service_unavailable at the "
            f"launch step (PEEK_INSTALL_NO_LAUNCH=1)\nstdout: {installed.stdout.strip()}\nstderr: {stderr}"
        )
    app = applications / "Peek.app"
    if not (app / "Contents/Info.plist").is_file():
        raise LoopbackError(f"peek app install did not install {app}\n{stderr}")
    leftovers = [p.name for p in applications.iterdir() if p.name != "Peek.app"]
    if leftovers:
        raise LoopbackError(f"peek app install left staging entries in {applications}: {leftovers}")
    status_path = support / "install-status.txt"
    status_file = status_path.read_text(encoding="utf-8") if status_path.is_file() else ""
    expected_line = f"ok\tinstall\tinstalled Peek.app {version} (build {info.get('bundle_version')})"
    if expected_line not in status_file:
        raise LoopbackError(f"install-status.txt lacks {expected_line!r}:\n{status_file}")
    verify = run(
        ["/usr/bin/codesign", "--verify", "--deep", "--strict", "--verbose=2", str(app)],
        env={"PATH": "/usr/bin:/bin"},
        cwd=scratch,
        timeout=60,
    )
    if verify.returncode != 0:
        raise LoopbackError(f"the Peek.app installed by peek app install fails codesign: {verify.stderr.strip()}")
    status = run([str(launcher), "app", "status", "--json"], env=env, cwd=scratch, timeout=60)
    try:
        report = json.loads(status.stdout)
    except json.JSONDecodeError as error:
        raise LoopbackError(
            f"peek app status --json printed non-JSON (exit {status.returncode}): {status.stdout!r}"
        ) from error
    wanted = {
        "installed": True,
        "build": int(info.get("bundle_version", "0")),
        "bundle_id": info.get("bundle_id"),
        "signature": "valid",
    }
    got = {key: report.get(key) for key in wanted}
    if status.returncode != 0 or got != wanted:
        raise LoopbackError(f"peek app status --json (exit {status.returncode}) reported {got}; expected {wanted}")
    if (report.get("bundled") or {}).get("build") != wanted["build"]:
        raise LoopbackError(f"peek app status --json does not see the bundled build: {report.get('bundled')}")
    return {
        "exit": installed.returncode,
        "stopped_at": error.get("code"),
        "installed": str(app),
        "codesign_verify": "valid",
        "app_status": {**got, "short_version": report.get("short_version")},
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--archive", type=Path, required=True, help="dist/peek-<version>.tar.gz")
    parser.add_argument(
        "--honeycomb", default=os.environ.get("HONEYCOMB", "honeycomb"), help="Honeycomb CLI to test with"
    )
    parser.add_argument("--timeout", type=int, default=180, help="seconds allowed for honeycomb install (default 180)")
    parser.add_argument(
        "--skip-app-install",
        action="store_true",
        help="skip checking install-app.sh's Peek.app and `peek app install` (Honeycomb still runs the script, in scratch)",
    )
    parser.add_argument("--keep", action="store_true", help="keep the scratch directory for inspection")
    args = parser.parse_args(argv)
    scratch = Path(tempfile.mkdtemp(prefix="peek-loopback-")).resolve()
    try:
        summary = loopback(args, scratch)
    except (LoopbackError, OSError, tarfile.TarError) as error:
        print(f"loopback-install-test: error: {error}", file=sys.stderr)
        if args.keep:
            print(f"loopback-install-test: scratch kept at {scratch}", file=sys.stderr)
        return 1
    finally:
        if not args.keep:
            shutil.rmtree(scratch, ignore_errors=True)
    if args.keep:
        summary["scratch"] = str(scratch)
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
