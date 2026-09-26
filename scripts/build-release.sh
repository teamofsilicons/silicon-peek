#!/usr/bin/env bash
# Build a complete, validated peek release candidate locally (BLUEPRINT §4.4):
#   1. scripts/build-cli-release.sh     six CLI targets
#   2. scripts/build-mac-release.sh     universal peekd + Peek.app, signed inside-out, optional
#                                       notarization, zip hygiene, round trip → dist/Peek.app.zip
#   3. scripts/package-honeycomb.py     stage dist/stage (Manifest B) with every payload checked
#   4. honeycomb validate → pack → validate the archive, then an archive/stage byte comparison
#   5. scripts/loopback-install-test.py install the archive against a loopback fake API (§4.4 step 6)
# Output: dist/peek-<version>.tar.gz (+ .sha256). Nothing is uploaded: publishing is the separate,
# approved operator procedure in deploy/README.md (P10, BLUEPRINT §4.5).
#
# Usage: scripts/build-release.sh [--version X.Y.Z] [--skip-build] [--skip-loopback] [--replace]
#   --version        fail unless the repository is at exactly this version
#   --skip-build     reuse existing target/ and dist/Peek.app.zip (steps 3-5 only)
#   --skip-loopback  skip step 5
#   --replace        overwrite an existing dist/peek-<version>.tar.gz (release archives are immutable
#                    once uploaded; replace only candidates that never left this machine)
# Environment: SIGN_IDENTITY, PEEK_SIGN_TIMESTAMP, NOTARY_PROFILE, NOTARY_KEYCHAIN, PEEK_TEAM_ID,
#              PEEK_PREBUILT_APP (see build-mac-release.sh), DEVELOPER_DIR, HONEYCOMB (Honeycomb CLI,
#              default `honeycomb`), CARGO_TARGET_DIR.
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$ROOT"
DIST=$ROOT/dist
HONEYCOMB=${HONEYCOMB:-honeycomb}
expected="" skip_build=0 skip_loopback=0 replace=0

die() {
    printf 'build-release: error: %s\n' "$1" >&2
    [[ -z ${2:-} ]] || printf 'fix: %s\n' "$2" >&2
    exit 1
}
step() { printf '\n######## %s\n' "$*" >&2; }

while (($#)); do
    case $1 in
        --version) [[ $# -ge 2 ]] || die "--version needs a value" "e.g. --version 0.1.0"; expected=$2; shift 2 ;;
        --skip-build) skip_build=1; shift ;;
        --skip-loopback) skip_loopback=1; shift ;;
        --replace) replace=1; shift ;;
        -h | --help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument $1" "run scripts/build-release.sh --help" ;;
    esac
done

command -v "$HONEYCOMB" >/dev/null 2>&1 || die "Honeycomb CLI '$HONEYCOMB' not found" \
    "install Honeycomb >= 0.5.0 or set HONEYCOMB=/path/to/honeycomb"
HONEYCOMB=$(command -v "$HONEYCOMB")

version_args=()
[[ -z $expected ]] || version_args=(--version "$expected")
versions=$(python3 "$ROOT/scripts/package-honeycomb.py" --check-versions ${version_args[@]+"${version_args[@]}"})
V=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])' <<<"$versions")
ARCHIVE=$DIST/peek-$V.tar.gz
if [[ -e $ARCHIVE ]]; then
    ((replace)) || die "$ARCHIVE already exists" \
        "each version is immutable per channel; bump the version, or pass --replace for a candidate that was never uploaded"
    rm -f "$ARCHIVE" "$ARCHIVE.sha256"
fi
mkdir -p "$DIST"

# Honeycomb gets a private scratch home: validate and pack are local-only and must not touch the
# operator's ~/.honeycomb (no package-update pass, no updater service, no telemetry).
HC_HOME=$(mktemp -d "${TMPDIR:-/tmp}/peek-release-honeycomb.XXXXXX")
trap 'rm -rf "$HC_HOME"' EXIT
hc() {
    env SILICON_HOME="$HC_HOME" HONEYCOMB_AUTO_UPDATE=0 HONEYCOMB_NO_SERVICE=1 HONEYCOMB_TELEMETRY=0 \
        HONEYCOMB_NO_MODIFY_PATH=1 "$HONEYCOMB" "$@" </dev/null
}

if ((skip_build)); then
    step "build skipped (--skip-build): reusing target/ and dist/Peek.app.zip"
else
    if [[ $(uname -s) == Darwin ]]; then
        # One identity for the CLI Mach-Os and the app, resolved once.
        # shellcheck source=scripts/lib/signing.sh
        . "$ROOT/scripts/lib/signing.sh"
        resolve_signing_identity
    fi
    step "1/5 CLI, six targets"
    "$ROOT/scripts/build-cli-release.sh"
    step "2/5 Peek.app"
    "$ROOT/scripts/build-mac-release.sh"
fi

step "3/5 stage dist/stage"
python3 "$ROOT/scripts/package-honeycomb.py" --version "$V" --out "$DIST/stage" --replace --honeycomb "$HONEYCOMB"

step "4/5 honeycomb validate, pack, validate"
hc validate "$DIST/stage" --json
hc pack "$DIST/stage" --output "$ARCHIVE" --json
hc validate "$ARCHIVE" --json
python3 "$ROOT/scripts/package-honeycomb.py" --verify-archive "$ARCHIVE" --out "$DIST/stage"
(cd "$DIST" && shasum -a 256 "$(basename "$ARCHIVE")" >"$(basename "$ARCHIVE").sha256")

if ((skip_loopback)); then
    step "5/5 loopback install test skipped (--skip-loopback)"
else
    step "5/5 loopback install test (fake API on 127.0.0.1, scratch SILICON_HOME, PATH removed)"
    python3 "$ROOT/scripts/loopback-install-test.py" --archive "$ARCHIVE" --honeycomb "$HONEYCOMB"
fi

bundle_id=$(sed -n 's/^bundle_id=//p' "$DIST/stage/targets/macos-aarch64/Peek.app.info")
channel=prod
[[ $bundle_id == ai.tos.peek ]] || channel=dev
cat >&2 <<EOF

Release candidate ready: $ARCHIVE
  $(cat "$ARCHIVE.sha256")
  Peek.app bundle id $bundle_id → upload to the '$channel' channel only$([[ $channel == dev ]] && echo ' (Apple Development build; prod waits for Developer ID + notarization, BLUEPRINT §5.5)')
Nothing was uploaded. Publishing is [MUTATING] and needs approval: deploy/README.md P10 (BLUEPRINT §4.5):
  HONEYCOMB_AUTO_UPDATE=0 honeycomb --json apps get peek      # note "revision"
  HONEYCOMB_AUTO_UPDATE=0 honeycomb --json --idempotency-key peek-rel-$V-$channel-01 \\
    releases upload peek $ARCHIVE --channel $channel --revision <N>
EOF
