#!/bin/sh
# peek install hook: apps.yaml targets.macos-*.install_script for Silicon Apps.
#
# Contract
#   * Always exits 0. A failed step never fails `silicon-apps install`, so it never fails
#     `silicon connect` (Stemcell treats a non-zero Silicon Apps exit as a failed
#     connection). Each step is reported on stderr as
#       peek install-app: [ok|skip|degraded] <step>: <detail>
#     and appended to "$SUPPORT/install-status.txt", because Stemcell discards
#     Silicon Apps's stderr when the install succeeds. `peek app status` shows that file.
#   * Idempotent. Silicon Apps runs it after every `silicon-apps install` that changes the
#     installed version, including updates.
#   * Never blocks and never holds the caller's pipes: no prompts (stdin is /dev/null
#     under Stemcell), no network, bounded waits, no child left running on our fds.
#   * Works with PATH removed (Stemcell) and without an Aqua session (SSH, CI).
#   * Never replaces an existing Peek.app. It installs the first copy only; otherwise
#     it offers the bundled build and Peek.app updates itself (same Team ID, quit first).
#   * Serialized with the peek CLI and Peek.app through flock(2) on
#     "$SUPPORT/install.lock" (lockf(1) takes the same BSD lock as Rust File::lock).
# Local overrides: PEEK_INSTALL_APPLICATIONS_DIR, PEEK_INSTALL_SUPPORT_DIR, PEEK_INSTALL_NO_LAUNCH=1.
#
# Referenced by apps.yaml Manifest A (install_script on both macOS targets, peek >= 0.1.2):
# `silicon-apps install peek` runs it, so Peek.app is installed (first copy only, Developer ID
# verified) and launched right away. Silicon Apps owns network downloads; peekd applies
# the locally bundled signed offer. The peek CLI's ensure_app()
# performs the same steps as the fallback for `peek login` and every Mac command.
# Reviewed copy of the version tested in notes/gap-silicon-apps-app-distribution §3.5, with two
# hardening changes: the watchdog in bounded() also stops its own sleep, so no process outlives
# a finished command, and the final rename re-checks that no Peek.app appeared meanwhile, so the
# staged bundle can never be moved *into* an existing one.
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 077

say() {
    printf 'peek install-app: [%s] %s: %s\n' "$1" "$2" "$3" >&2
    [ -n "${STATUS_FILE:-}" ] && printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$STATUS_FILE" 2>/dev/null
    return 0
}

# bounded SECONDS CMD... : run CMD; TERM it after SECONDS. The watchdog holds no caller fds.
bounded() {
    _secs=$1; shift
    "$@" &
    _pid=$!
    (
        exec </dev/null >/dev/null 2>&1
        _nap=
        trap '[ -n "$_nap" ] && /bin/kill -TERM "$_nap" 2>/dev/null; exit 0' TERM
        /bin/sleep "$_secs" &
        _nap=$!
        wait "$_nap" && /bin/kill -TERM "$_pid"
    ) &
    _dog=$!
    wait "$_pid"; _status=$?
    /bin/kill -TERM "$_dog" 2>/dev/null; wait "$_dog" 2>/dev/null
    return "$_status"
}

resolve() {
    user=$(/usr/bin/id -un 2>/dev/null)
    # One Peek.app per OS user: the account's real home, never SILICON_HOME.
    real_home=$(/usr/bin/dscl /Search -read "/Users/$user" NFSHomeDirectory 2>/dev/null \
        | /usr/bin/sed -n 's/^NFSHomeDirectory: //p')
    if [ -z "$real_home" ] || [ ! -d "$real_home" ]; then real_home=${HOME:-}; fi
    if [ -z "$real_home" ] || [ ! -d "$real_home" ]; then
        say degraded home "cannot resolve the home of user '$user'"
        return 1
    fi
    apps=${PEEK_INSTALL_APPLICATIONS_DIR:-"$real_home/Applications"}
    support=${PEEK_INSTALL_SUPPORT_DIR:-"$real_home/Library/Application Support/Peek"}
    app="$apps/Peek.app"
    offers="$support/offers"
    pkg=$(cd "$(/usr/bin/dirname "$0")" && pwd -P)
    zip="$pkg/Peek.app.zip"
    info="$pkg/Peek.app.info"
    # shellcheck disable=SC2015  # either step failing means degraded; say always succeeds
    /bin/mkdir -p "$offers" 2>/dev/null && /bin/chmod 700 "$support" "$offers" 2>/dev/null \
        || { say degraded support-dir "cannot create $offers"; return 1; }
    STATUS_FILE="$support/install-status.txt"
    size=$(/usr/bin/stat -f %z "$STATUS_FILE" 2>/dev/null || echo 0)
    [ "$size" -gt 65536 ] 2>/dev/null && : > "$STATUS_FILE"
    return 0
}

field() { /usr/bin/sed -n "s/^$1=//p" "$info" | /usr/bin/head -n 1; }

install_locked() {
    if [ ! -f "$zip" ] || [ ! -f "$info" ]; then
        say degraded payload "missing $zip or $info"
        return 0
    fi
    bundle_id=$(field bundle_id); build=$(field bundle_version); short=$(field short_version)
    team=$(field team_id); zsha=$(field zip_sha256); min_os=$(field minimum_system_version)
    case "$build" in ''|*[!0-9]*) say degraded payload "bundle_version '$build' in $info is not an integer"; return 0 ;; esac
    os=$(/usr/bin/sw_vers -productVersion 2>/dev/null)
    if [ -n "$min_os" ] && [ -n "$os" ] && [ "${os%%.*}" -lt "${min_os%%.*}" ] 2>/dev/null; then
        say degraded os "macOS $os is older than Peek's minimum $min_os; the peek CLI still handles accounts/login/config"
        return 0
    fi

    # 1. Offer this build (newest wins; Peek.app and every peek CLI read the offers).
    offer="$offers/$build-$(printf %.12s "$zsha").app.zip"
    if [ -f "$offer" ] && [ -f "${offer%.zip}.info" ]; then
        say ok offer "build $build is already offered at $offer"
    elif /bin/cp "$zip" "$offer.tmp.$$" 2>/dev/null && /bin/cp "$info" "${offer%.zip}.info.tmp.$$" 2>/dev/null \
        && /bin/mv -f "${offer%.zip}.info.tmp.$$" "${offer%.zip}.info" && /bin/mv -f "$offer.tmp.$$" "$offer"; then
        say ok offer "offered build $build ($short) at $offer"
    else
        /bin/rm -f "$offer.tmp.$$" "${offer%.zip}.info.tmp.$$" 2>/dev/null
        say degraded offer "cannot write $offer"
    fi

    # 2. First install only. An existing Peek.app applies newer offers itself.
    if [ -e "$app" ] || [ -L "$app" ]; then
        have=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$app/Contents/Info.plist" 2>/dev/null)
        say skip install "Peek.app build ${have:-unknown} is already at $app; it applies newer offers itself"
        return 0
    fi
    /bin/mkdir -p "$apps" 2>/dev/null || { say degraded install "cannot create $apps"; return 0; }
    stage="$apps/.Peek.app.install.$$"
    /bin/rm -rf "$stage" 2>/dev/null
    # Plain `ditto -x -k`: never --norsrc, which writes AppleDouble ._ files into the
    # bundle when the zip has them ("file added" -> invalid signature).
    if ! bounded 60 /usr/bin/ditto -x -k "$zip" "$stage" 2>/dev/null; then
        /bin/rm -rf "$stage"; say degraded install "ditto could not unpack $zip"; return 0
    fi
    got_id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$stage/Peek.app/Contents/Info.plist" 2>/dev/null)
    if [ -n "$team" ]; then
        # Developer ID Application leaf + Developer ID CA, this team, this bundle id.
        req="anchor apple generic and identifier \"$bundle_id\" and certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = \"$team\""
        bounded 60 /usr/bin/codesign --verify --deep --strict -R="$req" "$stage/Peek.app" 2>/dev/null
    else
        bounded 60 /usr/bin/codesign --verify --deep --strict "$stage/Peek.app" 2>/dev/null   # dev/ad-hoc builds only
    fi
    verified=$?
    if [ "$verified" -ne 0 ] || [ "$got_id" != "$bundle_id" ]; then
        got_team=$(/usr/bin/codesign -dv "$stage/Peek.app" 2>&1 | /usr/bin/sed -n 's/^TeamIdentifier=//p')
        /bin/rm -rf "$stage"
        say degraded install "unpacked bundle (id '$got_id', team '$got_team') fails codesign --verify --deep --strict for id '$bundle_id' team '${team:-any}' (status $verified)"
        return 0
    fi
    # Verified as ours. A zip made without --norsrc on a machine where the bundle was
    # quarantined carries com.apple.quarantine in AppleDouble entries, and
    # `ditto -x -k --noqtn` still restores it (tested), which would stop a hidden launch
    # at the Gatekeeper first-open dialog.
    /usr/bin/xattr -dr com.apple.quarantine "$stage/Peek.app" 2>/dev/null
    # $app is absent and we hold the lock, so this mv is a same-volume rename(2), not a move-into.
    # Re-check anyway: a process that ignores install.lock must never make us nest the bundle.
    if [ -e "$app" ] || [ -L "$app" ]; then
        /bin/rm -rf "$stage"
        say skip install "a Peek.app appeared at $app while unpacking; it applies newer offers itself"
        return 0
    fi
    if /bin/mv "$stage/Peek.app" "$app" 2>/dev/null; then
        say ok install "installed Peek.app $short (build $build) at $app"
    else
        say degraded install "cannot rename $stage/Peek.app to $app"
    fi
    /bin/rm -rf "$stage" 2>/dev/null
    return 0
}

launch() {
    # The app registers its login item and agent with SMAppService, applies newer
    # offers, and serves the per-user socket. Launching is only a head start:
    # every peek command that needs the app starts it on demand. It runs whether
    # the app was installed just now or already: `open` starts a Peek.app that is
    # not running and is a no-op for one that is.
    if [ "${PEEK_INSTALL_NO_LAUNCH:-0}" = 1 ]; then
        say skip launch "PEEK_INSTALL_NO_LAUNCH=1"; return 0
    fi
    [ -d "$app" ] || { say skip launch "no $app"; return 0; }
    manager=$(/bin/launchctl managername 2>&1)
    if [ "$manager" != Aqua ]; then
        say degraded launch "this process tree has no Aqua session (launchctl managername: $manager); peek starts Peek.app on demand from a GUI session"
        return 0
    fi
    if out=$(bounded 15 /usr/bin/open -g -j "$app" --args --launched-by install-script 2>&1); then
        say ok launch "asked LaunchServices to open $app in the background"
    else
        say degraded launch "open -g $app failed: $out; peek retries on demand"
    fi
    return 0
}

main() {
    [ "$(/usr/bin/uname -s)" = Darwin ] || { say skip platform "not macOS; nothing to install"; return 0; }
    resolve || return 0
    if [ "${PEEK_INSTALL_LOCKED:-0}" = 1 ]; then
        install_locked
        return 0
    fi
    printf 'run\t%s\t%s %s from %s\n' "$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)" \
        "${APPS_APP_ID:-peek}" "${APPS_APP_VERSION:-?}" "$pkg" >> "$STATUS_FILE" 2>/dev/null
    ( PEEK_INSTALL_LOCKED=1; export PEEK_INSTALL_LOCKED
      bounded 150 /usr/bin/lockf -s -k -t 30 "$support/install.lock" /bin/sh "$0" "$@" </dev/null )
    locked=$?
    case "$locked" in
        0) ;;
        75) say degraded lock "$support/install.lock stayed busy for 30 s; peek applies the offer on its next start" ;;
        *) say degraded install "locked install step ended with status $locked" ;;
    esac
    launch
    return 0
}

( main "$@" )
exit 0
