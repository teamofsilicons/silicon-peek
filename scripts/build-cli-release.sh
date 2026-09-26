#!/usr/bin/env bash
# Build the peek CLI (silicon-peek-cli, bin `peek`) for all six Honeycomb targets from one Mac
# (BLUEPRINT §4.4 step 1, notes/honeycomb §5):
#   macOS  aarch64/x86_64   cargo build, MACOSX_DEPLOYMENT_TARGET=11.0 (iam/login work on old Macs too)
#   Linux  aarch64/x86_64   cargo zigbuild, musl, fully static (glibc builds would need 2.39)
#   Windows aarch64/x86_64  cargo xwin, MSVC with the CRT linked statically
# Then verifies every binary's format and architecture (scripts/package-honeycomb.py --check-binaries).
#
# Environment:
#   CARGO_TARGET_DIR  cargo output directory (default: <repo>/target)
#   SIGN_IDENTITY     optional codesigning identity for the two macOS CLI binaries, signed with the
#                     hardened runtime and a secure timestamp (§8.10). "Developer ID Application: …
#                     (LTBSK59BJ2)" is for prod releases; "Apple Development: …" signs tos-internal
#                     dev releases the same way. Unset: the linker signature is kept.
#   PEEK_SIGN_TIMESTAMP  none: no secure timestamp (offline development builds only; scripts/lib/signing.sh)
# Needs: rustup targets for all six triples, cargo-zigbuild + zig, cargo-xwin + clang/lld.
# Writes only to CARGO_TARGET_DIR. Never publishes anything.
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$ROOT"
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
export CARGO_TARGET_DIR=$TARGET_DIR
PACKAGE=silicon-peek-cli
MAC_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
LINUX_TARGETS=(aarch64-unknown-linux-musl x86_64-unknown-linux-musl)
WINDOWS_TARGETS=(aarch64-pc-windows-msvc x86_64-pc-windows-msvc)

die() {
    printf 'build-cli-release: error: %s\n' "$1" >&2
    [[ -z ${2:-} ]] || printf 'fix: %s\n' "$2" >&2
    exit 1
}
step() { printf '\n==> %s\n' "$*" >&2; }

[[ $(uname -s) == Darwin ]] || die "the six-target build runs on macOS (Apple targets need the macOS SDK)" \
    "run it on a Mac, or use .github/workflows/release.yml, which builds each target natively"

for tool in cargo rustup cargo-zigbuild zig cargo-xwin python3; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not on PATH" \
        "install the release toolchain listed in the header of this script (BLUEPRINT §5.1)"
done

# An Xcode update leaves its license unaccepted; every compiler shim then exits 69 with a license notice.
if ! toolchain=$(cc --version 2>&1); then
    die "the C toolchain of the active developer directory ($(xcode-select -p 2>/dev/null)) does not run: ${toolchain//$'\n'/ }" \
        "[USER] accept the license with 'sudo xcodebuild -license', or run with DEVELOPER_DIR=/Library/Developer/CommandLineTools (the CLI needs only the Command Line Tools)"
fi

installed=$(rustup target list --installed)
missing=()
for target in "${MAC_TARGETS[@]}" "${LINUX_TARGETS[@]}" "${WINDOWS_TARGETS[@]}"; do
    grep -qx "$target" <<<"$installed" || missing+=("$target")
done
((${#missing[@]} == 0)) || die "rustup is missing the targets: ${missing[*]}" "rustup target add ${missing[*]}"

step "macOS CLI (${MAC_TARGETS[*]}), deployment target 11.0"
MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --locked -p "$PACKAGE" \
    --target "${MAC_TARGETS[0]}" --target "${MAC_TARGETS[1]}"

step "Linux CLI (${LINUX_TARGETS[*]}), static musl via zig"
cargo zigbuild --release --locked -p "$PACKAGE" \
    --target "${LINUX_TARGETS[0]}" --target "${LINUX_TARGETS[1]}"

step "Windows CLI (${WINDOWS_TARGETS[*]}), MSVC with +crt-static"
RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" cargo xwin build --cross-compiler clang \
    --release --locked -p "$PACKAGE" --target "${WINDOWS_TARGETS[0]}" --target "${WINDOWS_TARGETS[1]}"

if [[ -n ${SIGN_IDENTITY:-} ]]; then
    # shellcheck source=scripts/lib/signing.sh
    . "$ROOT/scripts/lib/signing.sh"
    resolve_signing_identity
    signing_timestamp_flags
    for target in "${MAC_TARGETS[@]}"; do
        binary="$TARGET_DIR/$target/release/peek"
        step "codesign $binary ($SIGN_KIND, hardened runtime, ${SIGN_TIMESTAMP[*]})"
        codesign --force --options runtime "${SIGN_TIMESTAMP[@]}" -s "$SIGN_HASH" "$binary"
        codesign --verify --strict --verbose=2 "$binary"
    done
else
    printf 'build-cli-release: SIGN_IDENTITY unset; macOS CLI binaries keep their linker signature\n' >&2
fi

step "verify formats and architectures"
python3 "$ROOT/scripts/package-honeycomb.py" --check-binaries --target-dir "$TARGET_DIR"
