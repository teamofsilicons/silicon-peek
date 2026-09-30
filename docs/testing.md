# Testing environments

peek supports IAM testing environments: isolated worlds with their own identities, orgs, Ting types and data, driven by peek's **test app secret**. A testing environment uses exactly the same code path as production; only the credentials and the storage partition differ. Nothing in a test world can reach production data, and peek never falls back to production when a test credential fails.

There is also **Simulation** in Peek.app, which is different: local fake data, no IAM, no Ting, no network. Use Simulation to see what bubbles look like; use a testing environment to exercise the full login, send, answer and delivery loop.

## Use a testing environment

You need peek's test app secret for the environment (an `ask_…` value from your operator) and a test Silicon's SLT or, in a test plane only, its public id.

```sh
H=/abs/path/to/test-silicon-home
# 1. log in with the test secret; peek discovers and saves the environment
printf %s "$PEEK_TEST_SECRET" | SILICON_HOME=$H SILICON_ORG=tos peek --app-secret-file - login si:peek-tester
# stderr ends with: Testing environment: peek testing (0192…-uuid)

# 2. from then on, select it by id
SILICON_HOME=$H peek --test 0192…-uuid register side 3
SILICON_HOME=$H peek --test 0192…-uuid register drawing ./logo.js
SILICON_HOME=$H ISI=deliberate peek --test 0192…-uuid send --json \
  --ask '{"question":"Ship it?","type":"single_choice","options":["Ship","Hold"]}'
```

The bubble shows a `TEST · peek testing` pill and a dashed ring. Answer it in Peek.app, and the test Silicon's webhook receives `peek.ask.answered` from the test Ting.

| Flag / env | Meaning |
|---|---|
| `--test <ENV_UUID>` / `SILICON_PEEK_TEST` | Select a **saved** environment. A raw secret here is refused with the hint "use --app-secret-file -". |
| `--app-secret-file <PATH\|->` | peek's test app secret; `-` reads one line from stdin. |
| `--app-secret <ask_…>` / `PEEK_TEST_APP_SECRET` | The same, inline (hidden in help). Conflicts with `--app-secret-file`. |

- When you pass a secret, peek calls `GET /api/v1/iam` with it, learns `testing_environment_id` and the current generation, and saves them to `$SILICON_HOME/.peek/testing.json` (0600).
- The session for a test world is stored under its own key, `"<api_url>#<environment uuid>"`, next to (never instead of) your production session.
- When testing is selected, the last line on stderr is always `Testing environment: <name> (<uuid>)`, on success and on failure. stdout stays pure JSON.
- peek **never** reads `IAM_TEST_APP_SECRET` or `IAM_TEST_KEY`. In a Silicon's environment those are Ting's test credentials, and sending them to peek's backend would fail.
- `peek iam --json --test <uuid>` adds `"testing":{"environment_id","name","generation"}`.
- `peek report` is refused under `--test`.

### When the world changes

A testing environment can be cleaned, rotated or rebuilt. peek then answers with a precise error and never retries against production:

| Code | Exit | Meaning | Fix |
|---|---|---|---|
| `testing_secret_invalid` | 3 | the secret is not a valid peek test secret, or was rotated | get the current secret; `peek --app-secret-file - login …` |
| `testing_generation_changed` | 3 | the environment was cleaned or rebuilt since you logged in | `peek --test <uuid> login status`, then log in again |
| `slt_rejected` | 3 | the test identity no longer exists | recreate it (operator), then log in |

After a clean, queued events and local history for the old generation are dropped.

## How isolation works

| Layer | Partition |
|---|---|
| CLI store | one session slot per `"<api_url>#<context>"`; `context` is `production` or the environment UUID |
| peekd (the Mac helper) | every row in its database is keyed by `context`; `si:dj` in production and `si:dj` in a test world are different identities, with separate positions, drawings, asks and deliveries |
| Peek.app | one physical set of 8 positions. **Production has priority**: a test registration of a position production holds succeeds in the test partition, but its bubble waits while a production bubble is there. Test bubbles carry a `TEST · <name>` pill, a dashed ring and a tooltip with the UUID and generation; drawings get `input.context === 'testing'`. Settings → Testing has "Show test peeks" (on by default) and lists the environments in use. |
| peek backend | every table has a `ctx` column, resolved from the validated secret and never from client input; test rows live in a separate database file |
| Ting | types, grants and hooks exist per context; a clean wipes them |
| Telemetry | test events are tagged `environment:"testing"` and never written to the production tables |
| Speech | TTS uses `PEEK_DEEPGRAM_TEST_API_KEY`; transcription uses `PEEK_OPENAI_TEST_API_KEY`. Test contexts never fall back to production keys |

### Headers on each hop

The same header name carries different secrets on different hops, so peek never forwards headers blindly:

| Hop | Headers |
|---|---|
| CLI / peekd → peek backend | `X-Testing-Environment-Key: <peek test secret>` on every route; `X-Testing-Environment-Generation: <n>` on every mutation except login and refresh (missing or stale → `409 testing_generation_changed`) |
| peek backend → IAM | the IAM SDK's testing application mode: `X-Testing-Application: Basic …` plus Basic auth with peek's test secret |
| peek backend → Ting | `IAM_TEST_APP_SECRET` and `X-Testing-Environment-Key` taken from **that proof's** `testing_context`, never from the inbound request |

## Provision an environment (operators)

peek must be registered as a Honeycomb **lifecycle participant** before it is imported into any environment. An app that is not a participant leaves the environment permanently pending. The Honeycomb operator adds `{"app_id":"peek","base_url":"https://backend.peek.teamofsilicons.com","token_env":"PEEK_HONEYCOMB_SERVICE_TOKEN"}` to `HONEYCOMB_LIFECYCLE_PARTICIPANTS`.

Then, in order (every step changes remote state):

1. Create the environment and fetch its root key:
   ```sh
   honeycomb --json environments create tos 'peek testing' --description '…'    # → ENV
   honeycomb --json environments key "$ENV"                                     # → ROOT
   ```
2. **Before the import**, create a test Carbon, the test org `tos` owned by it, and a test Silicon (the test plane's OTP is `000000`):
   ```sh
   iam --test $ENV signup --carbon-id peek-admin …
   iam --test $ENV login …
   iam --test $ENV org create tos
   iam --test $ENV -o json silicon create peek-tester …
   ```
   Otherwise IAM creates test `tos` owned by a non-login fixture Carbon, and nobody can rotate secrets or register Ting types.
3. Import peek. The import follows peek's external scopes recursively (peek → ting → honeycomb → briefcase); require every `services[].state == "ready"`.
   ```sh
   honeycomb --json environments import "$ENV" 'peek' --revision R
   ```
4. Get the test secrets. A Carbon-driven import prints none, so rotate them:
   ```sh
   SLT=$(iam --test $ENV -o json login --app-id honeycomb --grant-org tos --approve-scopes | jq -r .slt)
   honeycomb --test $ENV login "$SLT"
   honeycomb --test $ENV --json apps rotate-secret 'peek' --revision RP    # .app_secret = PEEK_TS
   honeycomb --test $ENV --json apps rotate-secret 'ting' --revision RT    # .app_secret = TING_TS
   ```
5. Register the nine Ting types in the test context, with descriptions byte-identical to production ([Ting events](ting.md)), using a test Ting session (`IAM_TEST_APP_SECRET=$TING_TS IAM_TEST_KEY=$ROOT ting login --token-stdin`).
6. Log the test Silicon in to peek: `printf %s "$PEEK_TS" | SILICON_HOME=$H SILICON_ORG=tos peek --app-secret-file - login si:peek-tester`.
7. Give the test Silicon a Ting webhook (`ting login`, `ting org use tos`, `ting webhook http://127.0.0.1:18777/events`; the same steps as [Receive answers without Stemcell](ting.md#receive-answers-without-stemcell)) and run the loop above.

After `honeycomb environments action $ENV clean`, repeat steps 2 to 7. The repository ships `scripts/testing/bootstrap.sh`, which does this idempotently.

## Local tests without any environment

The Rust crates are tested against local fakes only: HTTP fakes stand in for the peek backend, IAM, Ting and OpenAI, and a local WebSocket fake supplies the Deepgram voice stream. Every test uses a temporary `SILICON_HOME` and a private helper socket (`PEEK_DAEMON_SOCKET`). No test needs a key or touches your real `~/.peek` or `~/Library/Application Support/Peek`. To run the real binaries by hand against a local peek-server, use the [isolated run mode](development.md#isolated-run-mode): `PEEK_SUPPORT_DIR`, `PEEK_CACHES_DIR`, `PEEK_DAEMON_SOCKET`, `PEEK_NO_SERVICES=1` and `PEEK_API_URL` keep peekd and Peek.app away from your real data, login items and launch agents.
