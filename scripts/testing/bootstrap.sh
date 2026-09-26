#!/usr/bin/env bash
# Provision (or rebuild) a peek testing environment end to end: BLUEPRINT §2.9 steps 1-9,
# notes/gap-testing-environments §4 A-H. Same code paths as production; only the secrets differ.
#
#   scripts/testing/bootstrap.sh                      dry run: prints every step, runs only [RO] checks
#   scripts/testing/bootstrap.sh --yes --participant-registered
#                                                     executes the [MUTATING] steps, skipping finished ones
#   scripts/testing/bootstrap.sh --yes --clean        honeycomb environments action clean, then rebuild
#
# Options:
#   --yes                     execute [MUTATING] steps (each is printed first, secrets never are)
#   --participant-registered  confirm the Honeycomb operator added peek to HONEYCOMB_LIFECYCLE_PARTICIPANTS
#                             (BLUEPRINT §10.1 U12). Importing before that wedges the environment permanently.
#   --name NAME               environment name (default 'peek testing')
#   --env UUID                adopt an existing environment instead of creating one
#   --webhook URL             the test Silicon's Ting webhook (default http://127.0.0.1:18777/events)
#   --clean                   wipe the environment (generation + 1) and rebuild identities, secrets, types, logins
#   --state DIR               state directory (default $PEEK_OPS/testing/<name>)
#
# Idempotent: every step checks recorded state or a read-only probe first. State (0700/0600) holds
# the environment id, its root key, both test app secrets and the Silicon token; nothing is echoed.
# Needs: the operator toolbox (scripts/operator/toolbox.sh: iam4 = iam 4.0.0, Ting 0.1.9), honeycomb
# >= 0.5.0 logged in as a tos admin (c:shubham), jq, uuidgen, and the peek CLI (PEEK_BIN, default `peek`).
# Never exports SILICON_HOME: each tool gets its home per command.
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
# shellcheck source=scripts/operator/toolbox.sh
. "$ROOT/scripts/operator/toolbox.sh"

YES=0 PARTICIPANT=0 CLEAN=0
NAME="peek testing"
ENV_ARG=""
WEBHOOK=http://127.0.0.1:18777/events
STATE=""
PEEK_BIN=${PEEK_BIN:-peek}
ORG=tos
CARBON=peek-admin
SILICON=peek-tester

# BLUEPRINT §3.1: final names and descriptions, byte-identical in every context.
TING_TYPES=(
    "peek.ask.answered|A Carbon answered a peek --ask (voice, keyboard or click)."
    "peek.ask.dismissed|A Carbon dismissed a peek --ask without answering."
    "peek.ask.expired|A peek --ask reached its --expires-in deadline unanswered."
    "peek.message.received|A Carbon spoke or typed to the Silicon from its peek with no pending ask."
    "peek.speech.finished|A peek --speak finished playing or was stopped by the Carbon."
    "peek.show.dismissed|A Carbon closed a peek --show before it retracted."
)

die() {
    printf 'bootstrap: error: %s\n' "$1" >&2
    [[ -z ${2:-} ]] || printf 'fix: %s\n' "$2" >&2
    exit 1
}
say() { printf '%s\n' "$*" >&2; }
phase() { printf '\n== %s\n' "$*" >&2; }

while (($#)); do
    case $1 in
        --yes) YES=1 ;;
        --participant-registered) PARTICIPANT=1 ;;
        --clean) CLEAN=1 ;;
        --name) NAME=${2:?--name needs a value}; shift ;;
        --env) ENV_ARG=${2:?--env needs a UUID}; shift ;;
        --webhook) WEBHOOK=${2:?--webhook needs a URL}; shift ;;
        --state) STATE=${2:?--state needs a directory}; shift ;;
        -h | --help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument $1" "see scripts/testing/bootstrap.sh --help" ;;
    esac
    shift
done

slug=$(printf '%s' "$NAME" | tr -c 'A-Za-z0-9._-' '-')
STATE=${STATE:-$PEEK_OPS/testing/$slug}
CARBON_HOME=$STATE/carbon-home   # the test Carbon's Ting session (type registration)
SIL_HOME=$STATE/silicon-home     # SILICON_HOME of the test Silicon si:peek-tester

# ---------------------------------------------------------------------------------------- helpers

# mutate "<what>" "<command as shown>" cmd…   Shows the command text (never expanded secrets);
# runs it only with --yes. In a dry run it returns 1 so callers stop dependent work.
mutate() {
    local what=$1 shown=$2
    shift 2
    say "[MUTATING] $what"
    say "  \$ $shown"
    if ((YES)); then
        "$@"
    else
        say "  (dry run; pass --yes to execute)"
        return 1
    fi
}
secret_path() { printf '%s/%s' "$STATE" "$1"; }
iam_cmd() { # iam 4.0.0 as an argv prefix, honouring PEEK_OPS_IAM_ISOLATED like iam4 does
    if [[ $PEEK_OPS_IAM_ISOLATED == 1 ]]; then
        env SILICON_IAM_HOME="$PEEK_OPS/iam" "$PEEK_OPS/cargo/bin/iam" "$@"
    else
        "$PEEK_OPS/cargo/bin/iam" "$@"
    fi
}
have() { [[ -s $(secret_path "$1") ]]; }
read_state() { cat "$(secret_path "$1")"; }
save_state() { # save_state <name> <value>: 0600, atomic
    local target tmp
    target=$(secret_path "$1")
    tmp=$(mktemp "$STATE/.tmp.XXXXXX")
    printf '%s' "$2" >"$tmp"
    chmod 600 "$tmp"
    mv -f "$tmp" "$target"
}
forget() { local name; for name in "$@"; do rm -f "$(secret_path "$name")"; done; }
json() { jq -r "$1"; }
make_private_dirs() { (umask 077 && mkdir -p "$@"); }
ensure_state_dir() {
    if [[ -d $STATE ]]; then return 0; fi
    mutate "create the private state directory" "mkdir -p -m 700 '$STATE' '$CARBON_HOME' '$SIL_HOME'" \
        make_private_dirs "$STATE" "$CARBON_HOME" "$SIL_HOME"
}
# A rotation's idempotency key is saved before the call, so a lost response is replayed with the
# same key inside IAM's 10-minute window (BLUEPRINT §2.2); after that a new key is minted.
rotation_key() {
    local file
    file=$(secret_path "$1.idempotency")
    if [[ -s $file ]] && [[ $(($(date +%s) - $(stat -f %m "$file"))) -lt 540 ]]; then
        cat "$file"
    else
        uuidgen | tr '[:upper:]' '[:lower:]' | tee "$file"
    fi
}

# Step bodies (run through mutate, so they execute only with --yes).
create_test_org() {
    local out
    out=$(iam_cmd --test "$ENV" org create "$ORG" --name "$ORG (peek test)" 2>&1) ||
        grep -qiE 'exist|conflict|409' <<<"$out" || { printf '%s\n' "$out" >&2; return 1; }
    iam_cmd --test "$ENV" config set org "$ORG" >/dev/null
}
test_plane_honeycomb_login() {
    local slt
    slt=$(iam_cmd --test "$ENV" -o json login --app-id honeycomb --grant-org "$ORG" --approve-scopes | jq -r .slt)
    [[ -n $slt && $slt != null ]] || { say "  iam4 minted no Honeycomb SLT"; return 1; }
    hc --test "$ENV" login "$slt" >/dev/null
}
carbon_ting_login() {
    iam_cmd --test "$ENV" -o json login --app-id ting --grant-org "$ORG" --approve-scopes | jq -r .slt |
        SILICON_HOME="$CARBON_HOME" IAM_TEST_APP_SECRET="$(read_state ting_ts)" IAM_TEST_KEY="$(read_state root_key)" \
            "$PEEK_TING_BIN" login --token-stdin --json >/dev/null
}
silicon_peek_login() {
    SILICON_HOME="$SIL_HOME" SILICON_ORG="$ORG" "$PEEK_BIN" --app-secret-file - login "si:$SILICON" <"$(secret_path peek_ts)"
}
silicon_ting_setup() {
    SILICON_HOME="$SIL_HOME" IAM_TEST_APP_SECRET="$(read_state ting_ts)" IAM_TEST_KEY="$(read_state root_key)" \
        "$PEEK_TING_BIN" login "si:$SILICON" --json >/dev/null &&
        SILICON_HOME="$SIL_HOME" "$PEEK_TING_BIN" org use "$ORG" --json >/dev/null &&
        SILICON_HOME="$SIL_HOME" "$PEEK_TING_BIN" webhook "$WEBHOOK" --json >/dev/null
}

# ---------------------------------------------------------------------------------------- 0. preflight

phase "0. preflight [RO]"
for tool in jq uuidgen honeycomb; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not on PATH" "install it (jq, uuidgen, Honeycomb >= 0.5.0)"
done
[[ -x $PEEK_OPS/cargo/bin/iam ]] || die "iam 4.0.0 is missing from $PEEK_OPS/cargo" \
    ". scripts/operator/toolbox.sh && peek_ops_setup --yes && peek_ops_install --yes (BLUEPRINT §5.6)"
[[ -x $PEEK_TING_BIN ]] || die "the Ting 0.1.9 CLI is missing at $PEEK_TING_BIN" "set PEEK_TING_BIN (BLUEPRINT §5.6)"
command -v "$PEEK_BIN" >/dev/null 2>&1 || die "peek CLI '$PEEK_BIN' not found" "build it and set PEEK_BIN=/abs/path/to/peek"
if env | grep -q '^SILICON_HOME='; then
    die "SILICON_HOME is exported in this shell" "unset SILICON_HOME; this script scopes it per command"
fi
login=$(hc login status --json 2>/dev/null || true)
[[ -n $login && $(jq -r '.authenticated' <<<"$login" 2>/dev/null) == true ]] || die "Honeycomb is not logged in (honeycomb login status --json)" \
    "log in as a tos admin: honeycomb login <SLT from: iam4 -o json login --app-id honeycomb --grant-org tos>"
say "  honeycomb session: $(json '.id // .actor.public_id // "unknown"' <<<"$login")"
say "  state directory:   $STATE"
((YES)) || say "  dry run: nothing below is executed; re-run with --yes"

ensure_state_dir || true

# ---------------------------------------------------------------------------------------- A. environment

phase "A. environment '$NAME' (production Carbon session)"
if [[ -n $ENV_ARG ]]; then
    [[ -d $STATE ]] && save_state env_id "$ENV_ARG"
    ENV=$ENV_ARG
elif [[ -d $STATE ]] && have env_id; then
    ENV=$(read_state env_id)
else
    ENV=""
fi
if [[ -n $ENV ]]; then
    envinfo=$(hc --json environments get "$ENV") || die "honeycomb environments get $ENV failed" "check the UUID, or pass --env with the right one"
    say "  using environment $ENV (revision $(json .revision <<<"$envinfo"), generation $(json .generation <<<"$envinfo"))"
elif out=$(mutate "create the testing environment" \
    "honeycomb --json environments create $ORG '$NAME' --description 'peek e2e: IAM+Ting+peek, disposable'" \
    hc --json environments create "$ORG" "$NAME" --description 'peek e2e: IAM+Ting+peek, disposable'); then
    ENV=$(json .environment_id <<<"$out")
    [[ $ENV =~ ^[0-9a-f-]{36}$ ]] || die "environments create returned no environment_id" "inspect: honeycomb --json environments list"
    save_state env_id "$ENV"
    envinfo=$(hc --json environments get "$ENV")
else
    ENV="<ENV>"
    envinfo='{"revision":"<R>","generation":"<G>"}'
fi

if [[ $ENV != "<ENV>" ]] && ((CLEAN)); then
    revision=$(json .revision <<<"$envinfo")
    if mutate "clean the environment: every participant wipes its data and identities (generation + 1)" \
        "honeycomb --json environments action $ENV clean --revision $revision" \
        hc --json environments action "$ENV" clean --revision "$revision" >/dev/null; then
        forget peek_ts ting_ts silicon_credential.json iam_env_key.done org.done carbon.done ting_types.done \
            peek_login.done silicon_ting.done generation
        envinfo=$(hc --json environments get "$ENV")
    fi
fi

generation=$(json .generation <<<"$envinfo")
if [[ -d $STATE ]] && have generation && [[ $(read_state generation) != "$generation" ]]; then
    say "  generation changed ($(read_state generation) → $generation): identities, secrets, types and logins are gone; rebuilding B-F"
    forget peek_ts ting_ts silicon_credential.json iam_env_key.done org.done carbon.done ting_types.done peek_login.done silicon_ting.done
fi

if [[ $ENV != "<ENV>" ]] && ! have root_key; then
    if out=$(mutate "fetch the environment root key (audited; stored 0600, never printed)" \
        "honeycomb --json environments key $ENV" hc --json environments key "$ENV"); then
        save_state root_key "$(json .testing_key <<<"$out")"
    fi
fi
if ! { [[ -d $STATE ]] && have iam_env_key.done; }; then
    # UNCERTAIN (gap-testing §11): whether `iam env key` is authorized for a Honeycomb-created environment.
    mutate "store the root key in the IAM CLI so iam4 --test works" "iam4 -o json env key $ENV" \
        iam_cmd -o json env key "$ENV" >/dev/null && save_state iam_env_key.done 1 || true
fi

# ---------------------------------------------------------------------------------------- B. identities

phase "B. test Carbon c:$CARBON owns test-plane org '$ORG', test Silicon si:$SILICON (before the import)"
if ! { [[ -d $STATE ]] && have carbon.done; }; then
    if [[ $ENV != "<ENV>" ]] && iam_cmd --test "$ENV" -o json whoami >/dev/null 2>&1; then
        save_state carbon.done 1
        say "  test Carbon session already valid"
    else
        say "  signup asks for two OTPs on a TTY; the test plane accepts 000000 for both"
        mutate "sign up the test Carbon" \
            "iam4 --test $ENV signup --email $CARBON@example.test --phone +12025550199 --carbon-id $CARBON" \
            iam_cmd --test "$ENV" signup --email "$CARBON@example.test" --phone +12025550199 --carbon-id "$CARBON" || true
        if mutate "log the test Carbon in" "iam4 --test $ENV login --carbon-id $CARBON --code 000000" \
            iam_cmd --test "$ENV" login --carbon-id "$CARBON" --code 000000; then
            save_state carbon.done 1
        fi
    fi
fi
if ! { [[ -d $STATE ]] && have org.done; }; then
    # Must precede the import so the test Carbon owns test-plane tos (gap-testing §3.5).
    if mutate "create test-plane org $ORG (an 'already exists' answer counts as done)" \
        "iam4 --test $ENV org create $ORG --name '$ORG (peek test)' && iam4 --test $ENV config set org $ORG" \
        create_test_org; then
        save_state org.done 1
    fi
fi
if ! { [[ -d $STATE ]] && have silicon_credential.json; }; then
    if out=$(mutate "create the test Silicon (its credential is shown once and stored 0600)" \
        "iam4 --test $ENV -o json silicon create $SILICON --job-description 'peek e2e test silicon' --display-name 'Peek Tester'" \
        iam_cmd --test "$ENV" -o json silicon create "$SILICON" --job-description 'peek e2e test silicon' --display-name 'Peek Tester'); then
        save_state silicon_credential.json "$out"
    fi
fi

# ---------------------------------------------------------------------------------------- C. import + secrets

phase "C. import peek (→ ting → honeycomb → briefcase) and fetch the test app secrets"
if ! ((PARTICIPANT)); then
    say "  STOP before the import: pass --participant-registered once the Honeycomb operator lists peek in"
    say "  HONEYCOMB_LIFECYCLE_PARTICIPANTS (U12). Importing an unlisted participant wedges the environment."
    ((YES)) && exit 0
    say "  (dry run continues so the rest of the plan is visible)"
fi
if [[ $ENV != "<ENV>" ]]; then
    envinfo=$(hc --json environments get "$ENV")
    peek_state=$(json '[.services[]? | select(.app_id == "peek") | .state] | first // "absent"' <<<"$envinfo")
else
    peek_state=absent
fi
if [[ $peek_state != ready ]]; then
    revision=$(json .revision <<<"$envinfo")
    if ((PARTICIPANT)) && mutate "import peek recursively (services iam, honeycomb, briefcase, ting, peek)" \
        "honeycomb --json environments import $ENV 'peek' --revision $revision" \
        hc --json environments import "$ENV" 'peek' --revision "$revision" >/dev/null; then
        for _ in $(seq 1 120); do
            envinfo=$(hc --json environments get "$ENV")
            if [[ $(json '.operation_pending' <<<"$envinfo") == false ]] &&
                [[ $(json '[.services[]? | .state] | all(. == "ready")' <<<"$envinfo") == true ]]; then
                break
            fi
            sleep 5
        done
        [[ $(json '[.services[]? | .state] | all(. == "ready")' <<<"$envinfo") == true ]] || die \
            "services are not all ready: $(json '[.services[]? | "\(.app_id)=\(.state)"] | join(", ")' <<<"$envinfo")" \
            "fix the failed participant, then: honeycomb environments action $ENV retry --revision <current>"
    fi
else
    say "  peek already imported and ready"
fi
current_generation=$(json .generation <<<"$envinfo")
if [[ -d $STATE && $current_generation =~ ^[0-9]+$ ]]; then
    save_state generation "$current_generation" # later runs detect a clean by comparing this
fi

if ! { have peek_ts && have ting_ts; }; then
    # A Carbon-driven import prints no secrets, so rotate both (gap-testing §3.1). SLTs live 2 minutes
    # and are single use; honeycomb login takes the SLT as an argument (UNCERTAIN whether '-' reads stdin).
    if mutate "test-plane Honeycomb login for c:$CARBON" \
        "honeycomb --test $ENV login \"\$(iam4 --test $ENV -o json login --app-id honeycomb --grant-org $ORG --approve-scopes | jq -r .slt)\"" \
        test_plane_honeycomb_login; then
        for app in peek ting; do
            have "${app}_ts" && continue
            rp=$(hc --test "$ENV" --json apps get "$app" | json .revision)
            key=$(rotation_key "${app}_ts")
            if out=$(mutate "rotate the $app test secret (stored 0600; replay window 10 minutes)" \
                "honeycomb --test $ENV --json --idempotency-key $key apps rotate-secret '$app' --revision $rp" \
                hc --test "$ENV" --json --idempotency-key "$key" apps rotate-secret "$app" --revision "$rp"); then
                secret=$(json '.app_secret // empty' <<<"$out")
                [[ $secret == ask_* ]] || die "apps rotate-secret $app returned state $(json .state <<<"$out") without an app_secret" \
                    "retry with the same key within 10 minutes, or: honeycomb --test $ENV operations recover-secret $(json .id <<<"$out")"
                save_state "${app}_ts" "$secret"
                forget "${app}_ts.idempotency"
            fi
        done
    fi
fi

# ---------------------------------------------------------------------------------------- D. Ting types

phase "D. register the six peek Ting types in the test context (descriptions byte-identical to production)"
if ! have ting_types.done; then
    if mutate "log the test Carbon in to Ting (test headers from the Ting test secret and root key)" \
        "iam4 --test $ENV -o json login --app-id ting --grant-org $ORG --approve-scopes | jq -r .slt | SILICON_HOME=$CARBON_HOME IAM_TEST_APP_SECRET=<ting_ts> IAM_TEST_KEY=<root_key> ting login --token-stdin" \
        carbon_ting_login; then
        registered=1
        for entry in "${TING_TYPES[@]}"; do
            ting_type=${entry%%|*} description=${entry#*|}
            SILICON_HOME="$CARBON_HOME" "$PEEK_TING_BIN" --org "$ORG" types register --type "$ting_type" --description "$description" --json >/dev/null ||
                { say "  $ting_type: registration failed (409 type_exists = a different description exists; fix it with ting types update)"; registered=0; }
        done
        SILICON_HOME="$CARBON_HOME" "$PEEK_TING_BIN" --org "$ORG" types list --app peek --json >&2
        ((registered)) && save_state ting_types.done 1
    fi
fi

# ---------------------------------------------------------------------------------------- E. peek login

phase "E. si:$SILICON logs in to peek (the backend enrolls its Ting grant)"
if ! have peek_login.done; then
    # In a test plane a public id is a valid SLT (IAM oauth.rs:966-975). The secret travels on stdin.
    if mutate "peek login as si:$SILICON with the peek test secret" \
        "printf %s <peek_ts> | SILICON_HOME=$SIL_HOME SILICON_ORG=$ORG $PEEK_BIN --app-secret-file - login si:$SILICON" \
        silicon_peek_login; then
        status=$(SILICON_HOME="$SIL_HOME" "$PEEK_BIN" --test "$ENV" login status --json)
        [[ $(json .authenticated <<<"$status") == true ]] || die "peek login status says $(jq -c . <<<"$status")" "read the stderr banner and retry E"
        [[ $(json '.ting.subscribed' <<<"$status") == true ]] || say "  warning: Ting enrollment pending; retry with: SILICON_HOME=$SIL_HOME $PEEK_BIN --test $ENV ting enroll"
        save_state peek_login.done 1
    fi
fi

# ---------------------------------------------------------------------------------------- F. Ting webhook

phase "F. si:$SILICON receives tings at $WEBHOOK"
if ! have silicon_ting.done; then
    if mutate "log si:$SILICON in to Ting, select $ORG and register its webhook" \
        "SILICON_HOME=$SIL_HOME IAM_TEST_APP_SECRET=<ting_ts> IAM_TEST_KEY=<root_key> ting login si:$SILICON && ting org use $ORG && ting webhook $WEBHOOK --json" \
        silicon_ting_setup; then
        save_state silicon_ting.done 1
    fi
fi

# ---------------------------------------------------------------------------------------- G. the loop

phase "G. the ask and the answer (run these yourself; the Carbon answers in Peek.app)"
cat >&2 <<EOF
  (a receiver must answer 204 on $WEBHOOK)
  SILICON_HOME=$SIL_HOME $PEEK_BIN --test $ENV register side 3
  SILICON_HOME=$SIL_HOME $PEEK_BIN --test $ENV register drawing ./logo.js
  SILICON_HOME=$SIL_HOME ISI=deliberate $PEEK_BIN --test $ENV send --speak 'Delete old.zip?' \\
    --ask '{"question":"Delete ~/Downloads/old.zip?","type":"single_choice","options":[{"id":"keep","label":"Keep"},{"id":"delete","label":"Delete"}]}'
  → the bubble shows "TEST · $NAME"; clicking Keep delivers peek.ask.answered to the webhook.
After 'honeycomb environments action $ENV clean', rerun: scripts/testing/bootstrap.sh --yes --participant-registered
EOF
