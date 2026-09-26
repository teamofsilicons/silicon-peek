# shellcheck shell=bash
# Resolve the codesigning identity for release builds (sourced by build-*-release.sh).
#
# Input:  SIGN_IDENTITY  unset/empty: the first "Developer ID Application: … (<team>)" identity,
#                        else the first "Apple Development: …" identity in the keychain search list;
#                        or a full identity name; or its 40-hex SHA-1.
#         PEEK_TEAM_ID   the Apple team (default LTBSK59BJ2, BLUEPRINT D23).
#         PEEK_SIGN_TIMESTAMP  secure (default) | none: see signing_timestamp_flags below.
# Output: SIGN_IDENTITY  the resolved identity name
#         SIGN_HASH      its SHA-1 (what codesign is given, so equal names never collide)
#         SIGN_KIND      developer_id | development
# Developer ID builds ship as ai.tos.peek with team_id=<team>; Apple Development builds ship as
# ai.tos.peek.dev with an empty team_id, for tos-internal dev-channel releases only (BLUEPRINT §5.5).
# Ad-hoc signing ("-") is always refused: its cdhash changes every build and resets TCC (§8.10).

resolve_signing_identity() {
    local team=${PEEK_TEAM_ID:-LTBSK59BJ2} requested=${SIGN_IDENTITY:-} listing line hash name
    local -a hashes=() names=()
    if [[ $requested == "-" ]]; then
        printf 'signing: error: SIGN_IDENTITY=- (ad-hoc) is refused; ad-hoc cdhashes change on every build and reset TCC grants\n' >&2
        printf 'fix: install the Developer ID Application certificate for team %s, or use your Apple Development identity\n' "$team" >&2
        return 1
    fi
    listing=$(security find-identity -v -p codesigning 2>/dev/null || true)
    while IFS= read -r line; do
        # `  1) 8A3D6A2B…40 hex "Apple Development: Name (ABCDE12345)"`
        if [[ $line =~ ^[[:space:]]*[0-9]+\)[[:space:]]+([0-9A-F]{40})[[:space:]]+\"(.*)\"$ ]]; then
            hashes+=("${BASH_REMATCH[1]}")
            names+=("${BASH_REMATCH[2]}")
        fi
    done <<<"$listing"
    hash="" name=""
    local index
    if [[ -z $requested ]]; then
        for index in "${!names[@]}"; do
            if [[ ${names[$index]} == "Developer ID Application:"*"($team)" ]]; then
                hash=${hashes[$index]} name=${names[$index]}
                break
            fi
        done
        if [[ -z $hash ]]; then
            for index in "${!names[@]}"; do
                if [[ ${names[$index]} == "Apple Development:"* ]]; then
                    hash=${hashes[$index]} name=${names[$index]}
                    break
                fi
            done
        fi
    else
        for index in "${!names[@]}"; do
            if [[ ${hashes[$index]} == "$(tr '[:lower:]' '[:upper:]' <<<"$requested")" || ${names[$index]} == "$requested" ]]; then
                hash=${hashes[$index]} name=${names[$index]}
                break
            fi
        done
    fi
    if [[ -z $hash ]]; then
        printf 'signing: error: no usable codesigning identity %s\n' "${requested:+matching \"$requested\" }was found in the keychain search list" >&2
        printf 'fix: security find-identity -v -p codesigning lists what exists; Developer ID needs the account holder (Xcode → Settings → Accounts → Manage Certificates → + → Developer ID Application, BLUEPRINT §8.10)\n' >&2
        return 1
    fi
    case $name in
        "Developer ID Application:"*) SIGN_KIND=developer_id ;;
        "Apple Development:"*) SIGN_KIND=development ;;
        *)
            printf 'signing: error: "%s" is neither a Developer ID Application nor an Apple Development identity\n' "$name" >&2
            printf 'fix: peek ships Developer ID builds (prod) or Apple Development builds (dev channel) only\n' >&2
            return 1
            ;;
    esac
    SIGN_IDENTITY=$name
    SIGN_HASH=$hash
    export SIGN_IDENTITY SIGN_HASH SIGN_KIND
    printf 'signing: %s identity "%s" (%s)\n' "$SIGN_KIND" "$SIGN_IDENTITY" "$SIGN_HASH" >&2
}

# Sets the array SIGN_TIMESTAMP to the codesign timestamp flag for SIGN_KIND (call after
# resolve_signing_identity). Every release signature carries Apple's secure timestamp, Apple
# Development included (dev-channel builds get the same hardened runtime + timestamp shape as
# Developer ID ones). PEEK_SIGN_TIMESTAMP=none drops it for an offline development build only;
# Developer ID code always needs it (notarization rejects signatures without one).
# shellcheck disable=SC2034  # SIGN_TIMESTAMP is read by the script that sources this file
signing_timestamp_flags() {
    case ${PEEK_SIGN_TIMESTAMP:-secure} in
        secure) SIGN_TIMESTAMP=(--timestamp) ;;
        none)
            if [[ $SIGN_KIND == developer_id ]]; then
                printf 'signing: error: PEEK_SIGN_TIMESTAMP=none is refused for Developer ID code; notarization requires a secure timestamp\n' >&2
                printf 'fix: unset PEEK_SIGN_TIMESTAMP (the timestamp server must be reachable)\n' >&2
                return 1
            fi
            SIGN_TIMESTAMP=(--timestamp=none)
            ;;
        *)
            printf 'signing: error: PEEK_SIGN_TIMESTAMP=%s\n' "$PEEK_SIGN_TIMESTAMP" >&2
            printf 'fix: use secure (default) or none (offline development builds only)\n' >&2
            return 1
            ;;
    esac
}
