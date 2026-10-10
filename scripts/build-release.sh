#!/usr/bin/env bash
# Build native clients and the macOS app, then pack the Silicon Apps release.
set -euo pipefail
ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$ROOT"
APPS=${APPS:-silicon-apps}
skip_build=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) skip_build=1 ;;
    *) echo "Usage: scripts/build-release.sh [--skip-build]" >&2; exit 2 ;;
  esac
done
if (( ! skip_build )); then
  . scripts/lib/signing.sh
  resolve_signing_identity
  scripts/build-cli-release.sh
  scripts/build-mac-release.sh
fi
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
python3 scripts/package-apps.py --out dist/stage --replace --apps "$APPS"
"$APPS" pack dist/stage --output "dist/peek-$version.tar.gz" --json
(cd dist && shasum -a 256 "peek-$version.tar.gz" > "peek-$version.tar.gz.sha256")
echo "Built dist/peek-$version.tar.gz. Publish with the Silicon Apps CLI; see deploy/README.md."
