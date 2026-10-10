#!/usr/bin/env python3
"""Online, integrity-checked snapshots of peek-server's SQLite databases to encrypted S3.

Run hourly by peek-backup.timer as the peek user (BLUEPRINT §5.2). For each database in
/var/lib/peek (peek.sqlite):

  1. copy it with SQLite's online backup API (the equivalent of `sqlite3 .backup`), which is
     consistent while peek-server keeps writing in WAL mode;
  2. run PRAGMA integrity_check on the copy and refuse to upload anything but "ok";
  3. upload with `aws s3 cp --sse AES256` to s3://$PEEK_BACKUP_BUCKET/backups/YYYY/MM/DD/HHMMSSZ/<name>;
     the bucket's lifecycle rule expires backups/ after 7 days.

Missing databases are skipped (a fresh host has none until peek-server first starts). --local-dir
writes the verified snapshots to a directory instead of S3, for restore drills and tests.
Runs on the host's /usr/bin/python3 (Amazon Linux 2023: Python 3.9), so it stays 3.9-compatible.
Restore: stop peek-server, copy the snapshot over /var/lib/peek/<name> (owner peek, mode 0600),
delete any <name>-wal/-shm left beside it, start peek-server.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from contextlib import closing
from pathlib import Path

DATABASES = ("peek.sqlite",)


class BackupError(Exception):
    pass


def snapshot(source: Path, destination: Path) -> str:
    """Consistent copy of source into destination; returns its sha256 after an integrity check."""
    try:
        # Nested on purpose: parenthesized context managers need Python 3.10; the host runs 3.9.
        with closing(sqlite3.connect(f"file:{source}?mode=ro", uri=True, timeout=30)) as db:  # noqa: SIM117
            with closing(sqlite3.connect(destination)) as target:
                db.backup(target, pages=1024, sleep=0.05)
        with closing(sqlite3.connect(destination)) as check:
            rows = check.execute("PRAGMA integrity_check").fetchall()
    except sqlite3.Error as error:
        raise BackupError(f"cannot snapshot {source}: {error}") from error
    if rows != [("ok",)]:
        raise BackupError(f"snapshot of {source} failed PRAGMA integrity_check: {rows[:5]}; nothing was uploaded")
    destination.chmod(0o600)
    digest = hashlib.sha256()
    with destination.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def upload(aws: str, path: Path, bucket: str, key: str, region: str, digest: str) -> None:
    command = [
        aws,
        "s3",
        "cp",
        str(path),
        f"s3://{bucket}/{key}",
        "--region",
        region,
        "--sse",
        "AES256",
        "--only-show-errors",
        "--metadata",
        f"sha256={digest}",
    ]
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise BackupError(
            f"aws s3 cp to s3://{bucket}/{key} failed (exit {result.returncode}): {result.stderr.strip()}; "
            "check the instance role allows s3:PutObject on backups/* (deploy/stack.yaml)"
        )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--data-dir", type=Path, default=Path("/var/lib/peek"))
    parser.add_argument("--bucket", default=os.environ.get("PEEK_BACKUP_BUCKET", ""))
    parser.add_argument("--region", default=os.environ.get("PEEK_BACKUP_REGION", "us-east-1"))
    parser.add_argument("--aws", default=shutil.which("aws") or "/usr/bin/aws")
    parser.add_argument("--local-dir", type=Path, help="write verified snapshots here instead of uploading")
    args = parser.parse_args(argv)
    if args.local_dir is None and not args.bucket:
        print(
            "peek-backup: error: PEEK_BACKUP_BUCKET is not set; deploy/install.sh writes it to /etc/peek/backup.env "
            "from the stack's ArtifactBucket output",
            file=sys.stderr,
        )
        return 1
    now = datetime.datetime.now(datetime.timezone.utc)  # noqa: UP017 (the host runs Python 3.9)
    stamp = now.strftime("%Y/%m/%d/%H%M%SZ")
    done = []
    try:
        with tempfile.TemporaryDirectory(prefix="peek-backup-") as scratch:
            for name in DATABASES:
                source = args.data_dir / name
                if not source.is_file():
                    continue
                copy = Path(scratch) / name
                digest = snapshot(source, copy)
                key = f"backups/{stamp}/{name}"
                if args.local_dir is not None:
                    target = args.local_dir / stamp / name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(copy, target)
                    target.chmod(0o600)
                    location = str(target)
                else:
                    upload(args.aws, copy, args.bucket, key, args.region, digest)
                    location = f"s3://{args.bucket}/{key}"
                done.append({"database": name, "bytes": copy.stat().st_size, "sha256": digest, "location": location})
    except BackupError as error:
        print(f"peek-backup: error: {error}", file=sys.stderr)
        return 1
    if not done:
        print(json.dumps({"backed_up": [], "note": f"no databases in {args.data_dir} yet"}))
        return 0
    print(json.dumps({"backed_up": done}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
