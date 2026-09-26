# shellcheck shell=bash
# peek operator toolbox (BLUEPRINT §5.6; notes/gap-operator-cli-readiness §7).
#
#   . scripts/operator/toolbox.sh      # bash or zsh; then: peek_ops_help
#
# Sourcing defines functions and a few unexported variables. It creates nothing, installs
# nothing, exports nothing and changes no shell option (umask changes happen in subshells).
# The only writes are the three peek_ops_* setup functions, each [MUTATING] (local files under
# $PEEK_OPS only) and each refusing to run without --yes.
#
# Hazards this toolbox exists to contain (BLUEPRINT §10.2 item 15):
#   * never `export SILICON_HOME`: it re-roots IAM, Honeycomb, Space Station and Ting at once;
#   * every Honeycomb call runs with HONEYCOMB_AUTO_UPDATE=0 (use `hc`), or it runs a package-update pass;
#   * never run `ting-daemon --version` on 0.1.2 or 0.1.5: those builds start a daemon instead;
#   * iam 3.1.2 refreshes rewrite ~/.silicon-iam/credentials.json and cannot address bare app IDs.

if ! (return 0 2>/dev/null); then
    printf 'toolbox.sh defines shell functions; source it instead of running it:\n  . %s\n' "$0" >&2
    exit 2
fi

PEEK_OPS=${PEEK_OPS:-$HOME/.peek-operator}
PEEK_TING_BIN=${PEEK_TING_BIN:-$HOME/shubham/unlikefraction/tos/silicon-ting/dist/check-0.1.9/ting}
PEEK_TING_SHA256=72223c0942b9e0344b74b82fe12fa942ca88a40562cc942659577d5563c1fd20
PEEK_IAM_CLI_VERSION=4.0.0
PEEK_SPACESTATION_CLI_VERSION=0.2.0
# Set PEEK_OPS_IAM_ISOLATED=1 to give iam4 its own store ($PEEK_OPS/iam) and never touch
# ~/.silicon-iam; it then needs one OTP login: iam4 login --carbon-id c:shubham.
PEEK_OPS_IAM_ISOLATED=${PEEK_OPS_IAM_ISOLATED:-0}

peek_ops_help() {
    cat >&2 <<EOF
peek operator toolbox (PEEK_OPS=$PEEK_OPS)
  [RO]        peek_ops_verify             versions, Ting binary checksum, Honeycomb session, SILICON_HOME guard
  [MUTATING]  peek_ops_setup --yes        create \$PEEK_OPS/{backups,ting-home,iam,cargo} (0700)
  [MUTATING]  peek_ops_backup_iam --yes   copy ~/.silicon-iam to \$PEEK_OPS/backups/silicon-iam-<UTC>
  [MUTATING]  peek_ops_install --yes      cargo install silicon-iam-cli $PEEK_IAM_CLI_VERSION and space-station-cli
                                          $PEEK_SPACESTATION_CLI_VERSION into \$PEEK_OPS/cargo (nothing global)
  wrappers    iam4 …    iam $PEEK_IAM_CLI_VERSION ($( [ "$PEEK_OPS_IAM_ISOLATED" = 1 ] && echo "isolated store \$PEEK_OPS/iam" || echo "shared ~/.silicon-iam, backed up first"))
              tingop …  ting 0.1.9 with SILICON_HOME=\$PEEK_OPS/ting-home for this call only
              ss2 …     spacestation $PEEK_SPACESTATION_CLI_VERSION
              hc …      honeycomb with HONEYCOMB_AUTO_UPDATE=0
Never export SILICON_HOME. Never run ting-daemon --version on 0.1.2/0.1.5.
Rollout order and every command: deploy/README.md.
EOF
}

_peek_ops_confirmed() {
    # _peek_ops_confirmed <function> <first arg> <plan…>: print the plan; true only with --yes.
    local name=$1 flag=$2
    shift 2
    printf '[MUTATING] %s will run:\n' "$name" >&2
    printf '  %s\n' "$@" >&2
    if [ "$flag" != "--yes" ]; then
        printf 'Not run. Re-run as: %s --yes\n' "$name" >&2
        return 1
    fi
}

peek_ops_setup() {
    _peek_ops_confirmed peek_ops_setup "${1:-}" \
        "umask 077; mkdir -p '$PEEK_OPS'/{backups,ting-home,iam,cargo}; chmod 700 '$PEEK_OPS' '$PEEK_OPS'/*" || return 1
    (
        umask 077
        mkdir -p "$PEEK_OPS/backups" "$PEEK_OPS/ting-home" "$PEEK_OPS/iam" "$PEEK_OPS/cargo" &&
            chmod 700 "$PEEK_OPS" "$PEEK_OPS/backups" "$PEEK_OPS/ting-home" "$PEEK_OPS/iam" "$PEEK_OPS/cargo"
    ) && printf 'peek_ops_setup: %s ready\n' "$PEEK_OPS" >&2
}

peek_ops_backup_iam() {
    local stamp destination
    stamp=$(date -u +%Y%m%dT%H%M%SZ)
    destination="$PEEK_OPS/backups/silicon-iam-$stamp"
    _peek_ops_confirmed peek_ops_backup_iam "${1:-}" "cp -Rp ~/.silicon-iam '$destination'" || return 1
    [ -d "$HOME/.silicon-iam" ] || { printf 'peek_ops_backup_iam: %s/.silicon-iam does not exist; nothing to back up\n' "$HOME" >&2; return 1; }
    [ -d "$PEEK_OPS/backups" ] || { printf 'peek_ops_backup_iam: run peek_ops_setup --yes first\n' >&2; return 1; }
    ( umask 077 && cp -Rp "$HOME/.silicon-iam" "$destination" ) && printf 'peek_ops_backup_iam: saved %s\n' "$destination" >&2
}

peek_ops_install() {
    _peek_ops_confirmed peek_ops_install "${1:-}" \
        "cargo install silicon-iam-cli --version $PEEK_IAM_CLI_VERSION --locked --root '$PEEK_OPS/cargo'" \
        "cargo install space-station-cli --version $PEEK_SPACESTATION_CLI_VERSION --locked --root '$PEEK_OPS/cargo'" || return 1
    [ -d "$PEEK_OPS/cargo" ] || { printf 'peek_ops_install: run peek_ops_setup --yes first\n' >&2; return 1; }
    cargo install silicon-iam-cli --version "$PEEK_IAM_CLI_VERSION" --locked --root "$PEEK_OPS/cargo" &&
        cargo install space-station-cli --version "$PEEK_SPACESTATION_CLI_VERSION" --locked --root "$PEEK_OPS/cargo"
}

iam4() {
    if [ ! -x "$PEEK_OPS/cargo/bin/iam" ]; then
        printf 'iam4: %s/cargo/bin/iam is missing; run peek_ops_install --yes (BLUEPRINT §5.6)\n' "$PEEK_OPS" >&2
        return 127
    fi
    if [ "$PEEK_OPS_IAM_ISOLATED" = 1 ]; then
        SILICON_IAM_HOME="$PEEK_OPS/iam" "$PEEK_OPS/cargo/bin/iam" "$@"
    else
        "$PEEK_OPS/cargo/bin/iam" "$@"
    fi
}

tingop() {
    if [ ! -x "$PEEK_TING_BIN" ]; then
        printf 'tingop: %s is missing; set PEEK_TING_BIN, or run: cargo install silicon-ting-cli --version 0.1.9 --locked --root %s/cargo, then PEEK_TING_BIN=%s/cargo/bin/ting\n' \
            "$PEEK_TING_BIN" "$PEEK_OPS" "$PEEK_OPS" >&2
        return 127
    fi
    SILICON_HOME="$PEEK_OPS/ting-home" "$PEEK_TING_BIN" "$@"
}

ss2() {
    if [ ! -x "$PEEK_OPS/cargo/bin/spacestation" ]; then
        printf 'ss2: %s/cargo/bin/spacestation is missing; run peek_ops_install --yes\n' "$PEEK_OPS" >&2
        return 127
    fi
    "$PEEK_OPS/cargo/bin/spacestation" "$@"
}

hc() {
    HONEYCOMB_AUTO_UPDATE=0 honeycomb "$@"
}

peek_ops_verify() {
    local status=0 digest
    printf '[RO] peek operator toolbox checks\n' >&2
    if [ -n "${SILICON_HOME+x}" ] && env | grep -q '^SILICON_HOME='; then
        printf '  FAIL SILICON_HOME is exported (%s); run: unset SILICON_HOME\n' "$SILICON_HOME" >&2
        status=1
    else
        printf '  ok   SILICON_HOME is not exported\n' >&2
    fi
    if [ -x "$PEEK_TING_BIN" ]; then
        digest=$(shasum -a 256 "$PEEK_TING_BIN" | cut -d' ' -f1)
        if [ "$digest" = "$PEEK_TING_SHA256" ]; then
            printf '  ok   %s sha256 matches check-0.1.9\n' "$PEEK_TING_BIN" >&2
        else
            printf '  FAIL %s sha256 %s, expected %s\n' "$PEEK_TING_BIN" "$digest" "$PEEK_TING_SHA256" >&2
            status=1
        fi
        printf '       tingop --version: %s\n' "$(tingop --version 2>&1)" >&2
    else
        printf '  FAIL Ting binary %s is missing\n' "$PEEK_TING_BIN" >&2
        status=1
    fi
    if [ -x "$PEEK_OPS/cargo/bin/iam" ]; then
        printf '       iam4 --version: %s\n' "$(iam4 --version 2>&1)" >&2
    else
        printf '  FAIL iam %s not installed in %s/cargo (peek_ops_install --yes)\n' "$PEEK_IAM_CLI_VERSION" "$PEEK_OPS" >&2
        status=1
    fi
    if [ -x "$PEEK_OPS/cargo/bin/spacestation" ]; then
        printf '       ss2 --version: %s\n' "$(ss2 --version 2>&1)" >&2
    else
        printf '  FAIL spacestation %s not installed in %s/cargo\n' "$PEEK_SPACESTATION_CLI_VERSION" "$PEEK_OPS" >&2
        status=1
    fi
    if command -v honeycomb >/dev/null 2>&1; then
        printf '       honeycomb --version: %s\n' "$(hc --version 2>&1)" >&2
        printf '       honeycomb login status: %s\n' "$(hc login status --json 2>&1)" >&2
    else
        printf '  FAIL honeycomb is not on PATH\n' >&2
        status=1
    fi
    return "$status"
}
