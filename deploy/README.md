# Deploying and publishing peek

This runbook takes peek from a local build to a public Honeycomb app. The phases follow
BLUEPRINT §5.6. Run them in order, because each phase depends on the ones before it.

**Markers**

- **[MUTATING]** changes remote or shared state (AWS, DNS, GitHub, IAM, Honeycomb, Ting, Space
  Station, Vercel). It needs explicit approval before it runs. No command in this file has been run.
- **[USER]** only the account holder can do it: a certificate, an OTP, a payment, a DNS UI, or a
  third-party approval.
- **[RO]** read-only. Safe to repeat at any time.

**Invariants that hold in every phase**

- AWS runs in account `234951665042` with profile `default`. **Always pass `--region us-east-1`**,
  because the profile defaults to us-west-1.
- **Never `export SILICON_HOME`.** Use the wrappers in `scripts/operator/toolbox.sh` instead.
- Prefix every Honeycomb call with `HONEYCOMB_AUTO_UPDATE=0`. The toolbox's `hc` does this for you.
- Operator material lives in `~/.peek-operator` (mode 0700). Secrets go into files there or into
  Secrets Manager. They never go into argv, shell history, git or chat. Delete a transient secret
  file with `rm -P` once it has been stored.
- `application.json` and `runtime.json` are private and are listed in `.gitignore`. The committed
  files are `application.json.example` (repository root) and `deploy/runtime.json.example`.

## What is here

| File | Role |
|---|---|
| `stack.yaml` | CloudFormation stack `silicon-peek-production`. It creates an S3 bucket (`releases/`, plus `backups/` with a 7-day expiry, SSE, TLS-only), the Secrets Manager secret `silicon-peek/production/runtime`, an instance role (SSM core, `s3:GetObject releases/*`, `s3:PutObject backups/*`, `secretsmanager:GetSecretValue`), a security group that opens only 80 and 443, and a t4g.small AL2023 arm64 instance with 20 GB of encrypted gp3 (kept on termination), IMDSv2 and hop limit 1. An Elastic IP is created only when `AllocateEip=true`. |
| `deploy.py` | Builds a deterministic release archive, copies it to S3, runs `install.sh` on the host through `ssm send-command`, then checks `/healthz`. Use `--dry-run` to call no AWS API. |
| `install.sh` | The host-side installer, run as root through SSM. It takes a lock, backs up the managed files, renders `/etc/peek/runtime.env` (mode 0600), installs Caddy and the units, validates the Caddyfile, flips `/opt/peek/current` atomically, restarts, and health-checks. On any failure it rolls back: every step of the rollback runs even if one fails, and it ends with `restored <release>` only when the previous release answers `/healthz` again, `ROLLBACK INCOMPLETE` otherwise (which `deploy.py` reports). |
| `render-env.py` | Validates the runtime JSON key by key and writes the EnvironmentFile. `--check` validates locally and prints key names only. |
| `peek-server.service` | systemd unit for the `peek` user. Its sandbox allows writes only to `/var/lib/peek`. |
| `caddy.service`, `Caddyfile` | TLS for `backend.peek.teamofsilicons.com` via ACME on port 80, security headers, body limits (5 MiB on `POST /api/v1/speech/listen` for the speech proxy's recorded audio, 1 MB elsewhere, answered with peek's JSON error envelope), and a reverse proxy to `127.0.0.1:8080`. |
| `backup.py`, `peek-backup.service`, `peek-backup.timer` | Hourly online SQLite snapshots of `peek.sqlite` and `testing.sqlite`. Each snapshot is integrity-checked, then uploaded to `backups/` with SSE. |
| `runtime.json.example` | Every runtime key. Non-secret values are filled in and secret values are empty. |

## P0: local build and tests (no approval needed)

```sh
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
python3 scripts/sync-cli-docs.py --check && scripts/check-installer.sh
python3 -m unittest discover -s scripts/tests && python3 -m unittest discover -s deploy/tests
(cd web && npm ci && npm run build)
scripts/build-release.sh --skip-loopback        # after the spike (BLUEPRINT §8.12); a dev build without Developer ID
```

`scripts/build-release.sh` signs with the Developer ID Application identity when the keychain has
one. Otherwise it signs with Apple Development, which gives the bundle id `ai.tos.peek.dev`, an empty
`team_id` in `Peek.app.info`, and dev-channel releases only (BLUEPRINT §5.5). It never signs ad hoc.
Both kinds sign with the hardened runtime and Apple's secure timestamp. `PEEK_SIGN_TIMESTAMP=none`
skips the timestamp, and is allowed for an offline development build only.

After an Xcode update, every compiler shim fails with "You have not agreed to the Xcode license
agreements" until the [USER] runs `sudo xcodebuild -license`. The scripts stop with that fix. Until
it is done, a development candidate can still be built:

```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools \
PEEK_PREBUILT_APP=apps/mac/<derived-data>/Build/Products/Release/Peek.app \
  scripts/build-release.sh --replace
```

`DEVELOPER_DIR` makes cargo and lipo use the Command Line Tools. `PEEK_PREBUILT_APP` points at an
unsigned universal Release Peek.app that an earlier xcodebuild made from this tree. The build refuses
the bundle if any app source is newer than it. It then embeds a fresh peekd, applies the bundle id
through the same two build phases Xcode runs, and signs, zips and verifies the app exactly as usual.
`Peek.app.build.json` records the bundle's source as `app_source`.

## P1: operator toolbox [MUTATING, local files only] (depends on approval U1)

```sh
. scripts/operator/toolbox.sh
peek_ops_help
peek_ops_setup --yes            # ~/.peek-operator/{backups,ting-home,iam,cargo}, mode 0700
peek_ops_backup_iam --yes       # cp -Rp ~/.silicon-iam → ~/.peek-operator/backups/silicon-iam-<UTC>
peek_ops_install --yes          # iam 4.0.0 and spacestation 0.2.0 into ~/.peek-operator/cargo
peek_ops_verify                 # [RO] Ting 0.1.9 sha256 72223c09…, versions, Honeycomb session
```

For a stricter setup that never touches `~/.silicon-iam`, run `PEEK_OPS_IAM_ISOLATED=1`, then
`iam4 login --carbon-id c:shubham`. This costs one OTP.

## P2: GitHub repository [MUTATING] (depends on approval U2)

```sh
gh repo create teamofsilicons/silicon-peek --public \
  --description 'Glanceable voice-first comms between Carbons and Silicons on macOS'
git remote add origin git@github.com:teamofsilicons/silicon-peek.git && git push -u origin main
```

## P3: secrets, stack and first backend deploy [MUTATING, AWS spend] (depends on approval U3)

1. Generate the three generated secrets into files. Nothing is printed.

   ```sh
   umask 077; OPS=$HOME/.peek-operator
   openssl rand -hex 32 > "$OPS/webhook-secret"          # PEEK_IAM_WEBHOOK_SECRET
   openssl rand -hex 32 > "$OPS/encryption-key"          # PEEK_ENCRYPTION_KEY (AES-256-GCM for BYO keys and root keys)
   openssl rand -hex 32 > "$OPS/honeycomb-service-token" # PEEK_HONEYCOMB_SERVICE_TOKEN (shared with the Honeycomb operator, U12)
   jq --rawfile w "$OPS/webhook-secret" --rawfile e "$OPS/encryption-key" --rawfile t "$OPS/honeycomb-service-token" \
     '.PEEK_IAM_WEBHOOK_SECRET=($w|rtrimstr("\n")) | .PEEK_ENCRYPTION_KEY=($e|rtrimstr("\n")) | .PEEK_HONEYCOMB_SERVICE_TOKEN=($t|rtrimstr("\n"))' \
     deploy/runtime.json.example > "$OPS/runtime.json"
   python3 deploy/render-env.py --check < "$OPS/runtime.json"    # [RO] must print "31 keys valid"
   ```

2. Create the stack. The account is over its EIP quota (14 in use, 10 applied). If
   `AllocateEip=true` fails, redeploy with `AllocateEip=false`. Then never stop the instance,
   because a stop/start changes its public IP (BLUEPRINT §5.5).

   ```sh
   aws --region us-east-1 cloudformation deploy --stack-name silicon-peek-production --template-file deploy/stack.yaml \
     --capabilities CAPABILITY_NAMED_IAM \
     --parameter-overrides Vpc=vpc-04b23a487cfe0bd8e Subnet=subnet-07945746462c26b2d AllocateEip=true \
     --tags Service=silicon-peek Environment=production
   aws --region us-east-1 cloudformation describe-stacks --stack-name silicon-peek-production \
     --query 'Stacks[0].Outputs'                                   # [RO] InstanceId, PublicIp, ImageId, ArtifactBucket
   ```

   **On every later stack update, pass `PinnedImageId=<ImageId output>`.** CloudFormation
   re-resolves the latest-AMI SSM parameter on each update. A newer AMI would replace the instance,
   which means a new IP and an empty `/var/lib/peek`.

3. Store the runtime secret. The value is read from a file, never passed in argv.

   ```sh
   aws --region us-east-1 secretsmanager put-secret-value --secret-id silicon-peek/production/runtime \
     --secret-string file://$HOME/.peek-operator/runtime.json
   ```

4. Build and deploy. The first deploy must ship a Linux ARM64 Caddy binary. Take it from the
   official release (`caddy_<v>_linux_arm64.tar.gz` plus its `.sha256` checksum file) and verify it
   before use.

   ```sh
   cargo zigbuild --release --locked --target aarch64-unknown-linux-musl -p silicon-peek
   python3 deploy/deploy.py --dry-run                                  # [RO] release id, files, remote script
   python3 deploy/deploy.py --region us-east-1 --profile default --caddy ./caddy-linux-arm64
   ```

   The host checks `http://127.0.0.1:8080/healthz` for this exact version before it keeps the
   release. Before DNS exists, the HTTPS check only warns. After P4, deploy with `--require-tls`.
   The first deploy has no app secret, so `/readyz` reports `iam_config: missing` and login answers
   `503 iam_misconfigured`. Both are expected until P5.

Admin access uses SSM only: `aws --region us-east-1 ssm start-session --target <InstanceId>`.

## P4: DNS for the backend [USER] (depends on P3)

In Namecheap, change **only** this record, with TTL 300:
`A backend.peek → <PublicIp>`. **Never replace the whole zone**, because `setHosts` rewrites every
record. The API also needs the current IP whitelisted (U4). Then run these checks:

```sh
curl --fail --show-error https://backend.peek.teamofsilicons.com/healthz     # [RO] {"status":"ok","service":"peek","version":"0.1.0"}
curl --fail https://backend.peek.teamofsilicons.com/readyz                   # [RO]
python3 deploy/deploy.py --require-tls                                       # [MUTATING] redeploy; proves TLS through Caddy
```

Do not create the IAM app against an unresolvable webhook host. The create is effectively
irreversible, and it is UNCERTAIN whether IAM probes the host (§10.3 Q1).

## P5: Honeycomb app, app secret, webhook, Ting scopes [MUTATING, USER OTP] (depends on P4)

```sh
. scripts/operator/toolbox.sh; umask 077; OPS=$HOME/.peek-operator
jq --rawfile s "$OPS/webhook-secret" '.webhook_secret=($s|rtrimstr("\n"))' application.json.example > "$OPS/application.json"
hc --json --idempotency-key peek-logo-000000001 apps upload-logo tos ./brand/logo.png   # optional; then add "logo_url"
hc --json --idempotency-key peek-create-0000001 apps create "$OPS/application.json" > "$OPS/create-result.json"
```

Do the next step immediately. Put `.app_secret` (`ask_…`, shown once) into the runtime secret as
`PEEK_IAM_APP_SECRET`. Then redeploy, and delete the result file.

```sh
jq --arg k PEEK_IAM_APP_SECRET --rawfile r "$OPS/create-result.json" '.[$k]=($r|fromjson|.app_secret)' "$OPS/runtime.json" > "$OPS/runtime.json.new" \
  && mv "$OPS/runtime.json.new" "$OPS/runtime.json" && python3 deploy/render-env.py --check < "$OPS/runtime.json"
aws --region us-east-1 secretsmanager put-secret-value --secret-id silicon-peek/production/runtime --secret-string file://$OPS/runtime.json
python3 deploy/deploy.py --require-tls && rm -P "$OPS/create-result.json"
```

If the response is lost, rerun the **same** command with the **same** key and the same file
within 10 minutes, or run `hc operations recover-secret <op>`. After that window, use
`hc apps rotate-secret peek --revision N --step-up-file F`.

Webhook approval. The step-up needs an email OTP [USER], and the token lives 5 minutes.

```sh
hc --json apps get peek                          # [RO] revision N, iam_revision, state
hc --json apps webhook status peek               # [RO] pending endpoint UUID, application UUID
iam4 -o json step-up application.webhook.approve <APP_UUID> | jq -r .step_up_token > "$OPS/stepup"
hc --json --idempotency-key peek-webhook-approve-01 apps webhook approve peek --endpoint <PENDING_ENDPOINT_UUID> --revision <N> --step-up-file "$OPS/stepup"
rm -P "$OPS/stepup"
iam4 -o json --org tos app scopes requests --status pending      # [RO] if Ting's critical scopes are pending:
iam4 -o json app scopes approve <REQUEST_UUID>                   # c:shubham is a tos admin
```

Change the configuration later only by resubmitting the whole document:
`hc --idempotency-key <new> apps update peek "$OPS/application.json" --revision <current>`.
An empty `webhook_secret` keeps the stored secret. **Freeze the scopes before the first Silicon
logs in** (D26).

## P6: Ting types and Space Station tables [MUTATING] (depends on P5)

The six type names and descriptions are final. Types cannot be deleted (BLUEPRINT §3.1).

```sh
iam4 -o json login --app-id ting --grant-org tos | jq -r .slt | tingop login --token-stdin --json
tingop org use tos --json
tingop --org tos types register --type 'peek.ask.answered'     --description 'A Carbon answered a peek --ask (voice, keyboard or click).' --json
tingop --org tos types register --type 'peek.ask.dismissed'    --description 'A Carbon dismissed a peek --ask without answering.' --json
tingop --org tos types register --type 'peek.ask.expired'      --description 'A peek --ask reached its --expires-in deadline unanswered.' --json
tingop --org tos types register --type 'peek.message.received' --description 'A Carbon spoke or typed to the Silicon from its peek with no pending ask.' --json
tingop --org tos types register --type 'peek.speech.finished'  --description 'A peek --speak finished playing or was stopped by the Carbon.' --json
tingop --org tos types register --type 'peek.show.dismissed'   --description 'A Carbon closed a peek --show before it retracted.' --json
tingop --org tos types list --app peek --json                    # [RO] all six

iam4 -o json login --app-id spacestation --grant-org tos | jq -r .slt | ss2 login - --org tos
for t in peekbackend peekclidaemon peekfrontendanalytics peekfrontendevents; do
  ss2 --org tos tables create "$t" --access @c:shubham > "$OPS/$t.table-key"; done
ss2 --org tos tables ls --json                                   # [RO]
```

Move the four keys into `runtime.json` as `PEEK_BACKEND_TABLE_KEY`, `PEEK_CLIDAEMON_TABLE_KEY`,
`PEEK_FRONTEND_ANALYTICS_TABLE_KEY` and `PEEK_FRONTEND_EVENTS_TABLE_KEY`. Then run
`render-env.py --check`, `put-secret-value` and `deploy.py --require-tls`, and `rm -P` the four
key files.

## P7: Deepgram [USER key, then MUTATING] (depends on U6)

The account holder creates a Member-role key and a separate test-project key, and decides on MIP
opt-out. Put them into `PEEK_DEEPGRAM_API_KEY` and `PEEK_DEEPGRAM_TEST_API_KEY`, redeploy, and
check `/readyz`, which must show `"deepgram":"configured"`. Until this is done, `--speak` degrades
to a text pill and voice answers are disabled (BLUEPRINT §5.5).

## P8: website [MUTATING, USER DNS] (depends on U9)

```sh
cd web && npm ci && npm run build
V="npx --yes vercel@60.0.1"
$V link --project silicon-peek --scope shubham-guptas-projects-7ecb7811 --yes
$V deploy --prod --yes --build-env VITE_PEEK_ANALYTICS_TABLE=peekfrontendanalytics --build-env VITE_PEEK_EVENTS_TABLE=peekfrontendevents
$V domains add peek.teamofsilicons.com       # then [USER]: add the record Vercel prints (A peek → 76.76.21.21) at Namecheap
curl -fsSL https://peek.teamofsilicons.com/install.sh | cmp - scripts/install.sh   # [RO] the served installer is the canonical one
```

## P9: Developer ID and notarization [USER]

1. In Xcode, open Settings → Accounts → Manage Certificates → + → **Developer ID Application** for
   team LTBSK59BJ2.
2. Run `xcrun notarytool store-credentials peek-notary --apple-id <id> --team-id LTBSK59BJ2`. It
   asks for an app-specific password.
3. The first unattended codesign may show a keychain "Always Allow" dialog (U8).

## P10: release, dev channel, then prod [MUTATING] (depends on P5; prod also needs P9)

```sh
NOTARY_PROFILE=peek-notary scripts/build-release.sh --version 0.1.0     # validated, packed, loopback-tested; uploads nothing
hc login status --json                                                  # [RO] c:shubham, tos: org_admin
hc --json apps get peek                                                 # [RO] "revision": N
hc --json --idempotency-key peek-rel-0.1.0-dev-01  releases upload peek dist/peek-0.1.0.tar.gz --channel dev  --revision N
honeycomb install 'peek>test@0.1.0' --json && peek iam --json           # tos-member smoke test of the dev channel
hc --json --idempotency-key peek-rel-0.1.0-prod-01 releases upload peek dist/peek-0.1.0.tar.gz --channel prod --revision N
hc releases list peek --include-private --json                          # [RO]
```

Without Developer ID, the build is `ai.tos.peek.dev`. Upload it to `--channel dev` only.
`build-release.sh` prints the correct channel. A lost upload response is retried with the same key,
revision, channel and archive. Each version is immutable per channel.

The CI workflow `.github/workflows/release.yml` builds the same candidate from a `v*` tag. It never
uploads to Honeycomb.

## P11: publication gates [USER, third party] (depends on P10)

```sh
hc publication get peek --json          # [RO] expect request_pending; gates: ting, honeycomb
hc publication inbox
hc publication decide <REQUEST_ID> ting approve --revision N --reason 'peek needs subscriptions.register/tings.send/subscriptions.revoke to deliver answers to the asking Silicon.'
hc publication reply peek --message 'Linux/Windows builds implement the full IAM CLI contract; Mac-bound commands return platform_unsupported by design (Peek.app is macOS 26+).'
# [USER] a Honeycomb validator (validator=true; c:shubham is not one) approves the "honeycomb" gate (U11)
hc publication activate <REQUEST_ID> --revision N       # only if activation is not automatic
```

Until publication, peek is effectively private to `tos` members. `silicon connect` with
`apps: [peek]` likely fails (UNCERTAIN, BLUEPRINT §4.7). Use a Carbon-installed `peek` with
`SILICON_HOME=<home>` for test Silicons.

## P12: testing environments [USER, then MUTATING] (depends on U12)

The Honeycomb operator adds
`{"app_id":"peek","base_url":"https://backend.peek.teamofsilicons.com","token_env":"PEEK_HONEYCOMB_SERVICE_TOKEN"}`
to `HONEYCOMB_LIFECYCLE_PARTICIPANTS`, shares the token, and restarts Honeycomb. **Never import peek
into an environment before this is done**, because the environment then stays pending forever.

```sh
scripts/testing/bootstrap.sh                                   # dry run: every step, nothing mutating
scripts/testing/bootstrap.sh --yes --participant-registered    # create, identities, import, secrets, types, logins
scripts/testing/bootstrap.sh --yes --participant-registered --clean   # wipe (generation + 1) and rebuild
```

## Operating the host

| Task | Command |
|---|---|
| Logs | `sudo journalctl -u peek-server -u caddy --since -1h` (through `ssm start-session`) |
| Health | `curl -fsS http://127.0.0.1:8080/healthz` on the host; `/readyz` lists `db`, `iam_config`, `ting_config` and `deepgram` |
| Roll back | Redeploy the previous build: `python3 deploy/deploy.py --binary <old peek-server>`. Every release directory stays under `/opt/peek/releases` (the current one, the previous one and the three newest others). |
| Backups | `systemctl list-timers peek-backup.timer`; `sudo -u peek python3 /opt/peek/current/backup.py`; objects are under `s3://<ArtifactBucket>/backups/YYYY/MM/DD/HHMMSSZ/` |
| Restore | Stop `peek-server`. Copy the snapshot over `/var/lib/peek/<name>` (owner `peek`, mode 0600). Delete any stale `-wal`/`-shm` files. Start `peek-server`. |
| Rotate the webhook secret | `hc apps webhook rotate-secret peek --revision N --secret-file F --step-up-file F`, then update `PEEK_IAM_WEBHOOK_SECRET` and redeploy. Keep both key versions for 10 minutes. |
