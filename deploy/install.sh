#!/bin/bash
# Install one peek-server release on the production host. Runs as root through SSM, started by
# deploy/deploy.py after it downloaded and checksum-verified the release archive (BLUEPRINT §5.3).
#
#   install.sh <release-dir> <region> <artifact-bucket> [--require-tls]
#
# Steps: take the deploy lock → back up every managed file → render /etc/peek/runtime.env (0600)
# from Secrets Manager → install Caddy, the Caddyfile and the units → caddy validate → flip
# /opt/peek/current atomically → restart peek-server → local /healthz must report this version →
# reload Caddy → HTTPS check through Caddy (required with --require-tls; a warning before DNS and
# the first certificate exist) → start the backup timer → prune old releases.
# Any failure restores the previous release, configuration, Caddy binary and units, and reports
# whether that restore really worked.
set -euo pipefail

release=${1:?usage: install.sh <release-dir> <region> <artifact-bucket> [--require-tls]}
region=${2:?region required}
bucket=${3:?artifact bucket required}
require_tls=0
[[ ${4:-} != --require-tls ]] || require_tls=1
host=backend.peek.teamofsilicons.com
secret_id=silicon-peek/production/runtime
umask 077

die() { printf 'peek install: error: %s\n' "$*" >&2; exit 1; }

exec 9>/var/lock/peek-deploy.lock
flock -n 9 || die "another deployment holds /var/lock/peek-deploy.lock; wait for it (aws ssm list-commands) and retry"

release=$(readlink -e "$release") || die "release directory $1 does not exist"
[[ $release == /opt/peek/releases/* ]] || die "releases must live under /opt/peek/releases (got $release)"
for file in peek-server install.sh render-env.py backup.py release.json peek-server.service caddy.service \
    Caddyfile peek-backup.service peek-backup.timer; do
    [[ -f $release/$file ]] || die "the release lacks $file; rebuild it with deploy/deploy.py"
done
expected_version=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$release/release.json")

current_link=/opt/peek/current
local_healthz=http://127.0.0.1:8080/healthz
previous=$(readlink -e "$current_link" || true)
backup=$(mktemp -d /var/tmp/peek-deploy-backup.XXXXXX)
database_path=""
managed=(
    /etc/peek/runtime.env /etc/peek/backup.env /etc/caddy/Caddyfile /usr/local/bin/caddy
    /etc/systemd/system/peek-server.service /etc/systemd/system/caddy.service
    /etc/systemd/system/peek-backup.service /etc/systemd/system/peek-backup.timer
)
for target in "${managed[@]}"; do
    if [[ -f $target ]]; then cp -p "$target" "$backup/$(basename "$target")"; fi
done

# --- rollback ---
# Puts a saved file back without ever writing through the file in place: a copy next to it, then
# an atomic rename. Writing into a running executable (Caddy) fails with ETXTBSY, and a restore
# interrupted halfway must not leave a truncated file behind.
restore_file() {
    local saved=$1 target=$2
    cp -p "$saved" "$target.peek-rollback" && mv -f "$target.peek-rollback" "$target"
}

# The EXIT trap: on failure, restore every managed file, the previous release and its services.
# No step may abort the others (errexit is off inside), every failed step is reported, and the
# outcome printed last is the real one: "restored <release>" only when the previous release
# answers /healthz again, "ROLLBACK INCOMPLETE" otherwise (deploy.py reads these lines).
rollback() {
    local status=$?
    trap - EXIT
    if [[ $status != 0 ]]; then
        set +e
        local incomplete=0 target saved
        printf 'peek install: failed (status %s); restoring the previous state\n' "$status" >&2
        systemctl stop peek-server
        if [[ -n $database_path && -f $backup/peek.sqlite ]]; then
            rm -f "$database_path-wal" "$database_path-shm"
            if ! restore_file "$backup/peek.sqlite" "$database_path"; then
                printf 'peek install: rollback: could not restore the database\n' >&2
                incomplete=1
            fi
        fi
        for target in "${managed[@]}"; do
            saved=$backup/$(basename "$target")
            if [[ -f $saved ]]; then
                # Unchanged files (typically the running Caddy binary) are left alone.
                cmp -s "$saved" "$target" && continue
                if ! restore_file "$saved" "$target"; then
                    printf 'peek install: rollback: could not restore %s\n' "$target" >&2
                    rm -f "$target.peek-rollback"
                    incomplete=1
                fi
            elif ! rm -f "$target"; then
                printf 'peek install: rollback: could not remove %s\n' "$target" >&2
                incomplete=1
            fi
        done
        if [[ -n $previous ]]; then
            if ! { ln -sfn "$previous" "$current_link.next" && mv -Tf "$current_link.next" "$current_link"; }; then
                printf 'peek install: rollback: could not point %s back at %s\n' "$current_link" "$previous" >&2
                incomplete=1
            fi
        fi
        systemctl daemon-reload || incomplete=1
        if [[ -n $previous ]]; then
            systemctl restart peek-server || incomplete=1
            systemctl reload-or-restart caddy || incomplete=1
            local answered=0
            for _ in {1..15}; do
                if curl -fsS --max-time 2 "$local_healthz" >/dev/null 2>&1; then
                    answered=1
                    break
                fi
                sleep 2
            done
            if ((!answered)); then
                printf 'peek install: rollback: the previous release does not answer %s\n' "$local_healthz" >&2
                incomplete=1
            fi
            if ((incomplete)); then
                printf 'peek install: ROLLBACK INCOMPLETE: production may be down; recover by hand (journalctl -u peek-server -u caddy; previous release %s)\n' "$previous" >&2
            else
                printf 'peek install: restored %s\n' "$previous" >&2
            fi
        else
            printf 'peek install: no previous release existed; peek-server stays stopped\n' >&2
        fi
    fi
    rm -rf "$backup"
    exit "$status"
}
# --- end rollback ---
trap rollback EXIT

# Idempotent host layout (the stack's UserData creates the same on first boot).
id peek >/dev/null 2>&1 || useradd --system --home-dir /var/lib/peek --shell /sbin/nologin peek
id caddy >/dev/null 2>&1 || useradd --system --home-dir /var/lib/caddy --shell /sbin/nologin caddy
install -d -o peek -g peek -m 0700 /var/lib/peek /var/lib/peek/telemetry
install -d -o caddy -g caddy -m 0700 /var/lib/caddy
install -d -o root -g root -m 0700 /etc/peek
install -d -m 0755 /opt/peek/releases /etc/caddy

# Runtime configuration: the secret goes through a pipe, never argv, and is validated first.
aws secretsmanager get-secret-value --region "$region" --secret-id "$secret_id" --query SecretString --output text \
    | python3 "$release/render-env.py" --output /etc/peek/runtime.env
chown root:root /etc/peek/runtime.env
chmod 0600 /etc/peek/runtime.env
printf 'PEEK_BACKUP_BUCKET=%s\nPEEK_BACKUP_REGION=%s\n' "$bucket" "$region" >/etc/peek/backup.env
chmod 0600 /etc/peek/backup.env

if [[ -f $release/caddy ]]; then
    install -m 0755 "$release/caddy" /usr/local/bin/caddy
fi
[[ -x /usr/local/bin/caddy ]] || die "no Caddy binary is installed; the first deploy must pass --caddy to deploy/deploy.py"
install -m 0644 "$release/Caddyfile" /etc/caddy/Caddyfile
for unit in peek-server.service caddy.service peek-backup.service peek-backup.timer; do
    install -m 0644 "$release/$unit" "/etc/systemd/system/$unit"
done
/usr/local/bin/caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
chmod 0755 "$release" "$release/peek-server"

# Freeze writes and snapshot SQLite before the new binary can migrate its schema.
# Rollback restores this snapshot before restarting the previous binary.
database_path=$(python3 - <<'PYDB'
import pathlib, shlex
values = dict(line.split("=", 1) for line in pathlib.Path('/etc/peek/runtime.env').read_text().splitlines() if '=' in line)
print(shlex.split(values.get('PEEK_DATABASE_PATH', '"/var/lib/peek/peek.sqlite"'))[0])
PYDB
)
systemctl stop peek-server
if [[ -f $database_path ]]; then
    python3 - "$database_path" "$backup/peek.sqlite" <<'PYDB'
import sqlite3, sys
with sqlite3.connect(sys.argv[1]) as source, sqlite3.connect(sys.argv[2]) as target:
    source.backup(target)
PYDB
    chown --reference="$database_path" "$backup/peek.sqlite"
    chmod --reference="$database_path" "$backup/peek.sqlite"
fi

ln -sfn "$release" "$current_link.next"
mv -Tf "$current_link.next" "$current_link"
systemctl daemon-reload
systemctl enable peek-server caddy peek-backup.timer
systemctl restart peek-server

healthy=0
for _ in {1..30}; do
    if body=$(curl -fsS --max-time 2 "$local_healthz" 2>/dev/null) &&
        python3 -c 'import json,sys; b=json.loads(sys.argv[1]); sys.exit(0 if b.get("status")=="ok" and b.get("version")==sys.argv[2] else 1)' \
            "$body" "$expected_version"; then
        healthy=1
        break
    fi
    sleep 2
done
((healthy)) || die "peek-server did not report {\"status\":\"ok\",\"version\":\"$expected_version\"} on 127.0.0.1:8080/healthz within 60 s; see journalctl -u peek-server"

systemctl reload-or-restart caddy
systemctl is-active --quiet caddy || die "caddy is not running; see journalctl -u caddy"
tls=0
for _ in {1..15}; do
    if curl -fsS --connect-timeout 2 --max-time 5 --resolve "$host:443:127.0.0.1" "https://$host/healthz" >/dev/null 2>&1; then
        tls=1
        break
    fi
    sleep 2
done
if ((!tls)); then
    if ((require_tls)); then
        die "https://$host/healthz does not answer through Caddy (--require-tls); see journalctl -u caddy"
    fi
    printf 'peek install: warning: https://%s is not serving yet (DNS or the first certificate may be pending, BLUEPRINT §5.4)\n' "$host" >&2
fi
systemctl start peek-backup.timer

# Keep the current release, the previous one and the three newest others.
current=$(readlink -e "$current_link")
mapfile -t releases < <(find /opt/peek/releases -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' | sort -rn | cut -d' ' -f2-)
kept=0
for directory in "${releases[@]}"; do
    if [[ $directory == "$current" || $directory == "$previous" ]]; then continue; fi
    if ((kept < 3)); then kept=$((kept + 1)); continue; fi
    rm -rf -- "$directory"
done

printf 'peek install: %s (peek-server %s) is live%s\n' "$release" "$expected_version" "$( ((tls)) && echo ' over HTTPS' || echo ' locally')"
