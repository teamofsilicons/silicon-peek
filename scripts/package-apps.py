#!/usr/bin/env python3
"""Stage the peek Silicon Apps package (Manifest A, BLUEPRINT §4.1 as amended for 0.1.2) and verify every payload.

The stage written to --out holds exactly what `silicon-apps pack` archives:

  apps.yaml                        byte copy of the repository manifest
  targets/macos-aarch64/bin/peek        arm64 Mach-O (thin, or universal containing arm64)
  targets/macos-aarch64/Peek.app.zip    universal Peek.app, zipped with ditto --norsrc (§4.3)
  targets/macos-aarch64/Peek.app.info   key=value sidecar read by ensure_app() and install-app.sh
  targets/macos-aarch64/install-app.sh  0755; Manifest A's install_script (Silicon Apps >= 0.5.0 runs it)
  targets/macos-aarch64/LICENSE
  targets/macos-x86_64/...              the same zip, info, script and license bytes
  targets/linux-{x86_64,aarch64}/       bin/peek (static musl ELF) + LICENSE
  targets/windows-{x86_64,aarch64}/     bin/peek.exe (PE32+, static CRT) + LICENSE

Silicon Apps's validator does not inspect binary formats, so this script checks the ELF,
Mach-O and PE headers of every executable itself (adapted from
silicon-dm/scripts/package-release.py), refuses symlinks, hard links and special files,
and requires one version everywhere: every Cargo.toml, apps.yaml, apps/mac/project.yml
MARKETING_VERSION / CURRENT_PROJECT_VERSION, the zipped Info.plist and, in CI, the git tag.

Other modes: --check-versions, --check-binaries, --verify-archive ARCHIVE.
Silicon Apps runs in a private temporary SILICON_HOME with updates, services and telemetry off,
so packaging never touches the operator's Silicon Apps state. Requires Python 3.11+.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import os
import plistlib
import re
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile

try:
    import tomllib
except ModuleNotFoundError:  # Python < 3.11, e.g. the Command Line Tools' /usr/bin/python3 (3.9)
    sys.exit("package-apps.py needs Python 3.11 or newer (tomllib); run it with python3.11+")
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
APP_ID = "peek"
TEAM_ID = "LTBSK59BJ2"
BUNDLE_ID = "ai.tos.peek"
DEV_BUNDLE_ID = "ai.tos.peek.dev"
DAEMON_IDENTIFIER = "ai.tos.peek.daemon"
MINIMUM_SYSTEM_VERSION = "26.0"
CLI_MACOS_DEPLOYMENT_TARGET = (11, 0)
APP_MACOS_DEPLOYMENT_TARGET = (26, 0)
CRATES = {
    ".": "silicon-peek",
    "crates/client": "silicon-peek-client",
    "crates/cli": "silicon-peek-cli",
    "crates/daemon": "silicon-peek-daemon",
}
ZIP_REQUIRED_ENTRIES = (
    "Peek.app/Contents/Info.plist",
    "Peek.app/Contents/MacOS/Peek",
    "Peek.app/Contents/Helpers/peekd",
    "Peek.app/Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist",
    "Peek.app/Contents/_CodeSignature/CodeResources",
)
APPS_ENV = {
    "APPS_AUTO_UPDATE": "0",
    "APPS_NO_SERVICE": "1",
    "APPS_TELEMETRY": "0",
    "APPS_NO_MODIFY_PATH": "1",
}

# Mach-O CPU types (mach/machine.h) and ELF/PE machine numbers.
CPU_TYPE_ARM64 = 0x0100000C
CPU_TYPE_X86_64 = 0x01000007
ELF_MACHINE = {"aarch64": 183, "x86_64": 62}
PE_MACHINE = {"aarch64": 0xAA64, "x86_64": 0x8664}
MACHO_CPU = {"aarch64": CPU_TYPE_ARM64, "x86_64": CPU_TYPE_X86_64}


class PackagingError(Exception):
    """A precise, user-facing failure: what failed, why, and how to fix it."""


@dataclasses.dataclass(frozen=True)
class Target:
    triple: str
    executable: str  # path relative to the target root, as apps.yaml maps `cli`

    @property
    def os(self) -> str:
        return self.triple.split("-")[2] if "windows" not in self.triple else "windows"

    @property
    def arch(self) -> str:
        return self.triple.split("-", 1)[0]

    @property
    def binary_name(self) -> str:
        return PurePosixPath(self.executable).name


# BLUEPRINT §4.1: all six 64-bit targets are mandatory; Linux uses musl (§4.4).
TARGETS: dict[str, Target] = {
    "macos-aarch64": Target("aarch64-apple-darwin", "bin/peek"),
    "macos-x86_64": Target("x86_64-apple-darwin", "bin/peek"),
    "linux-x86_64": Target("x86_64-unknown-linux-musl", "bin/peek"),
    "linux-aarch64": Target("aarch64-unknown-linux-musl", "bin/peek"),
    "windows-x86_64": Target("x86_64-pc-windows-msvc", "bin/peek.exe"),
    "windows-aarch64": Target("aarch64-pc-windows-msvc", "bin/peek.exe"),
}
MAC_PAYLOADS = ("Peek.app.zip", "Peek.app.info", "install-app.sh")
# Manifest A: apps.yaml sets install_script: "install-app.sh" on exactly these targets.
MANIFEST_A_SCRIPT = "install-app.sh"
MANIFEST_A_TARGETS = ("macos-aarch64", "macos-x86_64")


def log(message: str) -> None:
    print(f"package-apps: {message}", file=sys.stderr, flush=True)


def rel(path: Path, root: Path) -> str:
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return str(path)


def sha256_file(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


# --------------------------------------------------------------------------- versions


@dataclasses.dataclass(frozen=True)
class Versions:
    version: str
    bundle_version: int
    sources: dict[str, str]


def bundle_version_for(version: str) -> int:
    """CFBundleVersion = major*1_000_000 + minor*1_000 + patch (BLUEPRINT §4.1)."""
    match = re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version)
    if not match:
        raise PackagingError(
            f"version {version!r} is not plain x.y.z; Silicon Apps uploads and "
            "CFBundleShortVersionString both need three dot-separated integers without leading zeros"
        )
    major, minor, patch = (int(part) for part in match.groups())
    if minor > 999 or patch > 999:
        raise PackagingError(
            f"version {version}: minor and patch must stay below 1000 so CFBundleVersion "
            "(major*1000000 + minor*1000 + patch) keeps increasing strictly"
        )
    return major * 1_000_000 + minor * 1_000 + patch


def _crate_version(root: Path, directory: str, expected_name: str, workspace: str | None) -> str:
    path = root / directory / "Cargo.toml"
    if not path.is_file():
        raise PackagingError(
            f"{rel(path, root)} is missing; the workspace layout is fixed by BLUEPRINT §1.3 "
            f"({expected_name} lives in {directory})"
        )
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as error:
        raise PackagingError(f"{rel(path, root)} is not valid TOML: {error}") from error
    package = data.get("package")
    if not isinstance(package, dict):
        raise PackagingError(f"{rel(path, root)} has no [package] table for {expected_name}")
    if package.get("name") != expected_name:
        raise PackagingError(
            f"{rel(path, root)} declares package {package.get('name')!r}; BLUEPRINT D2 names it {expected_name!r}"
        )
    version = package.get("version")
    if isinstance(version, dict) and version.get("workspace") is True:
        if workspace is None:
            raise PackagingError(
                f"{rel(path, root)} inherits version.workspace but the root Cargo.toml has no "
                "[workspace.package] version"
            )
        return workspace
    if not isinstance(version, str):
        raise PackagingError(f"{rel(path, root)} [package] version is missing or not a string")
    return version


def _manifest_version(root: Path) -> str:
    path = root / "apps.yaml"
    if not path.is_file() or path.is_symlink():
        raise PackagingError(f"{path} is missing or a symlink")
    manifest = json.loads(path.read_text())
    if manifest.get("schema_version") != 1 or manifest.get("app_id") != APP_ID or manifest.get("command") != APP_ID:
        raise PackagingError("apps.yaml must identify peek using schema_version 1")
    for platform, target in TARGETS.items():
        entry = manifest["targets"][platform]
        if entry.get("binary") != f"targets/{platform}/{target.executable}":
            raise PackagingError(f"apps.yaml has an incorrect binary for {platform}")
        wanted = f"targets/{platform}/install-app.sh" if platform.startswith("macos-") else None
        if entry.get("install_script") != wanted:
            raise PackagingError(f"apps.yaml has an incorrect install script for {platform}")
    return manifest["version"]


def _xcode_versions(root: Path) -> tuple[str, str]:
    path = root / "apps/mac/project.yml"
    if not path.is_file():
        raise PackagingError(f"{rel(path, root)} is missing; Peek.app's MARKETING_VERSION lives there (BLUEPRINT §8.1)")
    text = path.read_text(encoding="utf-8")
    result = []
    for key in ("MARKETING_VERSION", "CURRENT_PROJECT_VERSION"):
        found = set(re.findall(rf"\b{key}:\s*[\"']?([0-9.]+)[\"']?", text))
        if len(found) != 1:
            raise PackagingError(f"apps/mac/project.yml must set {key} to one value (found {sorted(found) or 'none'})")
        result.append(found.pop())
    return result[0], result[1]


def repo_versions(root: Path, expected: str | None = None) -> Versions:
    root_toml = root / "Cargo.toml"
    if not root_toml.is_file():
        raise PackagingError(f"{root_toml} is missing; run this script from the silicon-peek repository")
    try:
        root_data = tomllib.loads(root_toml.read_text(encoding="utf-8"))
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as error:
        raise PackagingError(f"{root_toml} is not valid TOML: {error}") from error
    workspace = root_data.get("workspace", {}).get("package", {}).get("version")
    sources: dict[str, str] = {}
    for directory, name in CRATES.items():
        sources[f"{directory}/Cargo.toml".removeprefix("./")] = _crate_version(root, directory, name, workspace)
    sources["apps.yaml"] = _manifest_version(root)
    marketing, build = _xcode_versions(root)
    sources["apps/mac/project.yml MARKETING_VERSION"] = marketing
    distinct = sorted(set(sources.values()))
    if len(distinct) != 1:
        listing = "\n".join(f"  {source}: {value}" for source, value in sources.items())
        raise PackagingError(
            "versions disagree; peek ships one version everywhere (BLUEPRINT §1.3):\n"
            f"{listing}\nSet them all to the same x.y.z and retag."
        )
    version = distinct[0]
    number = bundle_version_for(version)
    if build != str(number):
        raise PackagingError(
            f"apps/mac/project.yml CURRENT_PROJECT_VERSION is {build}; version {version} requires "
            f"{number} (major*1000000 + minor*1000 + patch)"
        )
    if expected is not None and expected != version:
        raise PackagingError(f"--version {expected} was requested but the repository is at {version}")
    ref, ref_name = os.environ.get("GITHUB_REF", ""), os.environ.get("GITHUB_REF_NAME", "")
    if ref.startswith("refs/tags/") and ref_name != f"v{version}":
        raise PackagingError(f"release tag {ref_name} does not match version {version}; tag v{version}")
    return Versions(version, number, sources)


# --------------------------------------------------------------------------- binaries


def _macho_slices(data: bytes) -> list[tuple[int, bytes]]:
    """Return (cputype, slice bytes) for a thin or universal Mach-O; [] if not Mach-O."""
    if data[:4] == b"\xcf\xfa\xed\xfe" and len(data) >= 32:
        return [(struct.unpack_from("<I", data, 4)[0], data)]
    if data[:4] in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf") and len(data) >= 8:
        wide = data[3] == 0xBF
        count = struct.unpack_from(">I", data, 4)[0]
        entry = 32 if wide else 20
        if not 0 < count <= 16 or len(data) < 8 + count * entry:
            return []
        slices = []
        for index in range(count):
            base = 8 + index * entry
            if wide:
                cpu, _sub, offset, size = struct.unpack_from(">IIQQ", data, base)
            else:
                cpu, _sub, offset, size = struct.unpack_from(">IIII", data, base)
            if offset + size > len(data):
                return []
            slices.append((cpu, data[offset : offset + size]))
        return slices
    return []


def _macho_minos(thin: bytes) -> tuple[int, int] | None:
    """Minimum macOS from LC_BUILD_VERSION (0x32) or LC_VERSION_MIN_MACOSX (0x24)."""
    if len(thin) < 32 or thin[:4] != b"\xcf\xfa\xed\xfe":
        return None
    ncmds, sizeofcmds = struct.unpack_from("<II", thin, 16)
    offset, end = 32, min(len(thin), 32 + sizeofcmds)
    for _ in range(ncmds):
        if offset + 8 > end:
            break
        command, size = struct.unpack_from("<II", thin, offset)
        if size < 8:
            break
        encoded = None
        if command == 0x32 and offset + 12 <= end:
            encoded = struct.unpack_from("<I", thin, offset + 12)[0]
        elif command == 0x24 and offset + 12 <= end:
            encoded = struct.unpack_from("<I", thin, offset + 8)[0]
        if encoded is not None:
            return (encoded >> 16, (encoded >> 8) & 0xFF)
        offset += size
    return None


def _check_macho(data: bytes, arches: tuple[str, ...], label: str, newest: tuple[int, int]) -> None:
    slices = _macho_slices(data)
    if not slices:
        raise PackagingError(f"{label} is not a 64-bit Mach-O executable")
    by_cpu = {cpu: thin for cpu, thin in slices}
    for arch in arches:
        thin = by_cpu.get(MACHO_CPU[arch])
        if thin is None:
            present = ", ".join(hex(cpu) for cpu in by_cpu)
            raise PackagingError(f"{label} has no {arch} slice (CPU types present: {present})")
        filetype = struct.unpack_from("<I", thin, 12)[0] if len(thin) >= 16 else 0
        if filetype != 2:
            raise PackagingError(f"{label} {arch} slice is not an executable (Mach-O filetype {filetype})")
        minos = _macho_minos(thin)
        if minos is not None and minos > newest:
            raise PackagingError(
                f"{label} {arch} slice requires macOS {minos[0]}.{minos[1]}; build it with "
                f"MACOSX_DEPLOYMENT_TARGET={newest[0]}.{newest[1]}"
            )


def verify_binary(path: Path, platform: str) -> None:
    """Reject scripts, wrong operating systems, wrong CPUs, dynamic Linux builds and dynamic CRTs."""
    target = TARGETS[platform]
    try:
        info = path.lstat()
    except FileNotFoundError:
        raise PackagingError(
            f"{platform}: {path} does not exist; build it with scripts/build-cli-release.sh (target {target.triple})"
        ) from None
    if stat.S_ISLNK(info.st_mode):
        raise PackagingError(f"{platform}: {path} is a symlink; pass the real build output")
    if not stat.S_ISREG(info.st_mode) or info.st_size == 0:
        raise PackagingError(f"{platform}: {path} is not a non-empty regular file")
    verify_binary_bytes(path.read_bytes(), platform, f"{platform} {path}")


def verify_binary_bytes(data: bytes, platform: str, label: str) -> None:
    """The format checks of verify_binary on the bytes of one executable (a file, or an archive member)."""
    target = TARGETS[platform]
    if target.os == "linux":
        if data[:6] != b"\x7fELF\x02\x01" or len(data) < 64:
            raise PackagingError(f"{label} is not a little-endian ELF64 executable")
        e_type, machine = struct.unpack_from("<HH", data, 16)
        if machine != ELF_MACHINE[target.arch]:
            raise PackagingError(
                f"{label} is ELF machine {machine}, expected {ELF_MACHINE[target.arch]} ({target.arch})"
            )
        if e_type not in (2, 3):
            raise PackagingError(f"{label} is ELF type {e_type}, not an executable")
        phoff = struct.unpack_from("<Q", data, 32)[0]
        phentsize, phnum = struct.unpack_from("<HH", data, 54)
        if phentsize < 4 or phoff + phentsize * phnum > len(data):
            raise PackagingError(f"{label} has a truncated ELF program header table")
        for index in range(phnum):
            if struct.unpack_from("<I", data, phoff + index * phentsize)[0] == 3:  # PT_INTERP
                raise PackagingError(
                    f"{label} is dynamically linked (it names an ELF interpreter); build "
                    f"{target.triple} so the CLI runs on every distribution (BLUEPRINT §4.4)"
                )
    elif target.os == "darwin":
        _check_macho(data, (target.arch,), label, CLI_MACOS_DEPLOYMENT_TARGET)
    else:
        if data[:2] != b"MZ" or len(data) < 64:
            raise PackagingError(f"{label} is not a PE executable (no MZ header)")
        offset = struct.unpack_from("<I", data, 60)[0]
        if len(data) < offset + 26 or data[offset : offset + 4] != b"PE\0\0":
            raise PackagingError(f"{label} has no PE signature")
        machine = struct.unpack_from("<H", data, offset + 4)[0]
        if machine != PE_MACHINE[target.arch]:
            raise PackagingError(f"{label} is PE machine {machine:#06x}, expected {PE_MACHINE[target.arch]:#06x}")
        if struct.unpack_from("<H", data, offset + 24)[0] != 0x20B:
            raise PackagingError(f"{label} is not PE32+ (64-bit)")
        lowered = data.lower()
        for dll in (b"vcruntime140.dll", b"api-ms-win-crt-"):
            if dll in lowered:
                raise PackagingError(
                    f"{label} imports the dynamic MSVC runtime ({dll.decode()}); build with "
                    "RUSTFLAGS='-C target-feature=+crt-static' (.cargo/config.toml) so it runs without a redistributable"
                )


def describe_binary(data: bytes, platform: str) -> str:
    """A one-line summary of an executable that verify_binary_bytes accepted (for release reports)."""
    target = TARGETS[platform]
    if target.os == "linux":
        e_type = struct.unpack_from("<H", data, 16)[0]
        kind = "static-pie" if e_type == 3 else "static"
        return f"ELF64 {target.arch} {kind}, no PT_INTERP"
    if target.os == "darwin":
        slices = _macho_slices(data)
        minos = [_macho_minos(thin) for cpu, thin in slices if cpu == MACHO_CPU[target.arch]]
        shown = f"{minos[0][0]}.{minos[0][1]}" if minos and minos[0] else "unknown"
        return f"Mach-O {target.arch} executable, minos {shown}"
    offset = struct.unpack_from("<I", data, 60)[0]
    machine = struct.unpack_from("<H", data, offset + 4)[0]
    return f"PE32+ machine {machine:#06x} ({target.arch}), no dynamic MSVC runtime imports"


def binary_source(platform: str, target_dir: Path | None, binaries_dir: Path | None) -> Path:
    target = TARGETS[platform]
    if binaries_dir is not None:
        return binaries_dir / platform / target.binary_name
    assert target_dir is not None
    return target_dir / target.triple / "release" / target.binary_name


# --------------------------------------------------------------------------- Peek.app.zip


@dataclasses.dataclass(frozen=True)
class AppZip:
    path: Path
    sha256: str
    bundle_id: str
    bundle_version: str
    short_version: str
    minimum_system_version: str


def _zip_read(archive: zipfile.ZipFile, name: str, label: str) -> bytes:
    info = archive.getinfo(name)
    if stat.S_ISLNK(info.external_attr >> 16):
        raise PackagingError(f"{label}: {name} is a symlink inside the zip; it must be a regular file")
    return archive.read(info)


def inspect_app_zip(path: Path, versions: Versions) -> AppZip:
    label = str(path)
    if path.is_symlink() or not path.is_file():
        raise PackagingError(
            f"{label} is missing or a symlink; build it with scripts/build-mac-release.sh (BLUEPRINT §4.4 steps 2-4)"
        )
    try:
        archive = zipfile.ZipFile(path)
    except zipfile.BadZipFile as error:
        raise PackagingError(f"{label} is not a zip archive: {error}") from error
    with archive:
        names = archive.namelist()
        for name in names:
            parts = PurePosixPath(name).parts
            if "__MACOSX" in parts or any(part.startswith("._") for part in parts):
                raise PackagingError(
                    f"{label}: entry {name!r} is AppleDouble metadata, which breaks the signature after "
                    "extraction; rebuild with ditto -c -k --norsrc --noextattr --noqtn --noacl --keepParent"
                )
            if name.startswith("/") or ".." in parts or not name.startswith("Peek.app/"):
                raise PackagingError(f"{label}: entry {name!r} is outside Peek.app/; zip with ditto --keepParent")
        missing = [entry for entry in ZIP_REQUIRED_ENTRIES if entry not in names]
        if missing:
            raise PackagingError(
                f"{label} lacks {', '.join(missing)}; the app must embed peekd and its agent (BLUEPRINT §8.1)"
            )
        try:
            info = plistlib.loads(_zip_read(archive, "Peek.app/Contents/Info.plist", label))
            agent = plistlib.loads(
                _zip_read(archive, "Peek.app/Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist", label)
            )
        except plistlib.InvalidFileException as error:
            raise PackagingError(f"{label}: unreadable property list: {error}") from error
        for binary in ("Peek.app/Contents/MacOS/Peek", "Peek.app/Contents/Helpers/peekd"):
            _check_macho(
                _zip_read(archive, binary, label),
                ("aarch64", "x86_64"),
                f"{label}:{binary}",
                APP_MACOS_DEPLOYMENT_TARGET,
            )
    values = {
        key: info.get(key)
        for key in (
            "CFBundleIdentifier",
            "CFBundleVersion",
            "CFBundleShortVersionString",
            "LSMinimumSystemVersion",
            "CFBundleExecutable",
        )
    }
    if not all(isinstance(value, str) and value for value in values.values()):
        raise PackagingError(f"{label}: Info.plist lacks one of {', '.join(values)}")
    if values["CFBundleExecutable"] != "Peek":
        raise PackagingError(f"{label}: CFBundleExecutable is {values['CFBundleExecutable']!r}, expected 'Peek'")
    if values["CFBundleIdentifier"] not in (BUNDLE_ID, DEV_BUNDLE_ID):
        raise PackagingError(
            f"{label}: CFBundleIdentifier {values['CFBundleIdentifier']!r} is neither {BUNDLE_ID} nor {DEV_BUNDLE_ID}"
        )
    if values["CFBundleShortVersionString"] != versions.version:
        raise PackagingError(
            f"{label}: CFBundleShortVersionString {values['CFBundleShortVersionString']} differs from "
            f"version {versions.version}; rebuild Peek.app from this tree"
        )
    if values["CFBundleVersion"] != str(versions.bundle_version):
        raise PackagingError(
            f"{label}: CFBundleVersion {values['CFBundleVersion']} must be {versions.bundle_version} for {versions.version}"
        )
    if values["LSMinimumSystemVersion"] != MINIMUM_SYSTEM_VERSION:
        raise PackagingError(
            f"{label}: LSMinimumSystemVersion is {values['LSMinimumSystemVersion']}, expected {MINIMUM_SYSTEM_VERSION} (D24)"
        )
    if agent.get("Label") != DAEMON_IDENTIFIER or agent.get("BundleProgram") != "Contents/Helpers/peekd":
        raise PackagingError(
            f"{label}: ai.tos.peek.daemon.plist must set Label {DAEMON_IDENTIFIER} and "
            "BundleProgram Contents/Helpers/peekd (BLUEPRINT §8.9)"
        )
    return AppZip(
        path=path,
        sha256=sha256_file(path),
        bundle_id=values["CFBundleIdentifier"],
        bundle_version=values["CFBundleVersion"],
        short_version=values["CFBundleShortVersionString"],
        minimum_system_version=values["LSMinimumSystemVersion"],
    )


def resolve_team_id(app: AppZip, override: str | None, build_info: Path | None) -> str:
    if override is not None:
        team = override
    elif build_info is not None and build_info.is_file():
        try:
            recorded: dict[str, Any] = json.loads(build_info.read_text(encoding="utf-8"))
        except json.JSONDecodeError as error:
            raise PackagingError(f"{build_info} is not JSON: {error}") from error
        if recorded.get("zip_sha256") not in (None, app.sha256):
            raise PackagingError(
                f"{build_info} describes a different Peek.app.zip (sha256 {recorded.get('zip_sha256')}, "
                f"found {app.sha256}); rerun scripts/build-mac-release.sh"
            )
        team = str(recorded.get("team_id", ""))
    else:
        team = TEAM_ID if app.bundle_id == BUNDLE_ID else ""
    if app.bundle_id == BUNDLE_ID and team != TEAM_ID:
        raise PackagingError(
            f"a {BUNDLE_ID} build must carry team_id={TEAM_ID} (Developer ID); got {team!r}. "
            f"Development builds use bundle id {DEV_BUNDLE_ID} and an empty team_id (BLUEPRINT §5.5)"
        )
    if app.bundle_id == DEV_BUNDLE_ID and team:
        raise PackagingError(
            f"a {DEV_BUNDLE_ID} build must leave team_id empty so ensure_app() only checks the "
            f"signature (BLUEPRINT §4.1); got {team!r}"
        )
    return team


def developer_id_requirement(bundle_id: str, team: str) -> str:
    return (
        f'anchor apple generic and identifier "{bundle_id}" and certificate 1[field.1.2.840.113635.100.6.2.6] '
        f'and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = "{team}"'
    )


def _codesign_details(path: Path) -> dict[str, str]:
    result = subprocess.run(
        ["/usr/bin/codesign", "-d", "--verbose=4", str(path)], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise PackagingError(f"codesign cannot read the signature of {path}: {result.stderr.strip()}")
    details: dict[str, str] = {}
    for line in result.stderr.splitlines():
        key, _, value = line.partition("=")
        if value:
            details.setdefault(key.strip(), value.strip())
    return details


def verify_app_signature(app: AppZip, team: str) -> None:
    """Round-trip the zip exactly as ensure_app() does and check the signatures (BLUEPRINT §1.8)."""
    if sys.platform != "darwin" or not Path("/usr/bin/codesign").exists():
        raise PackagingError(
            "signature verification needs macOS codesign; run on a Mac or pass --skip-signature-check "
            "(only after scripts/build-mac-release.sh verified the same zip)"
        )
    with tempfile.TemporaryDirectory(prefix="peek-app-verify-") as scratch:
        extract = subprocess.run(
            ["/usr/bin/ditto", "-x", "-k", str(app.path), scratch], capture_output=True, text=True, check=False
        )
        if extract.returncode != 0:
            raise PackagingError(f"ditto -x -k could not unpack {app.path}: {extract.stderr.strip()}")
        bundle = Path(scratch) / "Peek.app"
        command = ["/usr/bin/codesign", "--verify", "--deep", "--strict", "--verbose=2"]
        if team:
            command.append(f"-R={developer_id_requirement(app.bundle_id, team)}")
        verified = subprocess.run([*command, str(bundle)], capture_output=True, text=True, check=False)
        if verified.returncode != 0:
            raise PackagingError(
                f"the unpacked Peek.app fails {' '.join(command[1:4])}"
                f"{' with the Developer ID requirement for team ' + team if team else ''}: "
                f"{verified.stderr.strip()}"
            )
        outer = _codesign_details(bundle)
        helper = _codesign_details(bundle / "Contents/Helpers/peekd")
        if outer.get("Identifier") != app.bundle_id:
            raise PackagingError(f"Peek.app is signed as {outer.get('Identifier')!r}, expected {app.bundle_id}")
        if outer.get("TeamIdentifier") in (None, "not set"):
            raise PackagingError(
                "Peek.app is ad-hoc signed; never ship ad-hoc builds (TCC grants reset on every build, BLUEPRINT §8.10)"
            )
        if outer.get("TeamIdentifier") != TEAM_ID:
            raise PackagingError(f"Peek.app is signed by team {outer.get('TeamIdentifier')}, expected {TEAM_ID}")
        if helper.get("Identifier") != DAEMON_IDENTIFIER:
            raise PackagingError(
                f"Contents/Helpers/peekd is signed as {helper.get('Identifier')!r}; sign it with -i {DAEMON_IDENTIFIER}"
            )
        for name, details in (("Peek.app", outer), ("peekd", helper)):
            flags = re.search(r"flags=0x[0-9a-f]+\(([^)]*)\)", details.get("CodeDirectory v", ""))
            if flags is None or "runtime" not in flags.group(1).split(","):
                raise PackagingError(f"{name} is not signed with the hardened runtime (--options runtime)")
        xattrs = subprocess.run(["/usr/bin/xattr", "-lr", str(bundle)], capture_output=True, text=True, check=False)
        if "com.apple.quarantine" in xattrs.stdout:
            raise PackagingError(
                "the zip restores com.apple.quarantine on extraction; rebuild it with ditto --noqtn --noextattr"
            )


def peek_app_info(app: AppZip, team: str) -> str:
    """Exactly the six keys of BLUEPRINT §4.1, parsed by `sed -n s/^key=//p`."""
    return (
        f"bundle_id={app.bundle_id}\n"
        f"bundle_version={app.bundle_version}\n"
        f"short_version={app.short_version}\n"
        f"team_id={team}\n"
        f"zip_sha256={app.sha256}\n"
        f"minimum_system_version={app.minimum_system_version}\n"
    )


# --------------------------------------------------------------------------- stage


def expected_files() -> dict[str, int]:
    """Every staged path and its mode."""
    files = {"apps.yaml": 0o644}
    for platform, target in TARGETS.items():
        base = f"targets/{platform}"
        files[f"{base}/{target.executable}"] = 0o755
        files[f"{base}/LICENSE"] = 0o644
        if platform.startswith("macos-"):
            files[f"{base}/Peek.app.zip"] = 0o644
            files[f"{base}/Peek.app.info"] = 0o644
            files[f"{base}/install-app.sh"] = 0o755
    return files


def assert_clean_tree(stage: Path) -> None:
    """Only directories and single-link regular files; exactly the expected set."""
    expected = expected_files()
    found: set[str] = set()
    for current, directories, names in os.walk(stage, followlinks=False):
        for name in [*directories, *names]:
            path = Path(current) / name
            relative = path.relative_to(stage).as_posix()
            info = path.lstat()
            if stat.S_ISLNK(info.st_mode):
                raise PackagingError(f"{relative} is a symlink; Silicon Apps rejects links anywhere in a package")
            if stat.S_ISDIR(info.st_mode):
                continue
            if not stat.S_ISREG(info.st_mode):
                raise PackagingError(f"{relative} is a special file; only regular files and directories may ship")
            if info.st_nlink != 1:
                raise PackagingError(f"{relative} is hard-linked ({info.st_nlink} links); Silicon Apps rejects hard links")
            found.add(relative)
    if found != set(expected):
        extra, missing = sorted(found - set(expected)), sorted(set(expected) - found)
        raise PackagingError(
            f"stage layout differs from Manifest A: unexpected {extra or 'none'}, missing {missing or 'none'}"
        )
    for relative, mode in expected.items():
        actual = (stage / relative).stat().st_mode & 0o777
        if actual != mode:
            raise PackagingError(f"{relative} has mode {actual:o}, expected {mode:o}")


def _prepare_out(out: Path, replace: bool) -> None:
    if out.exists() or out.is_symlink():
        if not replace:
            raise PackagingError(f"{out} already exists; pass --replace to rebuild the stage")
        if out.is_symlink() or not out.is_dir():
            raise PackagingError(f"{out} is not a directory; refusing to replace it")
        unexpected = sorted(p.name for p in out.iterdir() if p.name not in ("apps.yaml", "targets"))
        if unexpected:
            raise PackagingError(
                f"refusing to delete {out}: it contains {', '.join(unexpected)}, which this script never writes"
            )
        shutil.rmtree(out)
    out.mkdir(parents=True, mode=0o755)


def _copy(source: Path, destination: Path, mode: int) -> None:
    if source.is_symlink():
        raise PackagingError(f"{source} is a symlink; pass the real file")
    destination.parent.mkdir(parents=True, exist_ok=True, mode=0o755)
    shutil.copyfile(source, destination)  # new inode: never a hard link into the build tree
    destination.chmod(mode)


def isolated_apps_env(home: Path) -> dict[str, str]:
    env = {
        key: value for key, value in os.environ.items() if not key.startswith("APPS_") and key != "SILICON_HOME"
    }
    env.update(APPS_ENV)
    env["SILICON_HOME"] = str(home)
    return env


def apps_validate(apps: str, path: Path) -> dict[str, Any]:
    executable = shutil.which(apps)
    if executable is None:
        raise PackagingError(
            f"Silicon Apps CLI {apps!r} not found; install Silicon Apps >= 0.5.0 or pass --no-validate "
            "for a layout-only check (the release build always validates)"
        )
    with tempfile.TemporaryDirectory(prefix="peek-apps-home-") as home:
        result = subprocess.run(
            [executable, "--json", "validate", str(path)],
            capture_output=True,
            text=True,
            env=isolated_apps_env(Path(home)),
            stdin=subprocess.DEVNULL,
            check=False,
        )
    try:
        report = json.loads(result.stdout)
    except json.JSONDecodeError:
        raise PackagingError(
            f"silicon-apps validate {path} printed no JSON (exit {result.returncode}): {result.stderr.strip()}"
        ) from None
    if not report.get("valid"):
        errors = "\n  ".join(report.get("errors") or [result.stderr.strip()])
        raise PackagingError(f"silicon-apps validate {path} failed:\n  {errors}")
    return report


def stage(args: argparse.Namespace) -> dict[str, Any]:
    root: Path = args.root
    versions = repo_versions(root, args.version)
    log(
        f"version {versions.version} (CFBundleVersion {versions.bundle_version}) is consistent across {len(versions.sources)} sources"
    )
    sources = {platform: binary_source(platform, args.target_dir, args.binaries_dir) for platform in TARGETS}
    for platform, source in sources.items():
        verify_binary(source, platform)
    log("all six CLI binaries have the right format and architecture")
    app = inspect_app_zip(args.app_zip, versions)
    team = resolve_team_id(app, args.team_id, args.app_build_info)
    if args.skip_signature_check:
        log("signature round-trip skipped (--skip-signature-check)")
    else:
        verify_app_signature(app, team)
        log(f"Peek.app.zip unpacks with ditto -x -k and verifies as {app.bundle_id} (team {TEAM_ID})")
    for required in ("LICENSE", "scripts/install-app.sh"):
        path = root / required
        if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
            raise PackagingError(f"{rel(path, root)} is missing, empty or a symlink")
    if not (root / "scripts/install-app.sh").read_bytes().startswith(b"#!/bin/sh\n"):
        raise PackagingError("scripts/install-app.sh must start with #!/bin/sh (it runs with PATH removed)")

    out: Path = args.out
    _prepare_out(out, args.replace)
    _copy(root / "apps.yaml", out / "apps.yaml", 0o644)
    info_text = peek_app_info(app, team)
    for platform, target in TARGETS.items():
        base = out / "targets" / platform
        _copy(sources[platform], base / target.executable, 0o755)
        _copy(root / "LICENSE", base / "LICENSE", 0o644)
        if platform.startswith("macos-"):
            _copy(app.path, base / "Peek.app.zip", 0o644)
            (base / "Peek.app.info").write_text(info_text, encoding="utf-8")
            (base / "Peek.app.info").chmod(0o644)
            _copy(root / "scripts/install-app.sh", base / "install-app.sh", 0o755)
    for directory in [out, *[p for p in out.rglob("*") if p.is_dir()]]:
        directory.chmod(0o755)
    assert_clean_tree(out)
    checksums = out.parent / f"peek-{versions.version}-stage.sha256"
    lines = [f"{sha256_file(out / relative)}  {relative}" for relative in sorted(expected_files())]
    checksums.write_text("\n".join(lines) + "\n", encoding="utf-8")
    validation = "skipped"
    if not args.no_validate:
        apps_validate(args.apps, out)
        validation = "passed"
        log(f"silicon-apps validate {out}: valid")
    return {
        "stage": str(out),
        "version": versions.version,
        "bundle_version": versions.bundle_version,
        "bundle_id": app.bundle_id,
        "team_id": team,
        "zip_sha256": app.sha256,
        "files": len(expected_files()),
        "checksums": str(checksums),
        "signature_check": "skipped" if args.skip_signature_check else "passed",
        "apps_validate": validation,
    }


def verify_archive(archive_path: Path, stage_dir: Path | None, root: Path) -> dict[str, Any]:
    """Check a packed archive: only the expected regular files and directories, right modes, same bytes."""
    if not archive_path.is_file():
        raise PackagingError(f"{archive_path} does not exist; run silicon-apps pack first")
    expected = expected_files()
    seen: dict[str, str] = {}
    binaries: dict[str, str] = {}
    with tarfile.open(archive_path, "r:gz") as package:
        for member in package.getmembers():
            name = member.name.removeprefix("./").rstrip("/")
            if not name or name == ".":
                continue
            if member.issym() or member.islnk() or not (member.isfile() or member.isdir()):
                raise PackagingError(f"{archive_path}: {name} is a link or special file")
            if PurePosixPath(name).is_absolute() or ".." in PurePosixPath(name).parts:
                raise PackagingError(f"{archive_path}: unsafe member path {name!r}")
            if member.isdir():
                continue
            if name not in expected:
                raise PackagingError(f"{archive_path}: unexpected member {name}")
            if expected[name] & 0o111 and not member.mode & 0o111:
                raise PackagingError(f"{archive_path}: {name} lost its executable bit")
            handle = package.extractfile(member)
            assert handle is not None
            data = handle.read()
            seen[name] = hashlib.sha256(data).hexdigest()
            platform = name.split("/")[1] if name.startswith("targets/") else ""
            if platform in TARGETS and name == f"targets/{platform}/{TARGETS[platform].executable}":
                # The archive's own bytes, not just the stage's: format, CPU, static linking, CRT.
                verify_binary_bytes(data, platform, f"{archive_path}:{name}")
                binaries[platform] = describe_binary(data, platform)
    missing = sorted(set(expected) - set(seen))
    if missing:
        raise PackagingError(f"{archive_path} is missing {', '.join(missing)}")
    if seen["apps.yaml"] != sha256_file(root / "apps.yaml"):
        raise PackagingError(f"{archive_path}: apps.yaml differs from the repository manifest")
    zips = {seen[f"targets/{p}/Peek.app.zip"] for p in TARGETS if p.startswith("macos-")}
    if len(zips) != 1:
        raise PackagingError(f"{archive_path}: the two macOS targets carry different Peek.app.zip bytes")
    if stage_dir is not None:
        for name, digest in seen.items():
            if sha256_file(stage_dir / name) != digest:
                raise PackagingError(f"{archive_path}: {name} differs from {stage_dir / name}")
    return {
        "archive": str(archive_path),
        "sha256": sha256_file(archive_path),
        "files": len(seen),
        "binaries": binaries,
        "verified": True,
    }


# --------------------------------------------------------------------------- CLI


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        epilog="Example: python3 scripts/package-apps.py --version 0.1.0 --out dist/stage",
    )
    parser.add_argument("--root", type=Path, default=ROOT, help="repository root (default: this checkout)")
    parser.add_argument("--version", help="expected release version; must equal every version source")
    parser.add_argument("--out", type=Path, help="stage directory to create (e.g. dist/stage)")
    parser.add_argument("--replace", action="store_true", help="replace an existing stage directory")
    inputs = parser.add_mutually_exclusive_group()
    inputs.add_argument(
        "--target-dir", type=Path, help="cargo target dir (default: $CARGO_TARGET_DIR or <root>/target)"
    )
    inputs.add_argument("--binaries-dir", type=Path, help="CI layout: <apps-target>/peek[.exe]")
    parser.add_argument("--app-zip", type=Path, help="Peek.app.zip (default: <root>/dist/Peek.app.zip)")
    parser.add_argument(
        "--app-build-info", type=Path, help="Peek.app.build.json from build-mac-release.sh (default: next to the zip)"
    )
    parser.add_argument("--team-id", help="override the Peek.app.info team_id ('' for development builds)")
    parser.add_argument(
        "--skip-signature-check", action="store_true", help="skip the codesign round trip (non-macOS hosts)"
    )
    parser.add_argument(
        "--apps", default=os.environ.get("APPS", "silicon-apps"), help="Silicon Apps CLI (default: silicon-apps)"
    )
    parser.add_argument("--no-validate", action="store_true", help="skip silicon-apps validate on the stage")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--check-versions", action="store_true", help="only check version consistency and print it")
    modes.add_argument("--check-binaries", action="store_true", help="only verify the six CLI binaries")
    modes.add_argument(
        "--verify-archive", type=Path, metavar="ARCHIVE", help="verify a packed archive (against --out if given)"
    )
    args = parser.parse_args(argv)
    args.root = args.root.resolve()
    if args.target_dir is None and args.binaries_dir is None:
        args.target_dir = Path(os.environ.get("CARGO_TARGET_DIR") or args.root / "target")
    if args.app_zip is None:
        args.app_zip = args.root / "dist" / "Peek.app.zip"
    if args.app_build_info is None:
        args.app_build_info = args.app_zip.with_name("Peek.app.build.json")
    if not (args.check_versions or args.check_binaries or args.verify_archive) and args.out is None:
        parser.error("--out is required to stage a package (e.g. --out dist/stage)")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        if args.check_versions:
            versions = repo_versions(args.root, args.version)
            result: dict[str, Any] = {
                "version": versions.version,
                "bundle_version": versions.bundle_version,
                "tag": f"v{versions.version}",
                "sources": versions.sources,
            }
        elif args.check_binaries:
            checked = {}
            for platform in TARGETS:
                source = binary_source(platform, args.target_dir, args.binaries_dir)
                verify_binary(source, platform)
                checked[platform] = {"path": str(source), "sha256": sha256_file(source)}
            result = {"binaries": checked, "verified": True}
        elif args.verify_archive:
            stage_dir = args.out if args.out is not None and args.out.is_dir() else None
            result = verify_archive(args.verify_archive, stage_dir, args.root)
        else:
            result = stage(args)
    except PackagingError as error:
        print(f"package-apps: error: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
