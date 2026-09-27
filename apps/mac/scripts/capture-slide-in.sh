#!/bin/sh
# Records a bubble's slide-in with ScreenCaptureKit at 60 fps and checks that the visual lands in colour
# (peek 0.1.2 contract §8.6 step 5: no grey → colour snap after landing).
#
# Usage: apps/mac/scripts/capture-slide-in.sh <Debug Peek.app> <output dir> [capture options…]
#   options: --scenario show-cover  --position 1-8  --mode normal|compact  --fps 60  --scale 1
#            --backdrop <png>  --label <name>  --frame-stride 2
#
# The app runs isolated (PEEK_NO_SERVICES=1, temporary support/caches/socket directories: no login item, no launchd
# agent, no connection to the real peekd) with `--simulate <scenario>` over a vivid backdrop, and only Peek's own
# windows are captured. Needs a Debug build (the PEEK_DEBUG_SLIDE_LOG timestamps) and Screen Recording access for the
# terminal running this. Writes <label>-report.txt, <label>-saturation.csv, <label>-contact-sheet.png and frames/.
# Exit status: 0 pass, 2 fail (a colour ramp after landing), 1 error.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
app=${1:?usage: capture-slide-in.sh <Debug Peek.app> <output dir> [options]}
out=${2:?usage: capture-slide-in.sh <Debug Peek.app> <output dir> [options]}
shift 2

core="$here/../Packages/PeekKit/Sources/PeekCore"
build="${TMPDIR:-/tmp}/peek-capture-slide-in"
tool="$build/capture-slide-in"
mkdir -p "$build"

# Rebuild the tool when its source or PeekCore changed (PeekCore supplies the slot geometry).
newest="$(ls -t "$here/capture-slide-in.swift" "$core"/*.swift | head -n 1)"
if [ ! -x "$tool" ] || [ "$newest" -nt "$tool" ]; then
  echo "capture-slide-in: compiling the capture tool" >&2
  xcrun swiftc -O -parse-as-library -swift-version 5 -module-name CaptureSlideIn \
    -target "$(uname -m)-apple-macosx26.0" -o "$tool" "$here/capture-slide-in.swift" "$core"/*.swift
fi

exec "$tool" --app "$app" --out "$out" "$@"
