"""Synthetic executables, Peek.app zips and repository skeletons for the packaging tests.

Nothing here is runnable code: the executables carry only valid headers, which is all
scripts/package-honeycomb.py inspects. Real builds come from scripts/build-*-release.sh.
"""

from __future__ import annotations

import plistlib
import re
import shutil
import struct
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CPU = {"aarch64": 0x0100000C, "x86_64": 0x01000007}


def elf(arch: str, *, interpreter: bool = False) -> bytes:
    machine = {"aarch64": 183, "x86_64": 62}[arch]
    ident = b"\x7fELF" + bytes([2, 1, 1, 0]) + bytes(8)
    header = ident + struct.pack("<HHIQQQIHHHHHH", 2, machine, 1, 0, 64, 0, 0, 64, 56, 1, 0, 0, 0)
    program = struct.pack("<IIQQQQQQ", 3 if interpreter else 1, 5, 0, 0, 0, 0, 0, 0x1000)
    return header + program + b"\0" * 64


def macho(arch: str, minos: tuple[int, int] = (11, 0)) -> bytes:
    build_version = struct.pack("<IIIIII", 0x32, 24, 1, (minos[0] << 16) | (minos[1] << 8), 26 << 16, 0)
    header = struct.pack("<IIIIIIII", 0xFEEDFACF, CPU[arch], 0, 2, 1, len(build_version), 0, 0)
    return header + build_version + b"\0" * 64


def fat(*slices: bytes) -> bytes:
    align = 0x1000
    header = struct.pack(">II", 0xCAFEBABE, len(slices))
    body = b""
    offset = align
    entries = b""
    for thin in slices:
        cpu = struct.unpack_from("<I", thin, 4)[0]
        entries += struct.pack(">IIIII", cpu, 0, offset, len(thin), 12)
        padded = thin + b"\0" * (-len(thin) % align)
        body += padded
        offset += len(padded)
    head = header + entries
    return head + b"\0" * (align - len(head)) + body


def pe(arch: str, *, dynamic_crt: bool = False) -> bytes:
    machine = {"aarch64": 0xAA64, "x86_64": 0x8664}[arch]
    dos = b"MZ" + bytes(58) + struct.pack("<I", 64)
    coff = b"PE\0\0" + struct.pack("<HHIIIHH", machine, 0, 0, 0, 0, 240, 0x22)
    optional = struct.pack("<H", 0x20B) + bytes(238)
    trailer = b"VCRUNTIME140.dll\0" if dynamic_crt else b"KERNEL32.dll\0"
    return dos + coff + optional + trailer


def cli_binary(platform: str) -> bytes:
    os_name, arch = platform.split("-")
    if os_name == "linux":
        return elf(arch)
    if os_name == "macos":
        return macho(arch)
    return pe(arch)


def write_binaries(directory: Path) -> None:
    """CI layout: <directory>/<honeycomb-target>/peek[.exe]."""
    for platform in (
        "macos-aarch64",
        "macos-x86_64",
        "linux-x86_64",
        "linux-aarch64",
        "windows-x86_64",
        "windows-aarch64",
    ):
        name = "peek.exe" if platform.startswith("windows-") else "peek"
        path = directory / platform / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(cli_binary(platform))
        path.chmod(0o755)


def write_app_zip(
    path: Path,
    *,
    bundle_id: str = "ai.tos.peek.dev",
    version: str = "0.1.0",
    build: str = "1000",
    minimum: str = "26.0",
    apple_double: bool = False,
    helper_arches: tuple[str, ...] = ("aarch64", "x86_64"),
) -> Path:
    info = {
        "CFBundleIdentifier": bundle_id,
        "CFBundleExecutable": "Peek",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": build,
        "LSMinimumSystemVersion": minimum,
    }
    agent = {
        "Label": "ai.tos.peek.daemon",
        "BundleProgram": "Contents/Helpers/peekd",
        "ProgramArguments": ["peekd", "run", "--launchd"],
    }
    universal = fat(macho("aarch64", (26, 0)), macho("x86_64", (26, 0)))
    helper = fat(*(macho(arch, (26, 0)) for arch in helper_arches))
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("Peek.app/", b"")
        archive.writestr("Peek.app/Contents/Info.plist", plistlib.dumps(info, fmt=plistlib.FMT_BINARY))
        archive.writestr("Peek.app/Contents/MacOS/Peek", universal)
        archive.writestr("Peek.app/Contents/Helpers/peekd", helper)
        archive.writestr("Peek.app/Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist", plistlib.dumps(agent))
        archive.writestr("Peek.app/Contents/_CodeSignature/CodeResources", b"<plist/>")
        if apple_double:
            archive.writestr("__MACOSX/Peek.app/._Info.plist", b"\0\5\26\7")
    return path


CARGO_ROOT = """[workspace]
members = [".", "crates/client", "crates/cli", "crates/daemon"]
resolver = "3"

[workspace.package]
version = "{version}"
edition = "2024"

[package]
name = "silicon-peek"
version.workspace = true
"""
CARGO_MEMBER = """[package]
name = "{name}"
version = "{version}"
"""
PROJECT_YML = """name: Peek
settings:
  base:
    MARKETING_VERSION: "{version}"
    CURRENT_PROJECT_VERSION: "{build}"
"""


def write_repo(root: Path, *, version: str = "0.1.0", cli_version: str | None = None, build: str = "1000") -> Path:
    """A repository skeleton carrying every version source package-honeycomb.py reads."""
    root.mkdir(parents=True, exist_ok=True)
    (root / "Cargo.toml").write_text(CARGO_ROOT.format(version=version))
    for directory, name in (
        ("client", "silicon-peek-client"),
        ("cli", "silicon-peek-cli"),
        ("daemon", "silicon-peek-daemon"),
    ):
        member = root / "crates" / directory
        member.mkdir(parents=True, exist_ok=True)
        member_version = cli_version if (directory == "cli" and cli_version) else version
        (member / "Cargo.toml").write_text(CARGO_MEMBER.format(name=name, version=member_version))
    # Whatever the real manifest's version is, the fixture carries `version`.
    manifest = re.sub(r'(?m)^version: "[^"]*"', f'version: "{version}"', (REPO / "honeycomb.yaml").read_text(), count=1)
    (root / "honeycomb.yaml").write_text(manifest)
    (root / "apps/mac").mkdir(parents=True, exist_ok=True)
    (root / "apps/mac/project.yml").write_text(PROJECT_YML.format(version=version, build=build))
    shutil.copyfile(REPO / "LICENSE", root / "LICENSE")
    (root / "scripts").mkdir(exist_ok=True)
    shutil.copyfile(REPO / "scripts/install-app.sh", root / "scripts/install-app.sh")
    (root / "scripts/install-app.sh").chmod(0o755)
    return root
