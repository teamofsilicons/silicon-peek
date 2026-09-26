#!/bin/sh
# Checks a built Peek.app (any configuration, signed or not) for what launchd and SMAppService need:
#   Contents/MacOS/Peek, Contents/Helpers/peekd (executable), and
#   Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist with Label ai.tos.peek.daemon,
#   BundleProgram Contents/Helpers/peekd and AssociatedBundleIdentifiers [CFBundleIdentifier].
# Optional: PEEK_EXPECT_VERSION / PEEK_EXPECT_BUILD compare CFBundleShortVersionString / CFBundleVersion.
#
#   apps/mac/scripts/check-app-bundle.sh apps/mac/build-fix/Build/Products/Release/Peek.app
set -eu

app=${1:?usage: check-app-bundle.sh <path/to/Peek.app>}
buddy=/usr/libexec/PlistBuddy
fail() { echo "check-app-bundle: error: $*" >&2; exit 1; }
print() { "$buddy" -c "Print :$2" "$1" 2>/dev/null || true; }

[ -d "$app" ] || fail "$app is not a directory"
info="$app/Contents/Info.plist"
[ -f "$info" ] || fail "$info is missing"
[ -x "$app/Contents/MacOS/Peek" ] || fail "Contents/MacOS/Peek is missing or not executable"
[ -x "$app/Contents/Helpers/peekd" ] || fail "Contents/Helpers/peekd is missing or not executable (scripts/embed-peekd.sh)"

agent="$app/Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist"
[ -f "$agent" ] || fail "Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist is missing (copyFiles phase in apps/mac/project.yml)"
plutil -lint "$agent" >/dev/null || fail "$agent is not a valid plist"
[ "$(print "$agent" Label)" = "ai.tos.peek.daemon" ] || fail "the agent Label is '$(print "$agent" Label)', expected ai.tos.peek.daemon"
[ "$(print "$agent" BundleProgram)" = "Contents/Helpers/peekd" ] \
    || fail "the agent BundleProgram is '$(print "$agent" BundleProgram)', expected Contents/Helpers/peekd"
bundle_id=$(print "$info" CFBundleIdentifier)
[ "$(print "$agent" AssociatedBundleIdentifiers:0)" = "$bundle_id" ] \
    || fail "the agent AssociatedBundleIdentifiers is '$(print "$agent" AssociatedBundleIdentifiers:0)', expected [$bundle_id] (scripts/configure-launch-agent.sh)"

version=$(print "$info" CFBundleShortVersionString)
build=$(print "$info" CFBundleVersion)
[ -z "${PEEK_EXPECT_VERSION:-}" ] || [ "$version" = "$PEEK_EXPECT_VERSION" ] || fail "CFBundleShortVersionString is $version, expected $PEEK_EXPECT_VERSION"
[ -z "${PEEK_EXPECT_BUILD:-}" ] || [ "$build" = "$PEEK_EXPECT_BUILD" ] || fail "CFBundleVersion is $build, expected $PEEK_EXPECT_BUILD"

echo "check-app-bundle: ok $app ($bundle_id $version ($build)); agent ai.tos.peek.daemon -> Contents/Helpers/peekd"
