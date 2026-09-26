"""Tests for scripts/package-honeycomb.py, scripts/sync-cli-docs.py and scripts/check-installer.sh.

Run: python3 -m unittest discover -s scripts/tests
Set PEEK_TEST_HONEYCOMB=/path/to/honeycomb to also run the real `honeycomb validate` / `pack`
on a synthetic stage (local-only commands, in a private SILICON_HOME).
"""

from __future__ import annotations

import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fixtures  # noqa: E402

REPO = fixtures.REPO


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


pkg = load("package_honeycomb", REPO / "scripts/package-honeycomb.py")
docs = load("sync_cli_docs", REPO / "scripts/sync-cli-docs.py")


def run_main(module, argv: list[str]) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        code = module.main(argv)
    return code, out.getvalue(), err.getvalue()


class Workspace(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="peek-pkg-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.root = fixtures.write_repo(self.tmp / "repo")
        self.bins = self.tmp / "bins"
        fixtures.write_binaries(self.bins)
        self.zip = fixtures.write_app_zip(self.tmp / "dist" / "Peek.app.zip")

    def stage_args(self, *extra: str) -> list[str]:
        return [
            "--root",
            str(self.root),
            "--version",
            "0.1.0",
            "--out",
            str(self.tmp / "dist" / "stage"),
            "--binaries-dir",
            str(self.bins),
            "--app-zip",
            str(self.zip),
            "--skip-signature-check",
            "--no-validate",
            *extra,
        ]


class VersionTests(Workspace):
    def test_consistent_versions(self) -> None:
        versions = pkg.repo_versions(self.root)
        self.assertEqual(versions.version, "0.1.0")
        self.assertEqual(versions.bundle_version, 1000)
        self.assertEqual(len(versions.sources), 6)

    def test_bundle_version_formula(self) -> None:
        self.assertEqual(pkg.bundle_version_for("1.2.3"), 1_002_003)
        for bad in ("0.1", "0.01.0", "1.2.3-rc1", "0.1000.0"):
            with self.assertRaises(pkg.PackagingError, msg=bad):
                pkg.bundle_version_for(bad)

    def test_mismatched_crate_version_is_named(self) -> None:
        fixtures.write_repo(self.root, cli_version="0.1.1")
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.repo_versions(self.root)
        self.assertIn("crates/cli/Cargo.toml: 0.1.1", str(caught.exception))

    def test_wrong_current_project_version(self) -> None:
        fixtures.write_repo(self.root, build="1001")
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.repo_versions(self.root)
        self.assertIn("CURRENT_PROJECT_VERSION is 1001", str(caught.exception))

    def test_requested_version_must_match(self) -> None:
        with self.assertRaises(pkg.PackagingError):
            pkg.repo_versions(self.root, "0.2.0")

    def test_tag_must_match(self) -> None:
        old = {k: os.environ.get(k) for k in ("GITHUB_REF", "GITHUB_REF_NAME")}
        os.environ.update(GITHUB_REF="refs/tags/v0.1.1", GITHUB_REF_NAME="v0.1.1")
        try:
            with self.assertRaises(pkg.PackagingError):
                pkg.repo_versions(self.root)
        finally:
            for key, value in old.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value

    def test_real_manifest_is_manifest_b(self) -> None:
        text = (REPO / "honeycomb.yaml").read_text()
        self.assertNotIn("install_script", text.split("\n", 1)[1])
        for platform, target in pkg.TARGETS.items():
            self.assertIn(f'root: "targets/{platform}"', text)
            self.assertIn(f'cli: "{target.executable}"', text)


class BinaryTests(Workspace):
    def write(self, name: str, data: bytes) -> Path:
        path = self.tmp / name
        path.write_bytes(data)
        path.chmod(0o755)
        return path

    def test_every_fixture_passes(self) -> None:
        for platform in pkg.TARGETS:
            pkg.verify_binary(pkg.binary_source(platform, None, self.bins), platform)

    def test_wrong_architecture(self) -> None:
        cases = {
            "linux-aarch64": fixtures.elf("x86_64"),
            "macos-x86_64": fixtures.macho("aarch64"),
            "windows-aarch64": fixtures.pe("x86_64"),
        }
        for platform, data in cases.items():
            with self.assertRaises(pkg.PackagingError, msg=platform):
                pkg.verify_binary(self.write(platform, data), platform)

    def test_dynamic_linux_binary_is_refused(self) -> None:
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.verify_binary(self.write("dyn", fixtures.elf("x86_64", interpreter=True)), "linux-x86_64")
        self.assertIn("dynamically linked", str(caught.exception))

    def test_dynamic_crt_is_refused(self) -> None:
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.verify_binary(self.write("crt.exe", fixtures.pe("x86_64", dynamic_crt=True)), "windows-x86_64")
        self.assertIn("crt-static", str(caught.exception))

    def test_universal_cli_is_accepted_and_minos_checked(self) -> None:
        universal = fixtures.fat(fixtures.macho("aarch64"), fixtures.macho("x86_64"))
        pkg.verify_binary(self.write("universal", universal), "macos-aarch64")
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.verify_binary(self.write("new", fixtures.macho("aarch64", (26, 0))), "macos-aarch64")
        self.assertIn("MACOSX_DEPLOYMENT_TARGET=11.0", str(caught.exception))

    def test_scripts_and_symlinks_are_refused(self) -> None:
        script = self.write("script", b"#!/bin/sh\necho peek\n")
        with self.assertRaises(pkg.PackagingError):
            pkg.verify_binary(script, "macos-aarch64")
        link = self.tmp / "link"
        link.symlink_to(pkg.binary_source("macos-aarch64", None, self.bins))
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.verify_binary(link, "macos-aarch64")
        self.assertIn("symlink", str(caught.exception))


class AppZipTests(Workspace):
    def test_inspects_dev_zip(self) -> None:
        app = pkg.inspect_app_zip(self.zip, pkg.repo_versions(self.root))
        self.assertEqual((app.bundle_id, app.bundle_version, app.short_version), ("ai.tos.peek.dev", "1000", "0.1.0"))
        self.assertEqual(pkg.resolve_team_id(app, None, None), "")

    def test_apple_double_is_refused(self) -> None:
        bad = fixtures.write_app_zip(self.tmp / "bad.zip", apple_double=True)
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.inspect_app_zip(bad, pkg.repo_versions(self.root))
        self.assertIn("--norsrc", str(caught.exception))

    def test_thin_peekd_is_refused(self) -> None:
        thin = fixtures.write_app_zip(self.tmp / "thin.zip", helper_arches=("aarch64",))
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.inspect_app_zip(thin, pkg.repo_versions(self.root))
        self.assertIn("x86_64", str(caught.exception))

    def test_version_mismatch_is_refused(self) -> None:
        stale = fixtures.write_app_zip(self.tmp / "stale.zip", version="0.0.9", build="9")
        with self.assertRaises(pkg.PackagingError):
            pkg.inspect_app_zip(stale, pkg.repo_versions(self.root))

    def test_team_pairing(self) -> None:
        prod = pkg.inspect_app_zip(
            fixtures.write_app_zip(self.tmp / "p.zip", bundle_id="ai.tos.peek"), pkg.repo_versions(self.root)
        )
        self.assertEqual(pkg.resolve_team_id(prod, None, None), "LTBSK59BJ2")
        with self.assertRaises(pkg.PackagingError):
            pkg.resolve_team_id(prod, "", None)
        dev = pkg.inspect_app_zip(self.zip, pkg.repo_versions(self.root))
        with self.assertRaises(pkg.PackagingError):
            pkg.resolve_team_id(dev, "LTBSK59BJ2", None)

    def test_build_info_must_describe_this_zip(self) -> None:
        app = pkg.inspect_app_zip(self.zip, pkg.repo_versions(self.root))
        info = self.tmp / "Peek.app.build.json"
        info.write_text(json.dumps({"team_id": "", "zip_sha256": "0" * 64}))
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.resolve_team_id(app, None, info)
        self.assertIn("different Peek.app.zip", str(caught.exception))
        info.write_text(json.dumps({"team_id": "", "zip_sha256": app.sha256}))
        self.assertEqual(pkg.resolve_team_id(app, None, info), "")


class StageTests(Workspace):
    def test_stage_layout_and_info(self) -> None:
        code, out, err = run_main(pkg, self.stage_args())
        self.assertEqual(code, 0, err)
        summary = json.loads(out)
        stage = Path(summary["stage"])
        self.assertEqual(sorted(p.name for p in stage.iterdir()), ["honeycomb.yaml", "targets"])
        pkg.assert_clean_tree(stage)
        info = (stage / "targets/macos-x86_64/Peek.app.info").read_text()
        self.assertEqual(
            info.splitlines(),
            [
                "bundle_id=ai.tos.peek.dev",
                "bundle_version=1000",
                "short_version=0.1.0",
                "team_id=",
                f"zip_sha256={pkg.sha256_file(self.zip)}",
                "minimum_system_version=26.0",
            ],
        )
        self.assertEqual((stage / "honeycomb.yaml").read_bytes(), (self.root / "honeycomb.yaml").read_bytes())
        self.assertEqual(
            pkg.sha256_file(stage / "targets/macos-aarch64/Peek.app.zip"),
            pkg.sha256_file(stage / "targets/macos-x86_64/Peek.app.zip"),
        )
        self.assertTrue(os.access(stage / "targets/macos-aarch64/install-app.sh", os.X_OK))
        self.assertTrue((stage.parent / "peek-0.1.0-stage.sha256").is_file())

    def test_existing_stage_needs_replace(self) -> None:
        self.assertEqual(run_main(pkg, self.stage_args())[0], 0)
        code, _, err = run_main(pkg, self.stage_args())
        self.assertEqual(code, 1)
        self.assertIn("--replace", err)
        self.assertEqual(run_main(pkg, self.stage_args("--replace"))[0], 0)

    def test_replace_refuses_foreign_directories(self) -> None:
        out = self.tmp / "dist" / "stage"
        out.mkdir(parents=True)
        (out / "notes.txt").write_text("keep me")
        code, _, err = run_main(pkg, self.stage_args("--replace"))
        self.assertEqual(code, 1)
        self.assertIn("refusing to delete", err)
        self.assertTrue((out / "notes.txt").exists())

    def test_hard_links_and_symlinks_are_detected(self) -> None:
        self.assertEqual(run_main(pkg, self.stage_args())[0], 0)
        stage = self.tmp / "dist" / "stage"
        license_file = stage / "targets/linux-x86_64/LICENSE"
        os.link(license_file, self.tmp / "extra-link")
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.assert_clean_tree(stage)
        self.assertIn("hard-linked", str(caught.exception))
        (self.tmp / "extra-link").unlink()
        license_file.unlink()
        license_file.symlink_to(self.root / "LICENSE")
        with self.assertRaises(pkg.PackagingError) as caught:
            pkg.assert_clean_tree(stage)
        self.assertIn("symlink", str(caught.exception))

    def test_check_binaries_mode(self) -> None:
        code, out, err = run_main(pkg, ["--root", str(self.root), "--check-binaries", "--binaries-dir", str(self.bins)])
        self.assertEqual(code, 0, err)
        self.assertEqual(len(json.loads(out)["binaries"]), 6)

    def test_verify_archive(self) -> None:
        self.assertEqual(run_main(pkg, self.stage_args())[0], 0)
        stage = self.tmp / "dist" / "stage"
        archive = self.tmp / "peek.tar.gz"
        with tarfile.open(archive, "w:gz") as package:
            package.add(stage / "honeycomb.yaml", "honeycomb.yaml")
            package.add(stage / "targets", "targets")
        code, out, err = run_main(
            pkg, ["--root", str(self.root), "--verify-archive", str(archive), "--out", str(stage)]
        )
        self.assertEqual(code, 0, err)
        self.assertTrue(json.loads(out)["verified"])
        with tarfile.open(self.tmp / "bad.tar.gz", "w:gz") as package:
            package.add(stage / "honeycomb.yaml", "honeycomb.yaml")
            package.add(stage / "targets", "targets")
            extra = tarfile.TarInfo("targets/linux-x86_64/bin/peek-link")
            extra.type, extra.linkname = tarfile.SYMTYPE, "peek"
            package.addfile(extra)
        code, _, err = run_main(pkg, ["--root", str(self.root), "--verify-archive", str(self.tmp / "bad.tar.gz")])
        self.assertEqual(code, 1)
        self.assertIn("link", err)

    def test_verify_archive_checks_every_packed_executable(self) -> None:
        self.assertEqual(run_main(pkg, self.stage_args())[0], 0)
        stage = self.tmp / "dist" / "stage"
        archive = self.tmp / "peek.tar.gz"
        with tarfile.open(archive, "w:gz") as package:
            package.add(stage / "honeycomb.yaml", "honeycomb.yaml")
            package.add(stage / "targets", "targets")
        code, out, err = run_main(pkg, ["--root", str(self.root), "--verify-archive", str(archive)])
        self.assertEqual(code, 0, err)
        binaries = json.loads(out)["binaries"]
        self.assertEqual(sorted(binaries), sorted(pkg.TARGETS))
        self.assertIn("ELF64 aarch64", binaries["linux-aarch64"])
        self.assertIn("PE32+ machine 0xaa64", binaries["windows-aarch64"])
        self.assertIn("Mach-O x86_64", binaries["macos-x86_64"])
        # An x86_64 PE packed as the ARM64 Windows executable: same layout, wrong CPU.
        wrong = stage / "targets/windows-aarch64/bin/peek.exe"
        wrong.write_bytes(fixtures.pe("x86_64"))
        with tarfile.open(self.tmp / "wrong.tar.gz", "w:gz") as package:
            package.add(stage / "honeycomb.yaml", "honeycomb.yaml")
            package.add(stage / "targets", "targets")
        code, _, err = run_main(pkg, ["--root", str(self.root), "--verify-archive", str(self.tmp / "wrong.tar.gz")])
        self.assertEqual(code, 1)
        self.assertIn("targets/windows-aarch64/bin/peek.exe is PE machine 0x8664", err)


@unittest.skipUnless(
    os.environ.get("PEEK_TEST_HONEYCOMB"), "set PEEK_TEST_HONEYCOMB to run the real Honeycomb validator"
)
class HoneycombIntegration(Workspace):
    def test_validate_pack_verify(self) -> None:
        honeycomb = os.environ["PEEK_TEST_HONEYCOMB"]
        code, out, err = run_main(
            pkg, [a for a in self.stage_args() if a != "--no-validate"] + ["--honeycomb", honeycomb]
        )
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(out)["honeycomb_validate"], "passed")
        stage = self.tmp / "dist" / "stage"
        archive = self.tmp / "dist" / "peek-0.1.0.tar.gz"
        with tempfile.TemporaryDirectory() as home:
            env = pkg.isolated_honeycomb_env(Path(home))
            subprocess.run(
                [honeycomb, "--json", "pack", str(stage), "--output", str(archive)],
                env=env,
                check=True,
                capture_output=True,
            )
            report = json.loads(
                subprocess.run(
                    [honeycomb, "--json", "validate", str(archive)], env=env, check=True, capture_output=True, text=True
                ).stdout
            )
        self.assertTrue(report["valid"], report)
        code, out, err = run_main(
            pkg, ["--root", str(self.root), "--verify-archive", str(archive), "--out", str(stage)]
        )
        self.assertEqual(code, 0, err)


class DocsSyncTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="peek-docs-test-"))
        self.addCleanup(shutil.rmtree, self.root, True)
        (self.root / "docs/dev").mkdir(parents=True)
        (self.root / "docs/start.md").write_text("# Start\n")
        (self.root / "docs/dev/ipc.md").write_text("# IPC\n")

    def test_sync_then_check(self) -> None:
        self.assertEqual(run_main(docs, ["--root", str(self.root), "--check"])[0], 1)
        self.assertEqual(run_main(docs, ["--root", str(self.root)])[0], 0)
        self.assertEqual((self.root / "crates/cli/docs/dev/ipc.md").read_text(), "# IPC\n")
        self.assertEqual(run_main(docs, ["--root", str(self.root), "--check"])[0], 0)

    def test_stale_and_changed_copies(self) -> None:
        run_main(docs, ["--root", str(self.root)])
        (self.root / "crates/cli/docs/old.md").write_text("gone")
        (self.root / "docs/start.md").write_text("# Start v2\n")
        code, _, err = run_main(docs, ["--root", str(self.root), "--check"])
        self.assertEqual(code, 1)
        self.assertIn("stale", err)
        self.assertIn("out of date", err)
        run_main(docs, ["--root", str(self.root)])
        self.assertFalse((self.root / "crates/cli/docs/old.md").exists())
        self.assertEqual((self.root / "crates/cli/docs/start.md").read_text(), "# Start v2\n")

    def test_non_utf8_is_refused(self) -> None:
        (self.root / "docs/bad.md").write_bytes(b"\xff\xfe")
        code, _, err = run_main(docs, ["--root", str(self.root)])
        self.assertEqual(code, 1)
        self.assertIn("UTF-8", err)


class InstallerTests(unittest.TestCase):
    def tree(self, served: bytes | None) -> Path:
        root = Path(tempfile.mkdtemp(prefix="peek-installer-test-"))
        self.addCleanup(shutil.rmtree, root, True)
        (root / "scripts").mkdir()
        shutil.copyfile(REPO / "scripts/check-installer.sh", root / "scripts/check-installer.sh")
        shutil.copyfile(REPO / "scripts/install.sh", root / "scripts/install.sh")
        if served is not None:
            (root / "web/public").mkdir(parents=True)
            (root / "web/public/install.sh").write_bytes(served)
        return root

    def check(self, root: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["/bin/sh", str(root / "scripts/check-installer.sh")], capture_output=True, text=True, check=False
        )

    def test_identical_copies_pass(self) -> None:
        result = self.check(self.tree((REPO / "scripts/install.sh").read_bytes()))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_drift_and_absence_fail(self) -> None:
        drift = self.check(self.tree((REPO / "scripts/install.sh").read_bytes() + b"# drift\n"))
        self.assertEqual(drift.returncode, 1)
        self.assertIn("differs", drift.stderr)
        self.assertEqual(self.check(self.tree(None)).returncode, 1)

    def test_canonical_installer_matches_blueprint_shape(self) -> None:
        text = (REPO / "scripts/install.sh").read_text()
        self.assertTrue(text.startswith("#!/bin/sh\n"))
        self.assertTrue(text.endswith('install_peek "$@"\n'))
        self.assertIn('"$PEEK" app install --json', text)


class ShellSyntaxTests(unittest.TestCase):
    def test_scripts_parse(self) -> None:
        for relative, shell in (
            ("scripts/install-app.sh", "/bin/sh"),
            ("scripts/install.sh", "/bin/sh"),
            ("scripts/check-installer.sh", "/bin/sh"),
            ("scripts/build-cli-release.sh", "bash"),
            ("scripts/build-mac-release.sh", "bash"),
            ("scripts/build-release.sh", "bash"),
            ("scripts/lib/signing.sh", "bash"),
            ("scripts/operator/toolbox.sh", "bash"),
            ("scripts/testing/bootstrap.sh", "bash"),
        ):
            result = subprocess.run([shell, "-n", str(REPO / relative)], capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 0, f"{relative}: {result.stderr}")

    def test_toolbox_sourcing_is_inert(self) -> None:
        home = Path(tempfile.mkdtemp(prefix="peek-toolbox-test-"))
        self.addCleanup(shutil.rmtree, home, True)
        script = f'. "{REPO}/scripts/operator/toolbox.sh"; peek_ops_setup; echo "rc=$?"; env | grep -c "^SILICON_HOME=" || true'
        for shell in ("bash", "zsh"):
            if shutil.which(shell) is None:
                continue
            result = subprocess.run(
                [shell, "-c", script],
                env={"HOME": str(home), "PATH": os.environ["PATH"]},
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertIn("rc=1", result.stdout, f"{shell}: {result.stderr}")
            self.assertIn("Not run", result.stderr)
            self.assertEqual(list(home.iterdir()), [], f"{shell}: sourcing or an unconfirmed call wrote files")
        executed = subprocess.run(
            ["bash", str(REPO / "scripts/operator/toolbox.sh")], capture_output=True, text=True, check=False
        )
        self.assertEqual(executed.returncode, 2)


class InstallAppScriptTests(unittest.TestCase):
    """install-app.sh always exits 0 and degrades clearly (hermetic: scratch dirs, no launch)."""

    @unittest.skipUnless(sys.platform == "darwin", "install-app.sh targets macOS")
    def test_missing_payload_degrades_and_exits_zero(self) -> None:
        scratch = Path(tempfile.mkdtemp(prefix="peek-install-app-test-"))
        self.addCleanup(shutil.rmtree, scratch, True)
        package = scratch / "pkg"
        package.mkdir()
        shutil.copyfile(REPO / "scripts/install-app.sh", package / "install-app.sh")
        env = {
            "PEEK_INSTALL_APPLICATIONS_DIR": str(scratch / "Applications"),
            "PEEK_INSTALL_SUPPORT_DIR": str(scratch / "Support"),
            "PEEK_INSTALL_NO_LAUNCH": "1",
            "HOME": str(scratch),
        }
        result = subprocess.run(
            ["/bin/sh", str(package / "install-app.sh")],
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("[degraded] payload", result.stderr)
        self.assertFalse((scratch / "Applications" / "Peek.app").exists())
        status = (scratch / "Support" / "install-status.txt").read_text()
        self.assertIn("degraded\tpayload", status)

    @unittest.skipUnless(sys.platform == "darwin", "install-app.sh targets macOS")
    def test_corrupt_zip_degrades(self) -> None:
        scratch = Path(tempfile.mkdtemp(prefix="peek-install-app-test-"))
        self.addCleanup(shutil.rmtree, scratch, True)
        package = scratch / "pkg"
        package.mkdir()
        shutil.copyfile(REPO / "scripts/install-app.sh", package / "install-app.sh")
        (package / "Peek.app.zip").write_bytes(b"not a zip")
        (package / "Peek.app.info").write_text(
            "bundle_id=ai.tos.peek.dev\nbundle_version=1000\nshort_version=0.1.0\nteam_id=\n"
            f"zip_sha256={'0' * 64}\nminimum_system_version=26.0\n"
        )
        env = {
            "PEEK_INSTALL_APPLICATIONS_DIR": str(scratch / "Applications"),
            "PEEK_INSTALL_SUPPORT_DIR": str(scratch / "Support"),
            "PEEK_INSTALL_NO_LAUNCH": "1",
            "HOME": str(scratch),
        }
        result = subprocess.run(
            ["/bin/sh", str(package / "install-app.sh")],
            env=env,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("[ok] offer", result.stderr)
        self.assertIn("[degraded] install", result.stderr)
        self.assertFalse((scratch / "Applications" / "Peek.app").exists())
        self.assertEqual(len(list((scratch / "Support" / "offers").glob("1000-000000000000.app.*"))), 2)


if __name__ == "__main__":
    unittest.main()
