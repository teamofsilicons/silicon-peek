#!/bin/sh
# Xcode "Configure LaunchAgent" build phase: checks that the agent plist was copied to
# Peek.app/Contents/Library/LaunchAgents and points its AssociatedBundleIdentifiers at
# this build's bundle id (ai.tos.peek, or ai.tos.peek.dev for Debug), so the Login Items
# entry is attributed to the app that registered it.
set -eu

: "${TARGET_BUILD_DIR:?configure-launch-agent.sh runs as an Xcode build phase}"
: "${CONTENTS_FOLDER_PATH:?CONTENTS_FOLDER_PATH is unset}"
: "${PRODUCT_BUNDLE_IDENTIFIER:?PRODUCT_BUNDLE_IDENTIFIER is unset}"

plist="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Library/LaunchAgents/ai.tos.peek.daemon.plist"
if [ ! -f "$plist" ]; then
  echo "error: configure-launch-agent: $plist is missing; the copy-files phase for Resources/LaunchAgents/ai.tos.peek.daemon.plist did not run (check project.yml)" >&2
  exit 1
fi

buddy=/usr/libexec/PlistBuddy
"$buddy" -c "Delete :AssociatedBundleIdentifiers" "$plist" >/dev/null 2>&1 || true
"$buddy" -c "Add :AssociatedBundleIdentifiers array" "$plist"
"$buddy" -c "Add :AssociatedBundleIdentifiers:0 string $PRODUCT_BUNDLE_IDENTIFIER" "$plist"

label="$("$buddy" -c "Print :Label" "$plist")"
program="$("$buddy" -c "Print :BundleProgram" "$plist")"
[ "$label" = "ai.tos.peek.daemon" ] || { echo "error: configure-launch-agent: Label is '$label', expected ai.tos.peek.daemon" >&2; exit 1; }
[ "$program" = "Contents/Helpers/peekd" ] || { echo "error: configure-launch-agent: BundleProgram is '$program', expected Contents/Helpers/peekd" >&2; exit 1; }
plutil -lint "$plist" >/dev/null
echo "configure-launch-agent: $plist -> AssociatedBundleIdentifiers [$PRODUCT_BUNDLE_IDENTIFIER]"
