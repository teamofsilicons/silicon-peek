#!/bin/sh
# Check that the installer served at https://peek.teamofsilicons.com/install.sh
# (web/public/install.sh) is byte-identical to the canonical scripts/install.sh
# (BLUEPRINT §9.4), parses, and still guards against partial downloads.
# Usage: scripts/check-installer.sh        (exit 0 = identical; CI runs it on every push)
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
canonical="$root/scripts/install.sh"
served="$root/web/public/install.sh"

fail() {
    printf 'check-installer: %s\n' "$1" >&2
    [ -z "${2:-}" ] || printf 'fix: %s\n' "$2" >&2
    exit 1
}

for file in "$canonical" "$served"; do
    [ -f "$file" ] && [ ! -L "$file" ] || fail "$file is missing or a symlink" \
        "restore it from BLUEPRINT §9.4; both copies must be regular files"
done

if ! cmp -s "$canonical" "$served"; then
    diff -u "$canonical" "$served" >&2 || true
    fail "web/public/install.sh differs from scripts/install.sh (diff above)" \
        "edit scripts/install.sh, then: cp scripts/install.sh web/public/install.sh"
fi

sh -n "$canonical" || fail "scripts/install.sh does not parse with sh -n" "fix the syntax error reported above"

[ "$(head -n 1 "$canonical")" = '#!/bin/sh' ] || fail "scripts/install.sh must start with #!/bin/sh" \
    "keep the POSIX shebang: the script is piped into sh"
# Everything lives in install_peek(); only the last line runs it, so a truncated
# download ends inside the function body and executes nothing.
[ "$(tail -n 1 "$canonical")" = 'install_peek "$@"' ] || fail \
    "the last line of scripts/install.sh must be: install_peek \"\$@\"" \
    "keep the function wrapper so partial downloads never run"

digest=$(shasum -a 256 "$canonical" 2>/dev/null || sha256sum "$canonical")
printf 'install.sh: web/public and scripts copies are identical (sha256 %s)\n' "${digest%% *}"
