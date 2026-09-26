"""Tests for the deploy tooling: render-env.py, backup.py, deploy.py, install.sh and stack.yaml.

Run: python3 -m unittest discover -s deploy/tests
Everything is local: no AWS call is made (deploy.py runs only with --dry-run).
"""

from __future__ import annotations

import importlib.util
import io
import json
import os
import shutil
import sqlite3
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from contextlib import closing, redirect_stderr, redirect_stdout
from pathlib import Path

DEPLOY = Path(__file__).resolve().parents[1]
REPO = DEPLOY.parent


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


render_env = load("render_env", DEPLOY / "render-env.py")
backup = load("peek_backup", DEPLOY / "backup.py")
deploy = load("peek_deploy", DEPLOY / "deploy.py")


def call(module, argv: list[str], stdin: str = "") -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    old = sys.stdin
    sys.stdin = io.StringIO(stdin)
    try:
        with redirect_stdout(out), redirect_stderr(err):
            code = module.main(argv)
    finally:
        sys.stdin = old
    return code, out.getvalue(), err.getvalue()


def filled_runtime() -> dict[str, str]:
    document = json.loads((DEPLOY / "runtime.json.example").read_text())
    document.update(
        PEEK_IAM_WEBHOOK_SECRET="a" * 64,
        PEEK_ENCRYPTION_KEY="b" * 64,
        PEEK_HONEYCOMB_SERVICE_TOKEN="c" * 40,
    )
    return document


def elf(machine: int, interpreter: bool = False) -> bytes:
    ident = b"\x7fELF" + bytes([2, 1, 1, 0]) + bytes(8)
    header = ident + struct.pack("<HHIQQQIHHHHHH", 2, machine, 1, 0, 64, 0, 0, 64, 56, 1, 0, 0, 0)
    return header + struct.pack("<IIQQQQQQ", 3 if interpreter else 1, 5, 0, 0, 0, 0, 0, 0) + bytes(32)


class RenderEnvTests(unittest.TestCase):
    def test_example_lists_every_key_and_needs_generated_secrets(self) -> None:
        example = json.loads((DEPLOY / "runtime.json.example").read_text())
        self.assertEqual(set(example), set(render_env.SPEC))
        for key, (_, secret, _) in render_env.SPEC.items():
            if secret:
                self.assertEqual(example[key], "", f"{key} must be empty in the committed example")
        code, _, err = call(render_env, ["--check"], json.dumps(example))
        self.assertEqual(code, 1)
        for key in ("PEEK_IAM_WEBHOOK_SECRET", "PEEK_ENCRYPTION_KEY", "PEEK_HONEYCOMB_SERVICE_TOKEN"):
            self.assertIn(f"{key}: must not be empty", err)

    def test_filled_runtime_renders_0600_without_printing_values(self) -> None:
        document = filled_runtime()
        document["PEEK_GITHUB_REPO"] = "teamofsilicons/silicon-peek"
        with tempfile.TemporaryDirectory() as scratch:
            output = Path(scratch) / "etc" / "runtime.env"
            code, out, err = call(render_env, ["--output", str(output)], json.dumps(document))
            self.assertEqual(code, 0, err)
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            text = output.read_text()
            self.assertIn('PEEK_BIND="127.0.0.1:8080"\n', text)
            self.assertIn(f'PEEK_ENCRYPTION_KEY="{"b" * 64}"\n', text)
            self.assertNotIn("b" * 64, out + err)
            self.assertIn("PEEK_IAM_APP_SECRET", err)  # reported as not configured yet, by name only

    def test_quoting_escapes_backslash_and_quote(self) -> None:
        self.assertEqual(render_env.quote('a"b\\c'), '"a\\"b\\\\c"')

    def test_rejections_are_specific(self) -> None:
        cases = {
            "PEEK_BIND": ("0.0.0.0:8080", "127.0.0.1:8080"),
            "PEEK_DATABASE_PATH": ("/tmp/peek.sqlite", "/var/lib/peek/"),
            "PEEK_PUBLIC_ORIGIN": ("http://backend.peek.teamofsilicons.com", "https://"),
            "PEEK_IAM_APP_SECRET": ("secret", "ask_"),
            "PEEK_ENCRYPTION_KEY": ("XYZ", "64 lowercase hex"),
            "PEEK_BACKEND_TABLE_KEY": ("table-other-" + "0" * 32, "table-peekbackend-"),
            "PEEK_IAM_REQUEST_TIMEOUT_SECONDS": ("0", "integer from 1"),
        }
        for key, (value, hint) in cases.items():
            document = filled_runtime()
            document[key] = value
            code, _, err = call(render_env, ["--check"], json.dumps(document))
            self.assertEqual(code, 1, key)
            self.assertIn(key, err)
            self.assertIn(hint, err)

    def test_byo_deepgram_hosts_is_optional_and_checked_like_the_server(self) -> None:
        # A secret written before the key existed still renders (the server defaults to *.deepgram.com).
        document = filled_runtime()
        del document["PEEK_BYO_DEEPGRAM_HOSTS"]
        code, _, err = call(render_env, ["--check"], json.dumps(document))
        self.assertEqual(code, 0, err)
        for good in ("", "*.deepgram.com", "dg.acme.example, *.deepgram.com", "*", "API.EU.Deepgram.com"):
            document = filled_runtime()
            document["PEEK_BYO_DEEPGRAM_HOSTS"] = good
            code, _, err = call(render_env, ["--check"], json.dumps(document))
            self.assertEqual(code, 0, f"{good!r}: {err}")
        for bad in ("127.0.0.1", "localhost", "dg.localhost", "*.", "*.com", "http://x.example", ",", "a..b"):
            document = filled_runtime()
            document["PEEK_BYO_DEEPGRAM_HOSTS"] = bad
            code, _, err = call(render_env, ["--check"], json.dumps(document))
            self.assertEqual(code, 1, bad)
            self.assertIn("PEEK_BYO_DEEPGRAM_HOSTS", err)

    def test_structure_errors(self) -> None:
        document = filled_runtime()
        document["AWS_SECRET_ACCESS_KEY"] = "x"
        self.assertIn("unknown key", call(render_env, ["--check"], json.dumps(document))[2])
        document = filled_runtime()
        document["PEEK_IAM_WEBHOOK_KEY_VERSION"] = 1
        self.assertIn("JSON string", call(render_env, ["--check"], json.dumps(document))[2])
        document = filled_runtime()
        document["RUST_LOG"] = "info\nPEEK_BIND=0.0.0.0:80"
        self.assertIn("newline", call(render_env, ["--check"], json.dumps(document))[2])
        del document["RUST_LOG"]
        self.assertIn("RUST_LOG: missing", call(render_env, ["--check"], json.dumps(document))[2])
        self.assertIn("duplicate key", call(render_env, ["--check"], '{"PEEK_BIND":"a","PEEK_BIND":"b"}')[2])
        self.assertIn("not JSON", call(render_env, ["--check"], "{")[2])

    def test_unknown_peek_key_warns(self) -> None:
        document = filled_runtime()
        document["PEEK_FUTURE_FLAG"] = "1"
        code, _, err = call(render_env, ["--check"], json.dumps(document))
        self.assertEqual(code, 0, err)
        self.assertIn("PEEK_FUTURE_FLAG: not known", err)


class BackupTests(unittest.TestCase):
    def test_wal_databases_snapshot_consistently(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            data = Path(scratch) / "data"
            data.mkdir()
            writer = sqlite3.connect(data / "peek.sqlite")
            writer.execute("PRAGMA journal_mode=WAL")
            writer.execute("CREATE TABLE drawings (ctx TEXT, sha256 TEXT)")
            writer.executemany("INSERT INTO drawings VALUES (?, ?)", [("production", str(i)) for i in range(500)])
            writer.commit()  # the writer stays open, as peek-server does
            with closing(sqlite3.connect(data / "testing.sqlite")) as other:
                other.execute("CREATE TABLE t (x)")
            out_dir = Path(scratch) / "out"
            code, out, err = call(backup, ["--data-dir", str(data), "--local-dir", str(out_dir)])
            writer.close()
            self.assertEqual(code, 0, err)
            report = json.loads(out)
            self.assertEqual([item["database"] for item in report["backed_up"]], ["peek.sqlite", "testing.sqlite"])
            snapshot = Path(report["backed_up"][0]["location"])
            self.assertEqual(snapshot.stat().st_mode & 0o777, 0o600)
            with closing(sqlite3.connect(snapshot)) as copy:
                self.assertEqual(copy.execute("SELECT count(*) FROM drawings").fetchone(), (500,))

    def test_nothing_to_back_up_and_missing_bucket(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            code, out, _ = call(backup, ["--data-dir", scratch, "--local-dir", scratch])
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(out)["backed_up"], [])
            env = os.environ.pop("PEEK_BACKUP_BUCKET", None)
            try:
                code, _, err = call(backup, ["--data-dir", scratch, "--bucket", ""])
            finally:
                if env is not None:
                    os.environ["PEEK_BACKUP_BUCKET"] = env
            self.assertEqual(code, 1)
            self.assertIn("PEEK_BACKUP_BUCKET", err)

    def test_corrupt_database_is_not_uploaded(self) -> None:
        with tempfile.TemporaryDirectory() as scratch:
            (Path(scratch) / "peek.sqlite").write_bytes(b"not a database" * 100)
            code, _, err = call(backup, ["--data-dir", scratch, "--local-dir", str(Path(scratch) / "out")])
            self.assertEqual(code, 1)
            self.assertIn("cannot snapshot", err)
            self.assertFalse((Path(scratch) / "out").exists())


class DeployTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="peek-deploy-test-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.binary = self.tmp / "peek-server"
        self.binary.write_bytes(elf(183))

    def test_elf_checks(self) -> None:
        deploy.check_linux_arm64(self.binary, require_static=True)
        (self.tmp / "x86").write_bytes(elf(62))
        with self.assertRaises(deploy.DeployError) as caught:
            deploy.check_linux_arm64(self.tmp / "x86", require_static=True)
        self.assertIn("aarch64-unknown-linux-musl", str(caught.exception))
        (self.tmp / "dyn").write_bytes(elf(183, interpreter=True))
        with self.assertRaises(deploy.DeployError) as caught:
            deploy.check_linux_arm64(self.tmp / "dyn", require_static=True)
        self.assertIn("dynamically linked", str(caught.exception))
        deploy.check_linux_arm64(self.tmp / "dyn", require_static=False)  # Caddy may be dynamic

    def test_archive_is_deterministic_and_complete(self) -> None:
        source = {"commit": "abc", "dirty": False}
        first, manifest = deploy.build_archive(self.binary, None, "0.1.0", source)
        second, _ = deploy.build_archive(self.binary, None, "0.1.0", source)
        self.assertEqual(first, second)
        with tarfile.open(fileobj=io.BytesIO(first), mode="r:gz") as archive:
            members = {m.name: m for m in archive.getmembers()}
            release = json.loads(archive.extractfile("release.json").read())
        expected = {"peek-server", "release.json", *(name for name, _ in deploy.HOST_FILES)}
        self.assertEqual(set(members), expected)
        self.assertEqual(members["peek-server"].mode, 0o755)
        self.assertEqual(members["install.sh"].mode, 0o755)
        self.assertTrue(all(m.uid == 0 and m.mtime == 0 and m.isfile() for m in members.values()))
        self.assertEqual(release["version"], "0.1.0")
        self.assertEqual(release["files"]["peek-server"], deploy.sha256(self.binary.read_bytes()))
        self.assertEqual(manifest["commit"], "abc")

    def test_remote_script(self) -> None:
        script = deploy.remote_script("s3://bucket/releases/abc.tar.gz", "f" * 64, "abc", "us-east-1", "bucket", True)
        self.assertIn("sha256sum -c -", script)
        self.assertIn(
            "bash /opt/peek/releases/abc/install.sh /opt/peek/releases/abc us-east-1 bucket --require-tls", script
        )
        self.assertIn("--no-same-owner", script)

    def test_dry_run_calls_no_aws(self) -> None:
        env_path = os.environ.get("PATH", "")
        os.environ["PATH"] = str(self.tmp)  # no aws binary reachable: a real call would fail loudly
        try:
            code, out, err = call(
                deploy, ["--binary", str(self.binary), "--dry-run", "--archive-out", str(self.tmp / "r.tgz")]
            )
        finally:
            os.environ["PATH"] = env_path
        self.assertEqual(code, 0, err)
        plan = json.loads(out)
        self.assertEqual(len(plan["release"]), 16)
        self.assertTrue((self.tmp / "r.tgz").is_file())
        self.assertIn("install.sh", plan["files"])


class HostFileTests(unittest.TestCase):
    def test_host_scripts_run_on_python_3_9(self) -> None:
        # Amazon Linux 2023's /usr/bin/python3 is 3.9: no 3.10+ syntax, no 3.11+ APIs on the host.
        import ast

        for name in ("render-env.py", "backup.py"):
            source = (DEPLOY / name).read_text()
            ast.parse(source, filename=name, feature_version=(3, 9))
            for api in ("hashlib.file_digest", "datetime.UTC", "tomllib", "typing.Self", "ExceptionGroup"):
                self.assertNotIn(api, source, f"{name} uses {api}, which Python 3.9 lacks")

    def test_install_script_parses(self) -> None:
        result = subprocess.run(["bash", "-n", str(DEPLOY / "install.sh")], capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_units_reference_real_paths(self) -> None:
        server = (DEPLOY / "peek-server.service").read_text()
        for line in (
            "User=peek",
            "EnvironmentFile=/etc/peek/runtime.env",
            "ExecStart=/opt/peek/current/peek-server",
            "UMask=0077",
            "NoNewPrivileges=true",
            "ProtectSystem=strict",
            "ProtectHome=true",
            "ReadWritePaths=/var/lib/peek",
            "MemoryMax=1G",
        ):
            self.assertIn(line + "\n", server)
        self.assertIn(
            "ExecStart=/usr/bin/python3 /opt/peek/current/backup.py", (DEPLOY / "peek-backup.service").read_text()
        )
        self.assertIn("OnCalendar=hourly", (DEPLOY / "peek-backup.timer").read_text())
        caddy = (DEPLOY / "Caddyfile").read_text()
        self.assertIn("backend.peek.teamofsilicons.com {", caddy)
        self.assertIn("reverse_proxy 127.0.0.1:8080", caddy)

    def test_caddy_body_limits_fit_the_speech_proxy(self) -> None:
        import re

        caddy = (DEPLOY / "Caddyfile").read_text()
        api = (REPO / "crates" / "client" / "src" / "api.rs").read_text()
        match = re.search(r"pub const LISTEN_MAX_BYTES: usize = (\d+) \* (\d+) \* (\d+);", api)
        self.assertIsNotNone(match, "LISTEN_MAX_BYTES moved; update this test")
        assert match
        listen_max = int(match[1]) * int(match[2]) * int(match[3])
        units = {"MB": 1000**2, "MiB": 1024**2, "KB": 1000, "KiB": 1024}

        def limit(matcher: str) -> int:
            block = re.search(r"request_body @" + matcher + r" \{\s*max_size (\d+)(MiB|MB|KiB|KB)\s*\}", caddy)
            self.assertIsNotNone(block, f"no request_body limit for @{matcher}")
            assert block
            return int(block[1]) * units[block[2]]

        # The listen route carries up to LISTEN_MAX_BYTES of audio plus multipart-free headroom;
        # every other route keeps the 1 MB edge limit. The matchers must not overlap (nested
        # limits apply the smaller one).
        self.assertIn("@speech_listen path /api/v1/speech/listen\n", caddy)
        self.assertIn("@other_routes not path /api/v1/speech/listen\n", caddy)
        self.assertGreater(limit("speech_listen"), listen_max)
        self.assertEqual(limit("other_routes"), 1000**2)
        self.assertNotIn("request_body {", caddy, "an unmatched request_body would cap the listen route too")
        envelope = re.search(r"respond `(.*)` 413", caddy)
        self.assertIsNotNone(envelope, "a 413 at the edge must carry peek's JSON error envelope")
        assert envelope
        body = json.loads(envelope[1])
        self.assertEqual(body["error"]["code"], "payload_too_large")
        self.assertFalse(body["error"]["retryable"])
        caddy_bin = shutil.which("caddy")
        if caddy_bin:
            result = subprocess.run(
                [caddy_bin, "validate", "--config", str(DEPLOY / "Caddyfile"), "--adapter", "caddyfile"],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr[-2000:])

    def _run_rollback(self, fail_restore: bool) -> tuple[subprocess.CompletedProcess[str], Path, str]:
        """Runs install.sh's rollback block alone, with stub systemctl/curl/mv, after a failure."""
        import re

        script = (DEPLOY / "install.sh").read_text()
        block = re.search(r"# --- rollback .*?\n(.*)# --- end rollback ---", script, re.S)
        self.assertIsNotNone(block, "install.sh lost its rollback markers")
        assert block
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, True)
        stubs, etc, locked, saved = root / "bin", root / "etc", root / "locked", root / "backup"
        for directory in (stubs, etc, locked, saved, root / "releases" / "old", root / "releases" / "new"):
            directory.mkdir(parents=True)
        log = root / "systemctl.log"
        (stubs / "systemctl").write_text(f'#!/bin/bash\necho "$*" >> {log}\n')
        (stubs / "curl").write_text("#!/bin/bash\nexit 0\n")
        # GNU mv -T semantics (rename onto the path itself) on any test host.
        (stubs / "mv").write_text(
            "#!/bin/bash\nwhile [[ $1 == -* ]]; do shift; done\n"
            'exec python3 -c "import os,sys; os.replace(sys.argv[1], sys.argv[2])" "$1" "$2"\n'
        )
        for stub in stubs.iterdir():
            stub.chmod(0o755)
        targets = [etc / "runtime.env", locked / "caddy", etc / "Caddyfile", etc / "new.service"]
        for target in targets[:3]:
            (saved / target.name).write_text(f"old {target.name}\n")
            target.write_text(f"new {target.name}\n")
        targets[3].write_text("created by the failed install\n")
        current = root / "current"
        current.symlink_to(root / "releases" / "new")
        if fail_restore:
            # Like ETXTBSY on the running Caddy: this restore cannot happen.
            locked.chmod(0o555)
            self.addCleanup(locked.chmod, 0o755)
        harness = root / "harness.sh"
        harness.write_text(
            "set -euo pipefail\n"
            f"managed=({' '.join(str(t) for t in targets)})\n"
            f"backup={saved}\nprevious={root / 'releases' / 'old'}\ncurrent_link={current}\n"
            "local_healthz=http://127.0.0.1:1/healthz\n"
            + block[1]
            + "trap rollback EXIT\nfalse\n"
        )
        env = {**os.environ, "PATH": f"{stubs}:{os.environ['PATH']}"}
        result = subprocess.run(["bash", str(harness)], capture_output=True, text=True, env=env, check=False)
        calls = log.read_text() if log.exists() else ""
        return result, root, calls

    def test_rollback_restores_and_confirms(self) -> None:
        result, root, calls = self._run_rollback(fail_restore=False)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("peek install: restored ", result.stderr)
        self.assertNotIn("INCOMPLETE", result.stderr)
        self.assertEqual((root / "etc" / "runtime.env").read_text(), "old runtime.env\n")
        self.assertEqual((root / "locked" / "caddy").read_text(), "old caddy\n")
        self.assertFalse((root / "etc" / "new.service").exists())
        self.assertEqual(os.readlink(root / "current"), str(root / "releases" / "old"))
        self.assertIn("restart peek-server", calls)
        self.assertFalse((root / "backup").exists(), "the backup directory is removed")

    def test_rollback_keeps_going_when_a_restore_fails(self) -> None:
        # Before the fix, `cp -p` over the running Caddy (ETXTBSY) aborted the trap under
        # set -e: no symlink restore, no daemon-reload, no restart, and a false "restored" claim.
        result, root, calls = self._run_rollback(fail_restore=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("could not restore", result.stderr)
        self.assertIn("peek install: ROLLBACK INCOMPLETE", result.stderr)
        self.assertNotIn("peek install: restored ", result.stderr)
        # Every later step still ran.
        self.assertEqual((root / "etc" / "Caddyfile").read_text(), "old Caddyfile\n")
        self.assertFalse((root / "etc" / "new.service").exists())
        self.assertEqual(os.readlink(root / "current"), str(root / "releases" / "old"))
        self.assertIn("daemon-reload", calls)
        self.assertIn("restart peek-server", calls)
        self.assertIn("reload-or-restart caddy", calls)

    def test_deploy_reports_the_hosts_real_rollback_outcome(self) -> None:
        self.assertIn("restored the previous release", deploy.rollback_report("c", "Failed", "peek install: restored /x\n"))
        self.assertIn("INCOMPLETE", deploy.rollback_report("c", "Failed", "peek install: ROLLBACK INCOMPLETE: …\n"))
        self.assertIn("did not confirm", deploy.rollback_report("c", "Failed", "Killed\n"))
        self.assertNotIn("restored", deploy.rollback_report("c", "TimedOut", ""))

    @unittest.skipUnless(shutil.which("ruby"), "ruby parses the CloudFormation YAML (custom tags) without PyYAML")
    def test_stack_structure(self) -> None:
        program = r"""
require "yaml"; require "json"
class Tagged; end
doc = Psych.parse_file(ARGV[0])
def plain(node)
  case node
  when Psych::Nodes::Scalar then node.tag ? {"tag" => node.tag, "value" => node.value} : node.value
  when Psych::Nodes::Sequence then v = node.children.map { |c| plain(c) }; node.tag ? {"tag" => node.tag, "value" => v} : v
  when Psych::Nodes::Mapping then h = {}; node.children.each_slice(2) { |k, v| h[plain(k)] = plain(v) }; node.tag ? {"tag" => node.tag, "value" => h} : h
  when Psych::Nodes::Document then plain(node.root)
  when Psych::Nodes::Stream then plain(node.children[0])
  end
end
puts JSON.generate(plain(doc))
"""
        result = subprocess.run(
            ["ruby", "-e", program, str(DEPLOY / "stack.yaml")], capture_output=True, text=True, check=False
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        stack = json.loads(result.stdout)
        resources = stack["Resources"]
        self.assertEqual(resources["Runtime"]["Properties"]["Name"], "silicon-peek/production/runtime")
        instance = resources["Instance"]["Properties"]
        self.assertEqual(instance["InstanceType"], "t4g.small")
        self.assertEqual(instance["MetadataOptions"]["HttpTokens"], "required")
        self.assertEqual(instance["MetadataOptions"]["HttpPutResponseHopLimit"], "1")
        ebs = instance["BlockDeviceMappings"][0]["Ebs"]
        self.assertEqual(
            (ebs["VolumeSize"], ebs["VolumeType"], ebs["Encrypted"], ebs["DeleteOnTermination"]),
            ("20", "gp3", "true", "false"),
        )
        self.assertEqual(resources["Address"]["Condition"], "CreateEip")
        ports = sorted(rule["FromPort"] for rule in resources["SecurityGroup"]["Properties"]["SecurityGroupIngress"])
        self.assertEqual(ports, ["443", "80"])
        rules = {r["Id"]: r for r in resources["Artifacts"]["Properties"]["LifecycleConfiguration"]["Rules"]}
        self.assertEqual(
            (rules["ShortLivedRecoverySnapshots"]["Prefix"], rules["ShortLivedRecoverySnapshots"]["ExpirationInDays"]),
            ("backups/", "7"),
        )
        statements = resources["Role"]["Properties"]["Policies"][0]["PolicyDocument"]["Statement"]
        self.assertEqual(
            sorted(s["Action"] for s in statements), ["s3:GetObject", "s3:PutObject", "secretsmanager:GetSecretValue"]
        )
        # Every !Ref / !GetAtt / !If names something that exists.
        names = set(resources) | set(stack["Parameters"]) | {"AWS::Region", "AWS::AccountId", "AWS::StackName"}
        conditions = set(stack["Conditions"])

        def walk(node):
            if isinstance(node, dict):
                if node.get("tag") == "!Ref":
                    self.assertIn(node["value"], names)
                elif node.get("tag") == "!GetAtt":
                    self.assertIn(node["value"].split(".")[0], names)
                elif node.get("tag") == "!If":
                    self.assertIn(node["value"][0], conditions)
                for value in node.values():
                    walk(value)
            elif isinstance(node, list):
                for value in node:
                    walk(value)

        walk(resources)
        walk(stack["Outputs"])
        for resource in resources.values():
            if "Condition" in resource:
                self.assertIn(resource["Condition"], conditions)


if __name__ == "__main__":
    unittest.main()
