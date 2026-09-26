#!/usr/bin/env bash
# Build, sign, (optionally) notarize and zip the universal Peek.app (BLUEPRINT §4.4 steps 2-4, §4.3, §8.10).
#
#   1. peekd, universal: cargo (MACOSX_DEPLOYMENT_TARGET=26.0) for arm64 + x86_64, lipo → dist/peekd
#      (apps/mac/scripts/embed-peekd.sh copies dist/peekd into Contents/Helpers during the build).
#   2. xcodegen + xcodebuild Release, ARCHS="arm64 x86_64".
#   3. Sign inside-out: nested code, then Contents/Helpers/peekd (-i ai.tos.peek.daemon), then the
#      app with apps/mac/Resources/Peek.entitlements; hardened runtime everywhere.
#   4. With NOTARY_PROFILE: notarytool submit --wait, staple, validate.
#   5. ditto -c -k --norsrc --noextattr --noqtn --noacl --keepParent → dist/Peek.app.zip; refuse
#      AppleDouble entries; unpack with plain ditto -x -k and verify again (CDHash unchanged).
#   6. Write dist/Peek.app.build.json, which scripts/package-honeycomb.py turns into Peek.app.info.
#
# Environment:
#   SIGN_IDENTITY     identity name or SHA-1; default: Developer ID Application for PEEK_TEAM_ID if the
#                     keychain has one, else Apple Development (scripts/lib/signing.sh). Developer ID
#                     builds are ai.tos.peek (team_id LTBSK59BJ2); Apple Development builds are
#                     ai.tos.peek.dev with an empty team_id, for the dev channel only (§5.5).
#   PEEK_SIGN_TIMESTAMP secure (default): every signature, Apple Development included, carries Apple's
#                     secure timestamp; none: offline development builds only (refused for Developer ID)
#   NOTARY_PROFILE    notarytool keychain profile (e.g. peek-notary, [USER] §8.10); unset = no notarization
#   NOTARY_KEYCHAIN   keychain holding that profile (CI uses a temporary keychain)
#   PEEK_TEAM_ID      default LTBSK59BJ2
#   PEEK_DERIVED_DATA xcodebuild -derivedDataPath (default apps/mac/build)
#   PEEK_XCODE_SIGNING identity (default): xcodebuild signs with SIGN_IDENTITY (manual style) and step 3
#                     re-signs explicitly; none: xcodebuild runs with CODE_SIGNING_ALLOWED=NO and step 3
#                     does all signing (fallback if manual signing in xcodebuild is refused)
#   PEEK_PREBUILT_APP an unsigned universal Release Peek.app that xcodebuild already produced from this
#                     tree (e.g. apps/mac/build-*/Build/Products/Release/Peek.app), for a machine whose
#                     Xcode cannot run right now (an Xcode update leaves its license unaccepted until the
#                     [USER] runs `sudo xcodebuild -license`). Step 2 then copies it instead of running
#                     xcodegen/xcodebuild, refuses it if any app source is newer than its executable, and
#                     replays the two Xcode build phases for this build (embed-peekd.sh with the fresh
#                     dist/peekd, configure-launch-agent.sh) plus CFBundleIdentifier=$BUNDLE_ID; steps 3-6
#                     are unchanged. Pair it with DEVELOPER_DIR=/Library/Developer/CommandLineTools so
#                     cargo and lipo use the Command Line Tools.
#   CARGO_TARGET_DIR  default <repo>/target
# The first codesign may raise a keychain "Always Allow" dialog ([USER], BLUEPRINT §10.1 U8).
# Writes dist/peekd, dist/Peek.app.zip, dist/Peek.app.build.json. Never publishes anything.
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$ROOT"
# shellcheck source=scripts/lib/signing.sh
. "$ROOT/scripts/lib/signing.sh"

TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
export CARGO_TARGET_DIR=$TARGET_DIR
DIST=$ROOT/dist
DERIVED=${PEEK_DERIVED_DATA:-$ROOT/apps/mac/build}
TEAM=${PEEK_TEAM_ID:-LTBSK59BJ2}
NOTARY_PROFILE=${NOTARY_PROFILE:-}
NOTARY_KEYCHAIN=${NOTARY_KEYCHAIN:-}
XCODE_SIGNING=${PEEK_XCODE_SIGNING:-identity}
PREBUILT_APP=${PEEK_PREBUILT_APP:-}
ENTITLEMENTS=$ROOT/apps/mac/Resources/Peek.entitlements
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/peek-mac-release.XXXXXX")
trap 'rm -rf "$SCRATCH"' EXIT

die() {
    printf 'build-mac-release: error: %s\n' "$1" >&2
    [[ -z ${2:-} ]] || printf 'fix: %s\n' "$2" >&2
    exit 1
}
step() { printf '\n==> %s\n' "$*" >&2; }
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1" 2>/dev/null; }
# Output is captured before it is matched: with pipefail, `codesign … | grep -q` fails whenever grep
# exits on the first match and codesign then takes SIGPIPE (and `| head -n 1` can abort a set -e assignment).
codesign_details() { codesign -d --verbose=4 "$1" 2>&1 || true; }
codesign_field() { awk -v key="$1" 'index($0, key "=") == 1 { print substr($0, length(key) + 2); exit }' <<<"$(codesign_details "$2")"; }

[[ $(uname -s) == Darwin ]] || die "Peek.app builds only on macOS" "run on a Mac with Xcode 26.4+ (BLUEPRINT §1.4)"
for tool in cargo lipo xcodegen xcodebuild codesign ditto zipinfo xcrun security python3 shasum; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not on PATH" "install Xcode 26.4+, xcodegen 2.46+ and the Rust toolchain"
done
# An Xcode update leaves its license unaccepted; every compiler shim then exits 69 with a license notice.
if ! toolchain=$(cc --version 2>&1); then
    die "the C toolchain of the active developer directory ($(xcode-select -p 2>/dev/null)) does not run: ${toolchain//$'\n'/ }" \
        "[USER] accept the license with 'sudo xcodebuild -license', or run with DEVELOPER_DIR=/Library/Developer/CommandLineTools (cargo and lipo need only the Command Line Tools) plus PEEK_PREBUILT_APP"
fi
if [[ -z $PREBUILT_APP ]] && ! xcode_version=$(xcodebuild -version 2>&1); then
    die "xcodebuild does not run: ${xcode_version//$'\n'/ }" \
        "[USER] 'sudo xcodebuild -license' (and DEVELOPER_DIR must point at Xcode.app), or pass PEEK_PREBUILT_APP=<unsigned Release Peek.app built from this tree>"
fi
[[ -f $ROOT/apps/mac/project.yml ]] || die "apps/mac/project.yml is missing" "the Mac app lives in apps/mac (BLUEPRINT §8.1)"
[[ -f $ENTITLEMENTS ]] || die "$ENTITLEMENTS is missing" "restore apps/mac/Resources/Peek.entitlements (BLUEPRINT §8.9)"

versions=$(python3 "$ROOT/scripts/package-honeycomb.py" --check-versions)
VERSION=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])' <<<"$versions")
BUILD=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["bundle_version"])' <<<"$versions")

resolve_signing_identity
signing_timestamp_flags
TIMESTAMP=("${SIGN_TIMESTAMP[@]}")
if [[ $SIGN_KIND == developer_id ]]; then
    BUNDLE_ID=ai.tos.peek
    INFO_TEAM=$TEAM
else
    BUNDLE_ID=ai.tos.peek.dev
    INFO_TEAM=""
fi
if [[ -n $NOTARY_PROFILE && $SIGN_KIND != developer_id ]]; then
    die "NOTARY_PROFILE is set but \"$SIGN_IDENTITY\" is not a Developer ID identity; Apple notarizes only Developer ID code" \
        "unset NOTARY_PROFILE for a development build, or sign with Developer ID Application"
fi
RUNTIME_FLAGS='flags=0x[0-9a-f]+\([^)]*runtime'
REQUIREMENT="anchor apple generic and identifier \"$BUNDLE_ID\" and certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = \"$TEAM\""
case $XCODE_SIGNING in
    identity) xcode_signing=(CODE_SIGN_STYLE=Manual "CODE_SIGN_IDENTITY=$SIGN_IDENTITY" PROVISIONING_PROFILE_SPECIFIER=) ;;
    none) xcode_signing=(CODE_SIGNING_ALLOWED=NO) ;;
    *) die "PEEK_XCODE_SIGNING=$XCODE_SIGNING" "use identity (default) or none" ;;
esac
mkdir -p "$DIST"

step "1/6 peekd $VERSION, universal (deployment target 26.0)"
MACOSX_DEPLOYMENT_TARGET=26.0 cargo build --release --locked -p silicon-peek-daemon \
    --target aarch64-apple-darwin --target x86_64-apple-darwin
lipo -create "$TARGET_DIR/aarch64-apple-darwin/release/peekd" "$TARGET_DIR/x86_64-apple-darwin/release/peekd" \
    -output "$DIST/peekd.tmp"
mv -f "$DIST/peekd.tmp" "$DIST/peekd"
archs=$(lipo -archs "$DIST/peekd")
[[ " $archs " == *" arm64 "* && " $archs " == *" x86_64 "* ]] || die "dist/peekd has architectures '$archs'" "both arm64 and x86_64 are required"

APP=$DERIVED/Build/Products/Release/Peek.app
APP_SOURCE=xcodebuild
if [[ -n $PREBUILT_APP ]]; then
    step "2/6 Peek.app $VERSION ($BUILD) as $BUNDLE_ID from the prebuilt Release bundle $PREBUILT_APP (no xcodebuild)"
    PREBUILT_APP=$(CDPATH='' cd -- "$PREBUILT_APP" 2>/dev/null && pwd -P) || die "PEEK_PREBUILT_APP=${PEEK_PREBUILT_APP} is not a directory"
    [[ -f $PREBUILT_APP/Contents/MacOS/Peek && -f $PREBUILT_APP/Contents/Info.plist ]] \
        || die "$PREBUILT_APP is not a Peek.app bundle (Contents/MacOS/Peek or Contents/Info.plist is missing)"
    [[ $PREBUILT_APP != "$APP" ]] || die "PEEK_PREBUILT_APP must not be $APP itself" "pass another derived-data path"
    prebuilt_archs=$(lipo -archs "$PREBUILT_APP/Contents/MacOS/Peek" 2>/dev/null || true)
    [[ " $prebuilt_archs " == *" arm64 "* && " $prebuilt_archs " == *" x86_64 "* ]] \
        || die "the prebuilt Peek executable has architectures '${prebuilt_archs:-none}'" "build it with ARCHS=\"arm64 x86_64\" ONLY_ACTIVE_ARCH=NO"
    [[ $(plist "$PREBUILT_APP/Contents/Info.plist" CFBundleIdentifier) == ai.tos.peek* ]] || die "the prebuilt bundle is not Peek"
    # Only this tree's sources may be in it: refuse the bundle if any app source changed after it was linked.
    stale=$(find "$ROOT/apps/mac/Sources" "$ROOT/apps/mac/Resources" "$ROOT/apps/mac/project.yml" "$ROOT/apps/mac/scripts" \
                "$ROOT/apps/mac/Packages/PeekKit/Package.swift" "$ROOT/apps/mac/Packages/PeekKit/Sources" \
                "$ROOT/apps/mac/Packages/CQuickJS/Package.swift" "$ROOT/apps/mac/Packages/CQuickJS/Sources" \
                -type f -newer "$PREBUILT_APP/Contents/MacOS/Peek" -print 2>/dev/null | head -n 5)
    [[ -z $stale ]] || die "the prebuilt Peek.app is older than these app sources: ${stale//$'\n'/, }" \
        "rebuild Peek.app with xcodebuild (BLUEPRINT §4.4 step 3)"
    rm -rf "$APP"
    mkdir -p "$(dirname "$APP")"
    ditto --norsrc --noextattr --noqtn --noacl "$PREBUILT_APP" "$APP"
    # What the Release build does for PRODUCT_BUNDLE_IDENTIFIER=$BUNDLE_ID: Info.plist, then the two phases.
    /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $BUNDLE_ID" "$APP/Contents/Info.plist"
    phase_env=(TARGET_BUILD_DIR="$(dirname "$APP")" CONTENTS_FOLDER_PATH=Peek.app/Contents SRCROOT="$ROOT/apps/mac"
               ARCHS="arm64 x86_64" CONFIGURATION=Release PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_ID")
    env "${phase_env[@]}" PEEKD_BINARY="$DIST/peekd" CODE_SIGNING_ALLOWED=NO "$ROOT/apps/mac/scripts/embed-peekd.sh"
    env "${phase_env[@]}" "$ROOT/apps/mac/scripts/configure-launch-agent.sh"
    APP_SOURCE="prebuilt:$PREBUILT_APP (Xcode $(plist "$APP/Contents/Info.plist" DTXcode), SDK $(plist "$APP/Contents/Info.plist" DTSDKName))"
else
    step "2/6 Peek.app $VERSION ($BUILD) as $BUNDLE_ID, Release, arm64 + x86_64"
    (cd "$ROOT/apps/mac" && xcodegen generate --spec project.yml)
    rm -rf "$APP"
    xcodebuild -project "$ROOT/apps/mac/Peek.xcodeproj" -scheme Peek -configuration Release \
        -destination 'generic/platform=macOS' -derivedDataPath "$DERIVED" \
        ARCHS="arm64 x86_64" ONLY_ACTIVE_ARCH=NO \
        PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_ID" DEVELOPMENT_TEAM="$TEAM" "${xcode_signing[@]}" \
        build
fi
[[ -d $APP ]] || die "xcodebuild reported success but $APP does not exist" "check the scheme's build output path"
[[ $(plist "$APP/Contents/Info.plist" CFBundleIdentifier) == "$BUNDLE_ID" ]] || die "Peek.app's CFBundleIdentifier is not $BUNDLE_ID"
[[ $(plist "$APP/Contents/Info.plist" CFBundleShortVersionString) == "$VERSION" ]] || die "CFBundleShortVersionString differs from $VERSION" \
    "Info.plist must use \$(MARKETING_VERSION)"
[[ $(plist "$APP/Contents/Info.plist" CFBundleVersion) == "$BUILD" ]] || die "CFBundleVersion differs from $BUILD" \
    "Info.plist must use \$(CURRENT_PROJECT_VERSION)"
[[ -f $APP/Contents/Helpers/peekd ]] || die "Contents/Helpers/peekd is missing" "apps/mac/scripts/embed-peekd.sh must copy dist/peekd (BLUEPRINT §8.1)"
embedded_archs=$(lipo -archs "$APP/Contents/Helpers/peekd" 2>/dev/null || true)
[[ " $embedded_archs " == *" arm64 "* && " $embedded_archs " == *" x86_64 "* ]] \
    || die "Contents/Helpers/peekd has architectures '${embedded_archs:-none}'" "embed-peekd.sh must copy the universal dist/peekd"
[[ -f $APP/Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist ]] || die "the daemon agent plist is not in Contents/Library/LaunchAgents" \
    "see the copyFiles phase in apps/mac/project.yml (BLUEPRINT §8.1)"
[[ $(plist "$APP/Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist" AssociatedBundleIdentifiers:0) == "$BUNDLE_ID" ]] \
    || die "the agent plist's AssociatedBundleIdentifiers is not [$BUNDLE_ID]" "apps/mac/scripts/configure-launch-agent.sh must run for this bundle id"

step "3/6 sign inside-out with \"$SIGN_IDENTITY\""
while IFS= read -r nested; do
    [[ -n $nested ]] || continue
    codesign --force --options runtime "${TIMESTAMP[@]}" -s "$SIGN_HASH" "$nested"
done < <(find "$APP/Contents/Frameworks" "$APP/Contents/PlugIns" "$APP/Contents/XPCServices" \
            \( -name '*.dylib' -o -name '*.framework' -o -name '*.xpc' -o -name '*.appex' -o -name '*.app' \) \
            -print 2>/dev/null | awk -F/ '{ print NF "\t" $0 }' | sort -rn | cut -f2-)  # deepest first
codesign --force --options runtime "${TIMESTAMP[@]}" -i ai.tos.peek.daemon -s "$SIGN_HASH" "$APP/Contents/Helpers/peekd"
codesign --force --options runtime "${TIMESTAMP[@]}" --entitlements "$ENTITLEMENTS" -s "$SIGN_HASH" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
if [[ $SIGN_KIND == developer_id ]]; then
    codesign --verify --deep --strict -R="$REQUIREMENT" "$APP" || die "Peek.app does not satisfy the Developer ID requirement for team $TEAM"
fi
[[ $(codesign_field Identifier "$APP") == "$BUNDLE_ID" ]] || die "Peek.app is not signed as $BUNDLE_ID"
[[ $(codesign_field Identifier "$APP/Contents/Helpers/peekd") == ai.tos.peek.daemon ]] || die "peekd is not signed as ai.tos.peek.daemon"
[[ $(codesign_field TeamIdentifier "$APP") == "$TEAM" ]] || die "Peek.app is signed by team '$(codesign_field TeamIdentifier "$APP")', expected $TEAM"
for code in "$APP" "$APP/Contents/Helpers/peekd"; do
    details=$(codesign_details "$code")
    [[ $details =~ $RUNTIME_FLAGS ]] || die "$code lacks the hardened runtime"
    if [[ ${TIMESTAMP[0]} == --timestamp ]]; then
        [[ $'\n'$details == *$'\n'Timestamp=* ]] || die "$code carries no secure timestamp" \
            "the timestamp server (timestamp.apple.com) must be reachable; PEEK_SIGN_TIMESTAMP=none only for offline dev builds"
    fi
done
entitlements=$(codesign -d --entitlements - --xml "$APP" 2>/dev/null || true)
if [[ $entitlements == *get-task-allow* ]]; then
    die "Peek.app carries com.apple.security.get-task-allow" "remove it; notarization rejects it (BLUEPRINT §8.1 Release config)"
fi
CDHASH=$(codesign_field CDHash "$APP")
[[ -n $CDHASH ]] || die "codesign printed no CDHash for $APP"

NOTARIZED=false
if [[ -n $NOTARY_PROFILE ]]; then
    step "4/6 notarize with keychain profile $NOTARY_PROFILE"
    ditto -c -k --keepParent "$APP" "$SCRATCH/Peek-notarize.zip"
    keychain_args=()
    [[ -z $NOTARY_KEYCHAIN ]] || keychain_args=(--keychain "$NOTARY_KEYCHAIN")
    xcrun notarytool submit "$SCRATCH/Peek-notarize.zip" --keychain-profile "$NOTARY_PROFILE" ${keychain_args[@]+"${keychain_args[@]}"} \
        --wait --output-format json >"$SCRATCH/notary.json" || true
    status=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("status",""))' "$SCRATCH/notary.json" 2>/dev/null || true)
    submission=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id",""))' "$SCRATCH/notary.json" 2>/dev/null || true)
    if [[ $status != Accepted ]]; then
        [[ -z $submission ]] || xcrun notarytool log "$submission" --keychain-profile "$NOTARY_PROFILE" ${keychain_args[@]+"${keychain_args[@]}"} >&2 || true
        die "notarization ended with status '${status:-unknown}' (submission ${submission:-none})" "read the log above, fix the signature, rebuild"
    fi
    xcrun stapler staple "$APP"
    xcrun stapler validate "$APP"
    NOTARIZED=true
else
    step "4/6 notarization skipped (NOTARY_PROFILE unset)"
    [[ $SIGN_KIND != developer_id ]] || printf 'build-mac-release: warning: this Developer ID build is not notarized; prod-channel uploads wait for notarization (BLUEPRINT §5.5)\n' >&2
fi

step "5/6 dist/Peek.app.zip (ditto --norsrc --noextattr --noqtn --noacl --keepParent)"
rm -f "$DIST/Peek.app.zip" "$DIST/Peek.app.zip.tmp"
ditto -c -k --norsrc --noextattr --noqtn --noacl --keepParent "$APP" "$DIST/Peek.app.zip.tmp"
if zipinfo -1 "$DIST/Peek.app.zip.tmp" | grep -E '(^|/)\._|__MACOSX'; then
    die "the zip contains AppleDouble entries (listed above), which invalidate the signature after extraction" \
        "check that ditto received --norsrc --noextattr"
fi
mv -f "$DIST/Peek.app.zip.tmp" "$DIST/Peek.app.zip"

step "6/6 round trip: plain ditto -x -k, then verify again"
ditto -x -k "$DIST/Peek.app.zip" "$SCRATCH/rt"
if [[ $SIGN_KIND == developer_id ]]; then
    codesign --verify --deep --strict -R="$REQUIREMENT" "$SCRATCH/rt/Peek.app"
else
    codesign --verify --deep --strict --verbose=2 "$SCRATCH/rt/Peek.app"
fi
[[ $(codesign_field CDHash "$SCRATCH/rt/Peek.app") == "$CDHASH" ]] \
    || die "the CDHash changed across zip and unzip" "rebuild; never extract with --norsrc"
if [[ $(xattr -lr "$SCRATCH/rt/Peek.app") == *com.apple.quarantine* ]]; then
    die "the zip restores com.apple.quarantine" "zip with --noqtn --noextattr"
fi
if [[ $NOTARIZED == true ]]; then
    spctl -a -vvv -t exec "$SCRATCH/rt/Peek.app" 2>&1 | tee "$SCRATCH/spctl.txt" >&2
    grep -q 'source=Notarized Developer ID' "$SCRATCH/spctl.txt" || die "Gatekeeper does not accept the unpacked app as Notarized Developer ID"
fi

ZIP_SHA=$(shasum -a 256 "$DIST/Peek.app.zip" | cut -d' ' -f1)
PEEKD_SHA=$(shasum -a 256 "$DIST/peekd" | cut -d' ' -f1)
BUNDLE_ID=$BUNDLE_ID BUILD=$BUILD VERSION=$VERSION INFO_TEAM=$INFO_TEAM SIGN_KIND=$SIGN_KIND \
    SIGN_IDENTITY=$SIGN_IDENTITY SIGN_HASH=$SIGN_HASH NOTARIZED=$NOTARIZED CDHASH=$CDHASH ZIP_SHA=$ZIP_SHA \
    PEEKD_SHA=$PEEKD_SHA APP_SOURCE=$APP_SOURCE python3 - "$DIST/Peek.app.build.json" <<'PY'
import datetime, json, os, sys
env = os.environ
record = {
    "bundle_id": env["BUNDLE_ID"],
    "bundle_version": env["BUILD"],
    "short_version": env["VERSION"],
    "team_id": env["INFO_TEAM"],
    "signing_kind": env["SIGN_KIND"],
    "signing_identity": env["SIGN_IDENTITY"],
    "signing_sha1": env["SIGN_HASH"],
    "notarized": env["NOTARIZED"] == "true",
    "cdhash": env["CDHASH"],
    "zip_sha256": env["ZIP_SHA"],
    "peekd_sha256": env["PEEKD_SHA"],
    "app_source": env["APP_SOURCE"],
    "built_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
}
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump(record, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY
printf '\nPeek.app %s (%s) %s build, %s\n  dist/Peek.app.zip  sha256 %s\n  dist/Peek.app.build.json\n' \
    "$VERSION" "$BUILD" "$SIGN_KIND" "$([[ $NOTARIZED == true ]] && echo notarized || echo 'not notarized')" "$ZIP_SHA"
