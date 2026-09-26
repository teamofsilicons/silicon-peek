#!/bin/sh
# Vendor quickjs-ng into Packages/CQuickJS (BLUEPRINT §8.1).
#
# What it does
#   1. Downloads the pinned quickjs-ng release tarball from codeload.github.com.
#   2. Refuses to continue unless its sha256 equals the pinned value below.
#   3. Copies exactly the library sources and the headers they include into
#      Packages/CQuickJS/Sources/CQuickJS/, unmodified. quickjs-libc.c (std/os
#      modules) is deliberately NOT vendored: drawings get no file, process or
#      timer access.
#   4. Writes Packages/CQuickJS/LICENSE.quickjs-ng and Packages/CQuickJS/VERSION.
#
# Our own files (peek_qjs.c, include/peek_qjs.h) are never touched. quickjs.h
# stays next to the sources, not in include/, so Swift can only see the shim.
#
# Usage
#   apps/mac/scripts/vendor-quickjs.sh            # vendor the pinned version
#   apps/mac/scripts/vendor-quickjs.sh --verify   # check the vendored files still match the tarball
#
# To upgrade: change QJS_VERSION, run once with QJS_SHA256=skip to print the new
# hash, review the upstream diff, pin the printed hash, and run again.
set -eu

QJS_VERSION="v0.17.0"
QJS_SHA256="${QJS_SHA256:-559bc4c420475e55c7ab4510adbc562f55d7524d75e8e89d79ce4bb02f5687d9}"
QJS_URL="https://codeload.github.com/quickjs-ng/quickjs/tar.gz/refs/tags/${QJS_VERSION}"

SOURCES="quickjs.c libregexp.c libunicode.c dtoa.c"
HEADERS="quickjs.h cutils.h list.h libregexp.h libregexp-opcode.h libunicode.h libunicode-table.h dtoa.h
quickjs-atom.h quickjs-opcode.h quickjs-c-atomics.h
builtin-array-fromasync.h builtin-iterator-zip-keyed.h builtin-iterator-zip.h"

here="$(cd "$(dirname "$0")" && pwd)"
pkg="$(cd "$here/.." && pwd)/Packages/CQuickJS"
dest="$pkg/Sources/CQuickJS"

mode="vendor"
case "${1:-}" in
  "") ;;
  --verify) mode="verify" ;;
  -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
  *) echo "vendor-quickjs: unknown argument '$1' (expected --verify or nothing)" >&2; exit 2 ;;
esac

command -v curl >/dev/null 2>&1 || { echo "vendor-quickjs: curl is required" >&2; exit 1; }
command -v shasum >/dev/null 2>&1 || { echo "vendor-quickjs: shasum is required" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/vendor-quickjs.XXXXXX")"
trap 'rm -rf "$work"' EXIT INT TERM

echo "vendor-quickjs: downloading $QJS_URL"
curl --fail --silent --show-error --location --retry 3 --output "$work/quickjs.tar.gz" "$QJS_URL"

actual="$(shasum -a 256 "$work/quickjs.tar.gz" | awk '{print $1}')"
if [ "$QJS_SHA256" = "skip" ]; then
  echo "vendor-quickjs: sha256 of $QJS_VERSION is $actual (pin it in QJS_SHA256 before vendoring)"
  exit 0
fi
if [ "$actual" != "$QJS_SHA256" ]; then
  echo "vendor-quickjs: sha256 mismatch for $QJS_URL" >&2
  echo "  expected $QJS_SHA256" >&2
  echo "  actual   $actual" >&2
  echo "  The tag was re-published or the download was tampered with. Do not vendor it; investigate upstream first." >&2
  exit 1
fi

tar -xzf "$work/quickjs.tar.gz" -C "$work"
src="$work/quickjs-${QJS_VERSION#v}"
[ -d "$src" ] || { echo "vendor-quickjs: tarball has no quickjs-${QJS_VERSION#v}/ directory" >&2; exit 1; }

if [ "$mode" = "verify" ]; then
  status=0
  for f in $SOURCES $HEADERS; do
    if ! cmp -s "$src/$f" "$dest/$f"; then
      echo "vendor-quickjs: $dest/$f differs from upstream $QJS_VERSION" >&2
      status=1
    fi
  done
  [ "$status" -eq 0 ] && echo "vendor-quickjs: vendored files match quickjs-ng $QJS_VERSION ($QJS_SHA256)"
  exit "$status"
fi

mkdir -p "$dest/include"
for f in $SOURCES $HEADERS; do
  [ -f "$src/$f" ] || { echo "vendor-quickjs: upstream $QJS_VERSION has no $f; update the file lists in this script" >&2; exit 1; }
  cp "$src/$f" "$dest/$f"
  chmod 0644 "$dest/$f"
done
cp "$src/LICENSE" "$pkg/LICENSE.quickjs-ng"
chmod 0644 "$pkg/LICENSE.quickjs-ng"
cat > "$pkg/VERSION" <<EOF
quickjs-ng $QJS_VERSION
url    $QJS_URL
sha256 $QJS_SHA256
files  $(echo $SOURCES $HEADERS)
EOF

echo "vendor-quickjs: vendored quickjs-ng $QJS_VERSION into $dest"
