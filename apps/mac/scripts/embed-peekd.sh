#!/bin/sh
# Xcode "Embed peekd" build phase (BLUEPRINT §8.1, D3): puts the Rust daemon at
# Peek.app/Contents/Helpers/peekd and signs it with the app's identity.
#
# Source, in order:
#   0. PEEK_SKIP_EMBED_PEEKD=1 skips the phase (UI work while crates/daemon does not build);
#   1. $PEEKD_BINARY, if set (must exist);
#   2. <repo>/dist/peekd (the release pipeline lipo's it there, BLUEPRINT §4.4), unless a
#      helper source (Cargo.toml, Cargo.lock, crates/client or crates/daemon) is newer than it:
#      a stale dist/peekd from an earlier release is then skipped with a warning (step 3);
#   3. a cargo build of crates/daemon (package silicon-peek-daemon) for every
#      architecture in $ARCHS, when crates/daemon/Cargo.toml exists;
#   4. otherwise nothing is embedded and Xcode shows a warning. The app still
#      builds and runs; it reports the missing helper when it tries to start peekd.
#
# A prebuilt binary must contain every architecture in $ARCHS (a universal Release
# needs arm64 and x86_64). The helper is signed with -i ai.tos.peek.daemon and the
# hardened runtime, before Xcode signs the app around it (inside-out).
set -eu

fail() {
  echo "error: embed-peekd: $*" >&2
  exit 1
}

: "${TARGET_BUILD_DIR:?embed-peekd.sh runs as an Xcode build phase (TARGET_BUILD_DIR is unset)}"
: "${CONTENTS_FOLDER_PATH:?CONTENTS_FOLDER_PATH is unset}"
: "${SRCROOT:?SRCROOT is unset}"
: "${ARCHS:?ARCHS is unset}"

repo="$(cd "$SRCROOT/../.." && pwd)"
dest_dir="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Helpers"
dest="$dest_dir/peekd"
configuration="${CONFIGURATION:-Debug}"

triple_for() {
  case "$1" in
    arm64) echo "aarch64-apple-darwin" ;;
    x86_64) echo "x86_64-apple-darwin" ;;
    *) fail "unsupported architecture '$1' in ARCHS='$ARCHS' (expected arm64 and/or x86_64)" ;;
  esac
}

# The first helper source newer than dist/peekd (empty when dist/peekd is current). Only the crates peekd is
# built from count: docs, the CLI, the server and the app do not change the helper.
stale_dist_peekd() {
  (cd "$repo" && find Cargo.toml Cargo.lock crates/client crates/daemon \
    \( -name target -prune \) -o \( -type f \( -name '*.rs' -o -name '*.toml' -o -name '*.sql' -o -name Cargo.lock \) \
    -newer dist/peekd -print \) 2>/dev/null | head -n 1)
}

check_archs() {
  have="$(lipo -archs "$1" 2>/dev/null)" || fail "$1 is not a Mach-O binary (lipo -archs failed)"
  for arch in $ARCHS; do
    case " $have " in
      *" $arch "*) ;;
      *) fail "$1 contains [$have] but this build needs [$ARCHS]. Rebuild peekd for $arch (see BLUEPRINT §4.4) or set PEEKD_BINARY." ;;
    esac
  done
}

build_with_cargo() {
  cargo="${CARGO:-}"
  if [ -z "$cargo" ]; then
    if [ -x "$HOME/.cargo/bin/cargo" ]; then cargo="$HOME/.cargo/bin/cargo"
    elif command -v cargo >/dev/null 2>&1; then cargo="$(command -v cargo)"
    else fail "crates/daemon exists but cargo was not found (looked at \$CARGO, ~/.cargo/bin/cargo and PATH). Install rustup or set PEEKD_BINARY."
    fi
  fi
  profile_flag=""
  profile_dir="debug"
  if [ "$configuration" = "Release" ]; then profile_flag="--release"; profile_dir="release"; fi
  locked=""
  [ -f "$repo/Cargo.lock" ] && locked="--locked"
  target_dir="${CARGO_TARGET_DIR:-$repo/target}"
  built=""
  for arch in $ARCHS; do
    triple="$(triple_for "$arch")"
    echo "embed-peekd: cargo build -p silicon-peek-daemon --target $triple $profile_flag"
    # A clean environment: Xcode exports build settings (SDKROOT, LD, ARCHS…) that confuse cargo and cc.
    (cd "$repo" && /usr/bin/env -i HOME="$HOME" USER="${USER:-}" TMPDIR="${TMPDIR:-/tmp}" \
      PATH="$(dirname "$cargo"):/usr/bin:/bin:/usr/sbin:/sbin" \
      CARGO_TARGET_DIR="$target_dir" MACOSX_DEPLOYMENT_TARGET=26.0 \
      "$cargo" build -p silicon-peek-daemon --bin peekd --target "$triple" $profile_flag $locked) \
      || fail "cargo build of peekd for $triple failed (see the log above)"
    built="$built $target_dir/$triple/$profile_dir/peekd"
  done
  out_dir="${DERIVED_FILE_DIR:-${TMPDIR:-/tmp}}"
  out="$out_dir/peekd"
  mkdir -p "$out_dir"
  # shellcheck disable=SC2086
  lipo -create $built -output "$out" || fail "lipo -create of$built failed"
  source_binary="$out"
}

if [ "${PEEK_SKIP_EMBED_PEEKD:-0}" = "1" ]; then
  rm -f "$dest"
  echo "warning: embed-peekd: PEEK_SKIP_EMBED_PEEKD=1, Peek.app is built without Contents/Helpers/peekd."
  exit 0
elif [ -n "${PEEKD_BINARY:-}" ]; then
  [ -f "$PEEKD_BINARY" ] || fail "PEEKD_BINARY=$PEEKD_BINARY does not exist"
  source_binary="$PEEKD_BINARY"
  echo "embed-peekd: using PEEKD_BINARY=$source_binary"
elif [ -f "$repo/dist/peekd" ] && [ -z "$(stale_dist_peekd)" ]; then
  source_binary="$repo/dist/peekd"
  echo "embed-peekd: using $source_binary"
elif [ -f "$repo/crates/daemon/Cargo.toml" ]; then
  if [ -f "$repo/dist/peekd" ]; then
    echo "warning: embed-peekd: $repo/dist/peekd is older than $(stale_dist_peekd); building peekd from this tree instead."
  fi
  build_with_cargo
else
  rm -f "$dest"
  echo "warning: embed-peekd: no peekd to embed (no \$PEEKD_BINARY, no $repo/dist/peekd, no $repo/crates/daemon). Peek.app is built without Contents/Helpers/peekd."
  exit 0
fi

check_archs "$source_binary"
mkdir -p "$dest_dir"
rm -f "$dest"
# A plain copy: no symlinks inside Peek.app (Honeycomb rejects them).
cp "$source_binary" "$dest"
chmod 0755 "$dest"

if [ "${CODE_SIGNING_ALLOWED:-YES}" = "YES" ] && [ -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ]; then
  timestamp="--timestamp=none"
  [ "$configuration" = "Release" ] && timestamp="--timestamp"
  codesign --force --options runtime $timestamp -i ai.tos.peek.daemon -s "$EXPANDED_CODE_SIGN_IDENTITY" "$dest" \
    || fail "codesign of $dest with identity $EXPANDED_CODE_SIGN_IDENTITY failed"
  echo "embed-peekd: signed $dest as ai.tos.peek.daemon"
else
  echo "embed-peekd: code signing disabled for this build; $dest is unsigned"
fi
