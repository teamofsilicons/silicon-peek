# peek-server

The backend of Peek (root package `silicon-peek`, binary `peek-server`;
BLUEPRINT §5.2). It holds peek's IAM app secret and nothing a Silicon owns:

- exchanges SLTs, rotates and revokes app sessions — the `oat_`/`ort_` pair goes
  straight back to the caller and is **never stored** (D5);
- introspects every bearer **live** on every request (§2.7);
- mints a fresh single-use Ting OBO proof for every enrollment, revocation and
  delivery attempt, bound to the exact body bytes (§3.4–§3.6);
- streams Gemini TTS and sends completed WAV recordings to OpenAI
  `gpt-transcribe` through authenticated relays; provider keys stay on the server;
- keeps each Silicon's drawing copy, files bug reports as GitHub issues,
  relays client telemetry to Space Station, receives IAM webhooks, and is a
  Honeycomb lifecycle participant for testing environments (§2.9, §2.10).

## Layout

| File | What |
|---|---|
| `main.rs` | env → `Config`, JSON logs, bind, graceful shutdown (SIGTERM/Ctrl-C), 5-minute housekeeping |
| `config.rs` | every `PEEK_*` variable, strictly validated; all problems reported at once |
| `app.rs` | router, request middleware (`X-Request-ID`, `Cache-Control: no-store`, envelope for every error, logs, `http.completed`) |
| `plane.rs` | production vs testing plane, resolved only from the IAM-validated test secret; generation fence |
| `auth.rs` | bearer verification (the §2.7 checks) and per-route scopes |
| `iam.rs` | the IAM seam: `IamPlane`/`IamConnector` traits over `silicon-iam-client` 4.0.0 |
| `ting.rs` | OBO proofs, enrollment, revocation, deliveries, the §3.6 error matrix |
| `gemini.rs`, `openai.rs`, `deepgram.rs`, `github.rs` | Gemini TTS streaming, OpenAI transcription, legacy Deepgram BYO key validation, and bug reports |
| `honeycomb.rs` | lifecycle participant (barrier → wipe → receipt) and activity reports |
| `webhook.rs` | IAM webhook verification, routing, dedupe and removal cleanup |
| `idempotency.rs` | the `idempotency` table semantics |
| `telemetry.rs` | `EventSink` (Space Station `SpaceClient`), the §6.4 envelope, the gateway relay |
| `db.rs`, `store/` | SQLite (WAL, embedded migrations from `../migrations/`) and SQL per table |
| `routes/` | one handler module per route family |

## Routes

Every response carries `X-Request-ID` and `Cache-Control: no-store` (auth
routes also `Pragma: no-cache`). Every error is
`{"error":{"code","message","hint","retryable","request_id","details"?}}` with
codes from `silicon_peek_client::ErrorCode`, plus `Retry-After` when retrying
helps. Bodies are capped at 1 MiB (Caddy caps at 1 MB too); requests time out
after 60 s.

| Method and path | Auth | Notes |
|---|---|---|
| `GET /healthz` | – | `{"status":"ok","service":"peek","version"}` |
| `GET /readyz` | – | `200`/`503` `{"status","checks":{"db","iam_config","ting_config","gemini","openai","deepgram"}}`; ready = both databases answer and the app secret is set |
| `GET /api/v1/iam` | – (test key optional) | discovery; with a test key also `testing_environment_id`, `testing_generation`, `testing_environment{id,name,generation}` |
| `POST /api/v1/auth/login` | – | `{"slt"}` (+ `X-Org-ID` hint) → session incl. `ting` enrollment; public IDs refused in production (`slt_is_public_id`) |
| `POST /api/v1/auth/refresh` | – | `{"refresh_token"}` → session without `ting`; terminal errors → `401 session_rejected` |
| `POST /api/v1/auth/logout` | optional Bearer | `{"token"}` → `204`; with a bearer the Ting grant is revoked first (best effort); unknown tokens succeed |
| `GET /api/v1/auth/me` | Bearer + `X-Org-ID` | live identity, `display_name` (via `application_reads().me()`), `org_role`, scopes, `reconsent_required`, `ting` |
| `POST /api/v1/ting/recipient` | Bearer | `{}` → `{"subscribed":true,"subscription_id"}` (explicit enrollment only; never automatic, D7) |
| `POST /api/v1/deliveries` | Bearer | peekd's exact outbox bytes → `{"event_id","ting_id","status":"accepted","silent","replayed"}`; `type` must be one of the nine `peek.*` types (`TingType::ALL`: the six 0.1.0 types plus `peek.send.expired`, `peek.schedule.due`, `peek.send.shown`), `data` must match that type's strict client-crate schema and `data.context` the plane, and `key` must be `<verified actor>/<subject id>/<event>` |
| `POST /api/v1/speech/token` | Bearer | TTS returns `{"provider":"gemini","mode":"proxy",…}`; STT returns `{"provider":"openai","mode":"proxy",…}`. Both use `base_url:"<PEEK_PUBLIC_ORIGIN>/api/v1/speech"` and return no provider credential. |
| `POST /api/v1/speech/speak` | Bearer | `{"text","model","voice_instructions"?,"language"?,"sample_rate"?:24000}` → Gemini 3.8 Flash TTS SSE forwarded unbuffered. `model` is the selected voice ID, e.g. `Kore`. |
| `POST /api/v1/speech/listen` | Bearer | completed WAV (≤ 4 MiB) + allow-listed query (`model`, `language`, `detect_language`*, `keyterm`*, `numerals`, `smart_format`) → `{"text","request_id"?,"detected_language"?}` from OpenAI `gpt-transcribe` |
| `PUT/GET/DELETE /api/v1/drawings/current` | Bearer | raw JS ≤ 256 KiB with `X-Peek-Drawing-Sha256`; `GET` sends `ETag: "<sha256>"` (and honours `If-None-Match`) |
| `GET/PUT/DELETE /api/v1/orgs/{org}/byo/deepgram` | Bearer; PUT/DELETE need owner/admin | Legacy administration only; `PUT {"api_key","base_url"?}` validates and seals a Deepgram key, which is never returned or used for speech |
| `POST /api/v1/reports` | optional Bearer | stored, then filed as a GitHub issue; 10/h per client IP |
| `POST /api/web/telemetry` | none (Origin allowlist) | relay for `peekclidaemon`, `peekfrontendanalytics`, `peekfrontendevents` |
| `POST /webhooks/iam` | HMAC | verified over the raw body, deduped on `event_id`, `204` |
| `PUT/GET /internal/honeycomb/organizations/{org}/testing-environments/{env}/operations/{op}` | Bearer service token | lifecycle participant receipts |

`Idempotency-Key` (16–255 visible ASCII) is required on every POST except the
telemetry gateway (browsers never send one; Space Station dedupes by
`record_id`) and the IAM webhook (deduped by `X-Silicon-IAM-Event-ID`).

- **login/refresh/logout** pass the key straight to IAM, which replays within
  its 10-minute window. Their responses are never stored here (they contain
  tokens).
- **ting/recipient, deliveries, reports** use the `idempotency` table: same key
  and body → the stored response (`Idempotent-Replayed: true`); same key,
  different body → `409 idempotency_conflict`; still running → `409
  idempotency_in_progress` (retryable). Only successes are stored, so a failed
  attempt never poisons its key. The work runs in its own task, so a client
  that disconnects still finds the result on retry.
- **speech/token, speech/speak, speech/listen** require the key but never
  replay (audio and text are never stored).

### Speech (§5.2, speech proxy mode)

TTS uses `PEEK_GEMINI_API_KEY`, or only `PEEK_GEMINI_TEST_API_KEY` on the testing plane.
The API model is pinned to `gemini-3.8-flash-tts`; voice IDs and delivery instructions
come from the daemon's request. Peek converts paired `<indian accent>Anuv Jain</indian accent>`
spans into scoped `speech_metadata.style` and preserves Google's singleton vocal tags.
Requests use `store:false`. The server relays Google's SSE unchanged (24 kHz mono PCM
encoded in audio deltas); clients decode and play it progressively. Peek adds no
TTS concurrency cap; Google's project quotas govern upstream capacity. EOF or
client disconnect releases the upstream connection. Provider error bodies are dropped.

STT uses `PEEK_OPENAI_API_KEY`, or only `PEEK_OPENAI_TEST_API_KEY` on the testing
plane. Missing keys fail with `503 speech_unavailable {reason:not_configured}`;
there is no fallback to production credentials or Deepgram. The daemon uploads
its completed WAV; the server sends one multipart `audio.wav` to
`/v1/audio/transcriptions`, pinned to `gpt-transcribe`, and returns a neutral JSON
transcript. Optional `request_id` comes from OpenAI's `x-request-id` header and
`detected_language` from the first reported language. Neither is required.

`model` must be omitted or `gpt-transcribe`. `language` and explicit
`detect_language` hints become deduplicated `languages[]`; `true`/`false` add no
hint. BCP-47 hints use their primary subtag, preserving supported `zh-CN`,
`zh-TW` and `zh-HK` variants. Repeated `keyterm` values become `keywords[]`
(up to 100 values, 1–100 characters each; no angle brackets or control characters).
`numerals=true` requests digits using a short prompt; `smart_format` is accepted
for compatibility. Responses are limited to 1 MiB while reading. Provider error
bodies are discarded; 408, 429, 5xx and transport failures are retryable, while
invalid inputs and rejected credentials are not. Audio and transcript text are
never logged.

Token and relay calls share one budget: 120 per minute per Silicon, 1200 per org.
`listen` accepts up to 4 MiB (the router default is 1 MiB); `deploy/Caddyfile`
allows 5 MiB on `/api/v1/speech/listen` and 1 MB elsewhere.

Existing org Deepgram keys remain sealed for compatibility with the legacy BYO
administration API. They have no effect on speech routing. Saving a legacy key
validates its permissions with Deepgram. Its `base_url` must use HTTPS and a
hostname allowed by `PEEK_BYO_DEEPGRAM_HOSTS` (default `*.deepgram.com`); IP
addresses and localhost are refused. Startup and maintenance make no Deepgram calls.

### Delivery error matrix (§3.6)

| Ting says | peek-server returns |
|---|---|
| `202` / `200` | `200` (`replayed` = Ting's `200`) |
| `401 invalid_proof\|proof_expired\|proof_consumed` | one retry with a new proof over the same bytes, then `503 ting_unavailable` |
| `403 recipient_not_registered` | `409 recipient_not_registered` (enrollment marked revoked; no re-registration) |
| `403` otherwise | `502 ting_rejected` + `details.ting_code` |
| `404` | `502 ting_type_missing` (`Retry-After: 900`) |
| `409` | `409 ting_key_conflict` |
| `429` / `5xx` / transport | `503 ting_unavailable` + `details.retry_after` |
| IAM `401` on introspect | `401 unauthenticated` (peekd refreshes once) |
| missing scope | `403 reconsent_required` + `details.missing_scopes` |

## Data planes and storage

Two SQLite files with the same schema (`migrations/0001_initial.sql`): the
production database and one testing database for every testing environment.
Every row carries `ctx` (`production` or the environment UUID), resolved only
from a validated credential.

A request with `X-Testing-Environment-Key` carries **peek's** test app secret.
The server validates it with IAM (`applications().testing_context()` with the
testing-application selector), requires a Honeycomb participant binding
(`409 environment_not_prepared` otherwise), and requires the binding's current
`X-Testing-Environment-Generation` on every mutation except login and refresh
(`409 testing_generation_changed`). Ting's test headers come only from each
proof's `testing_context`, validated against the same environment; inbound
headers are never forwarded. Test traffic records no telemetry.

Lifecycle operations commit a pending barrier (new test requests are refused),
wait for in-flight requests of that environment, wipe its rows on
`clean`/`purge`/`retire-applications`, then write the completed receipt. The
root key is kept sealed for activity reports and test-webhook routing, and
erased on purge.

## Environment

Every variable, with a local default, is in [`../.env.example`](../.env.example)
(a unit test keeps that file in sync with `config::VARIABLES`). Required:
`PEEK_IAM_WEBHOOK_SECRET` and `PEEK_ENCRYPTION_KEY`. Deliberately optional:
`PEEK_IAM_APP_SECRET` (first deploy: `/readyz` says `iam_config: missing`, IAM
routes answer `503 iam_misconfigured`), the Gemini and OpenAI keys (`503
speech_unavailable {"reason":"not_configured"}`), the GitHub token (reports stay
`stored`), the Honeycomb token (participant routes answer `503`), and the Space
Station keys (a bad key disables that table with a warning; never a startup
failure). Unknown `PEEK_*` variables are logged as warnings.

## Running locally with a fake IAM

The quickest way is the whole local stack — fake IAM and Ting, this server
(temp SQLite, optional real Gemini and OpenAI keys passed explicitly through
the environment) and an isolated peekd — printed as an environment to source:

```sh
scripts/e2e/run-local.sh            # prints the env for the CLI and Peek.app
. <dir>/env.sh && peek login any-slt && peek register side 3
scripts/e2e/stop.sh
python3 scripts/e2e/e2e.py          # the automated end-to-end run (real CLI, fake UI)
```

To run only the server, `scripts/e2e/fake_services.py` (Python 3.9+, stdlib
only) fakes the IAM and Ting routes peek-server calls:

```sh
python3 scripts/e2e/fake_services.py --iam-port 8081 --ting-port 8082 &
# Any 47-character secret starting with ask_ works against the fake. Replace the
# empty PEEK_IAM_APP_SECRET= line instead of appending one: .env never overrides a
# variable that is already set, so its first occurrence wins and an appended second
# line is ignored (IAM routes would answer 503 iam_misconfigured).
sed "s/^PEEK_IAM_APP_SECRET=.*/PEEK_IAM_APP_SECRET=ask_$(printf 'd%.0s' $(seq 43))/" .env.example > .env
CARGO_TARGET_DIR=target cargo run -p silicon-peek --bin peek-server

curl -s localhost:8080/readyz
curl -s -H 'Idempotency-Key: local-login-000000001' -H 'Content-Type: application/json' \
  -d '{"slt":"oac_anything"}' localhost:8080/api/v1/auth/login
curl -s -H "Authorization: Bearer <access_token>" -H 'X-Org-ID: tos' localhost:8080/api/v1/auth/me
```

The CLI can use it too: `PEEK_API_URL=http://127.0.0.1:8080 peek login oac_anything`.

## Tests

```sh
export CARGO_TARGET_DIR=$PWD/target
cargo fmt -p silicon-peek --check
cargo clippy -p silicon-peek --all-targets -- -D warnings
cargo test -p silicon-peek
```

Unit tests cover configuration, sealing, the §2.7 checks, the delivery error
matrix, idempotency, the lifecycle transition table and the telemetry rules.
The integration tests in `tests/` run the real router and the real
`silicon-iam-client` against local wiremock fakes of IAM, Ting, Gemini, OpenAI, legacy Deepgram,
GitHub, Space Station and Honeycomb, with temp-dir databases; nothing talks to
a real service, except opt-in ignored live speech tests.
