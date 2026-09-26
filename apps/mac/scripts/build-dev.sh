#!/bin/sh
# Build a Debug Peek.app (bundle id ai.tos.peek.dev) with xcodegen + xcodebuild.
# It never launches the app: launching registers a login item and the peekd agent.
#
# Usage: apps/mac/scripts/build-dev.sh [--unsigned] [--release-universal] [extra xcodebuild args…]
#   --unsigned           CODE_SIGNING_ALLOWED=NO (no keychain prompt; mic grants will not persist)
#   --release-universal  Release, ARCHS="arm64 x86_64", then check that Peek.app contains no symlinks
#   DERIVED_DATA=<dir>   derived data path (default apps/mac/build)
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

command -v xcodegen >/dev/null 2>&1 || { echo "build-dev: xcodegen is required (brew install xcodegen; 2.46 is known to work)" >&2; exit 1; }
command -v xcodebuild >/dev/null 2>&1 || { echo "build-dev: xcodebuild is required (install Xcode 26.4+)" >&2; exit 1; }

configuration=Debug
signing=""
universal=""
while [ $# -gt 0 ]; do
  case "$1" in
    --unsigned) signing="CODE_SIGNING_ALLOWED=NO"; shift ;;
    --release-universal) configuration=Release; universal=1; shift ;;
    -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
    *) break ;;
  esac
done

derived="${DERIVED_DATA:-$root/build}"
xcodegen generate --spec "$root/project.yml" --quiet

if [ -n "$universal" ]; then
  # shellcheck disable=SC2086
  xcodebuild -project Peek.xcodeproj -scheme Peek -configuration Release -destination 'generic/platform=macOS' \
    -derivedDataPath "$derived" ARCHS="arm64 x86_64" ONLY_ACTIVE_ARCH=NO $signing "$@" build
else
  # shellcheck disable=SC2086
  xcodebuild -project Peek.xcodeproj -scheme Peek -configuration Debug -destination 'platform=macOS' \
    -derivedDataPath "$derived" $signing "$@" build
fi

app="$derived/Build/Products/$configuration/Peek.app"
[ -d "$app" ] || { echo "build-dev: expected $app after the build" >&2; exit 1; }
links="$(find "$app" -type l)"
if [ -n "$links" ]; then
  echo "build-dev: Peek.app contains symlinks, which Honeycomb rejects:" >&2
  echo "$links" >&2
  exit 1
fi
echo "build-dev: built $app (no symlinks; archs: $(lipo -archs "$app/Contents/MacOS/Peek"))"
if [ ! -x "$app/Contents/Helpers/peekd" ]; then
  echo "build-dev: note: no Contents/Helpers/peekd embedded (see scripts/embed-peekd.sh)"
fi
echo "build-dev: run it with: open \"$app\"   (registers the login item and the peekd agent for $( [ "$configuration" = Debug ] && echo ai.tos.peek.dev || echo ai.tos.peek ))"
