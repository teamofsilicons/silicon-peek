#!/usr/bin/env python3
"""Mirror the canonical manuals in docs/ into crates/cli/docs/ for `peek docs`.

The CLI embeds crates/cli/docs/**/*.md with include_str!, so the crate stays publishable on
its own (BLUEPRINT §7.6). docs/ is the only place to edit; this copy is generated.

  python3 scripts/sync-cli-docs.py           copy changed manuals, delete stale copies
  python3 scripts/sync-cli-docs.py --check   exit 1 if the copy is out of date (CI)

Only Markdown is managed: other files under crates/cli/docs/ are left alone. Manuals must be
UTF-8 regular files (include_str! rejects anything else); symlinks are refused.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class SyncError(Exception):
    pass


def manuals(directory: Path) -> dict[str, bytes]:
    """Relative POSIX path -> bytes for every Markdown file below directory."""
    found: dict[str, bytes] = {}
    if not directory.exists():
        return found
    for path in sorted(directory.rglob("*.md")):
        relative = path.relative_to(directory).as_posix()
        parents = [directory / parent for parent in path.relative_to(directory).parents if str(parent) != "."]
        if path.is_symlink() or any(parent.is_symlink() for parent in parents):
            raise SyncError(f"{path} is (or sits under) a symlink; manuals must be regular files")
        if not path.is_file():
            continue
        data = path.read_bytes()
        try:
            data.decode("utf-8")
        except UnicodeDecodeError as error:
            raise SyncError(f"{path} is not UTF-8 ({error}); include_str! needs UTF-8") from error
        found[relative] = data
    return found


def plan(root: Path) -> tuple[dict[str, bytes], list[str], list[str], list[str]]:
    source_dir, destination_dir = root / "docs", root / "crates" / "cli" / "docs"
    if not source_dir.is_dir():
        raise SyncError(f"{source_dir} does not exist; the canonical manuals live in docs/*.md (BLUEPRINT §1.3)")
    if destination_dir.is_symlink():
        raise SyncError(f"{destination_dir} is a symlink; it must be a real directory inside the crate")
    source = manuals(source_dir)
    if not source:
        raise SyncError(f"{source_dir} contains no Markdown manuals")
    copied = manuals(destination_dir)
    added = sorted(name for name in source if name not in copied)
    changed = sorted(name for name in source if name in copied and copied[name] != source[name])
    stale = sorted(name for name in copied if name not in source)
    return source, added, changed, stale


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="verify only; exit 1 when crates/cli/docs is stale")
    parser.add_argument("--root", type=Path, default=ROOT, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    root = args.root.resolve()
    try:
        source, added, changed, stale = plan(root)
    except SyncError as error:
        print(f"sync-cli-docs: error: {error}", file=sys.stderr)
        return 1
    destination = root / "crates" / "cli" / "docs"
    if args.check:
        if added or changed or stale:
            for label, names in (("missing", added), ("out of date", changed), ("stale (no docs/ source)", stale)):
                for name in names:
                    print(f"sync-cli-docs: {label}: crates/cli/docs/{name}", file=sys.stderr)
            print("fix: python3 scripts/sync-cli-docs.py, then commit crates/cli/docs", file=sys.stderr)
            return 1
        print(f"sync-cli-docs: crates/cli/docs matches docs/ ({len(source)} manuals)")
        return 0
    for name in [*added, *changed]:
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        temporary = target.with_name(f".{target.name}.sync-tmp")
        temporary.write_bytes(source[name])
        temporary.replace(target)
    for name in stale:
        (destination / name).unlink()
    for directory in sorted({p for p in destination.rglob("*") if p.is_dir()}, reverse=True):
        if not any(directory.iterdir()):
            directory.rmdir()
    print(
        f"sync-cli-docs: {len(source)} manuals in sync "
        f"({len(added)} added, {len(changed)} updated, {len(stale)} removed)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
