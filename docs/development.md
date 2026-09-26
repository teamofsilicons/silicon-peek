# Development

peek is open source: https://github.com/teamofsilicons/silicon-peek. This page explains how the pieces fit, how to build and test each one, the local protocol between them, and how to contribute a fix. If you only use peek, you do not need it.

## Architecture

| Component | Language | Runs | Owns |
|---|---|---|---|
| **Peek.app** (`ai.tos.peek`) | Swift 6, SwiftUI + AppKit | one per macOS user, `~/Applications/Peek.app`, menu-bar only | the 8 position panels, the glass and drawing compositor, one QuickJS VM per drawing, audio playback and microphone capture, hotkeys, backdrop sampling, Settings, Simulation. **No tokens and no network**: everything goes through peekd. |
| **peekd** (`Contents/Helpers/peekd`) | Rust | one per macOS user, a launchd agent registered by the app | positions, the send queue, asks and history, the delivery outbox, Deepgram calls, per-home session refresh, the telemetry relay, Peek.app self-update, the stale-CLI watchdog |
| **peek CLI** (`peek`) | Rust | one copy per Silicon home (installed by Honeycomb), plus an optional Carbon copy | the IAM app contract (`iam`, `login`, `status`, `logout`, `config`), the per-home store `$SILICON_HOME/.peek/`, input validation, reading image and drawing bytes |
| **silicon-peek-client** | Rust library | linked into the CLI, peekd and the server | a stateless HTTP client for the backend, all wire types (IPC, show and ask schemas and validators, delivery bodies, Ting data), error types. Feature `runtime` adds the store, session refresh, locks and the IPC client. |
| **peek-server** | Rust (axum, SQLite) | `https://backend.peek.teamofsilicons.com` | the IAM app secret, SLT exchange, refresh and revoke, bearer introspection, Ting proofs and sends, Ting enrollment, the Deepgram key and org keys (short-lived tokens in direct mode, a streaming relay in proxy mode), drawing copies, bug reports, the IAM webhook, the Honeycomb lifecycle participant, the telemetry gateway |
| **Website and docs** | SolidJS + Vite | `https://peek.teamofsilicons.com` | the landing page, `/docs`, `/install.sh`, `/llms.txt` |

```text
 ISI (claude/codex) ──$ peek send …──┐              Carbon (hotkeys, mic, keyboard, clicks)
 stemcell ──$ peek login/status──┐   │                         │
                                 ▼   ▼                         ▼
                   peek CLI (per SILICON_HOME)         Peek.app (UI, QuickJS, audio)
                     │  $SILICON_HOME/.peek/                   │
                     │  session.json (oat_/ort_)               │ UDS role=ui
                     │  UDS role=cli + home_token              │
                     ▼                                         ▼
          /var/tmp/silicon-peek-<uid>/peekd.sock ◄──────► peekd (per macOS user)
                                                   ~/Library/Application Support/Peek/peekd.sqlite
                           ┌───────────────────────────────┼──────────────────────────┐
                           ▼ HTTPS, Bearer oat_ + X-Org-ID  ▼ direct mode only:        │
                  peek-server (app secret)                 HTTPS, Bearer <JWT>         │
          ┌───────┬────────┼──────────┬──────────┐         api.deepgram.com            │
          ▼       ▼        ▼          ▼          ▼         /v1/speak  /v1/listen       │
         IAM    Ting   Deepgram    Space      GitHub                                   │
                 │     /v1/auth/grant, or the relay in proxy mode:                     │
                 │     /api/v1/speech/speak → /v1/speak, /api/v1/speech/listen → /v1/listen
                 ▼ WebSocket → ting-daemon → POST http://<handle>.<org>.localhost/events → your flow
```

Design choices that shape everything:

- **The helper lives inside the app**, not in the Honeycomb package, so there is one stable path per macOS user. Honeycomb deletes package directories on every update, and every Silicon home has its own package copy.
- **Client custody of tokens.** Each Silicon's IAM session lives in its own home; the backend stores none. See [IAM and sessions](iam.md).
- **Speech is Deepgram, driven by peekd.** peekd asks the backend for a speech token first. In **direct** mode the backend mints a token that lives at most 60 seconds and peekd calls Deepgram itself; in **proxy** mode (a key that cannot mint tokens) peekd sends the request to the backend, which relays it to Deepgram and streams the answer back without storing or logging it. See [Speech tokens and the relay](#speech-tokens-and-the-relay).
- **No live transcript.** Speech-to-text runs once, after the Carbon stops recording.
- **Validation runs in the app**, with the same QuickJS and renderer as the screen, so a drawing that validates behaves identically live.

## Repository layout

```text
silicon-peek/
  Cargo.toml          # workspace: ".", crates/client, crates/cli, crates/daemon (resolver 3)
  src/                # package silicon-peek → bin peek-server
  migrations/         # backend SQLite migrations (applied at startup)
  crates/client/      # silicon-peek-client (feature "runtime")
  crates/cli/         # silicon-peek-cli, bin "peek"; docs/ is a synced copy of /docs
  crates/daemon/      # silicon-peek-daemon, bin "peekd" (macOS only)
  apps/mac/           # Peek.app: project.yml (xcodegen), Sources/, Resources/, Packages/CQuickJS/
  web/                # this website; web/scripts/build-docs.mjs renders /docs
  docs/               # canonical Markdown: these pages
  deploy/             # CloudFormation, systemd units, Caddyfile, deploy scripts
  scripts/            # release builds, Honeycomb packaging, docs sync, testing bootstrap
  honeycomb.yaml      # the package manifest
```

## Build and test

Rust 1.98 or newer (`rust-toolchain.toml`, with clippy and rustfmt; the workspace sets `rust-version = "1.98"`):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p silicon-peek-cli              # target/debug/peek
cargo build -p silicon-peek-daemon           # target/debug/peekd (macOS only)
cargo build -p silicon-peek                  # target/debug/peek-server
```

Tests never need keys or the network. The backend, IAM, Ting and Deepgram are replaced by `wiremock` servers, stores live in temporary directories, and the helper's socket is overridden with `PEEK_DAEMON_SOCKET`. Never point a test at your real `~/.peek`, `~/.silicon-iam` or `~/Library/Application Support/Peek`. To run the real binaries by hand, use the [isolated run mode](#isolated-run-mode).

Peek.app (Xcode 26.4, macOS SDK 26.4, xcodegen; QuickJS-ng v0.17.0 is vendored with a pinned hash):

```sh
cd apps/mac
xcodegen generate
xcodebuild -project Peek.xcodeproj -scheme Peek -configuration Debug -destination 'platform=macOS' -derivedDataPath build build
(cd Packages/PeekKit && swift test)          # the app's Swift packages (Swift Testing)
```

The Xcode build compiles peekd with cargo and embeds it at `Contents/Helpers/peekd`; set `PEEK_SKIP_EMBED_PEEKD=1` to skip that step, or `PEEKD_BINARY=<path>` to embed a prebuilt one. Add `CODE_SIGNING_ALLOWED=NO` for an unsigned build: macOS may not keep permissions such as the microphone for it, but it is enough for Simulation and the isolated run mode.

Debug builds use the bundle id `ai.tos.peek.dev`, so their permissions and launch services identity never collide with the released app. Sign development builds with an Apple Development certificate, never ad hoc: an ad-hoc signature changes on every build and resets the microphone permission.

Website (Node 24 or newer):

```sh
cd web
npm ci
npm run dev          # http://127.0.0.1:4317
npm run build        # type-check, bundle, prerender /docs, then check every page and link
npm test             # unit tests for the docs build and the landing page
```

The docs are the Markdown files in `/docs`. `web/scripts/build-docs.mjs` renders them to `/docs/<topic>`, and `scripts/sync-cli-docs.py` copies them into `crates/cli/docs/` so `peek docs` embeds the same text. Edit `/docs`, never the copies.

## Isolated run mode

To try the real binaries (peek-server, peekd, Peek.app and the CLI) on your own Mac without touching your real Peek, point every piece at scratch locations with these variables. Both peekd and Peek.app honour them.

| Variable | Read by | Replaces |
|---|---|---|
| `PEEK_SUPPORT_DIR` | peekd, Peek.app | `~/Library/Application Support/Peek` (the database, `settings.json`, drawings, recordings, offers, logs) |
| `PEEK_CACHES_DIR` | peekd, Peek.app | `~/Library/Caches/Peek` |
| `PEEK_DAEMON_SOCKET` | peekd, Peek.app, CLI | `/var/tmp/silicon-peek-<uid>/peekd.sock`. The lock file sits next to it. Its directory must be yours and mode 0700, and the path must stay under 104 bytes (the macOS socket limit). |
| `PEEK_NO_SERVICES=1` | Peek.app | Peek.app never touches SMAppService (no login item, no launchd agent, not even a status read), never spawns peekd itself, and `--uninstall` leaves services alone. It connects to whatever peekd listens on `PEEK_DAEMON_SOCKET`. |
| `PEEK_API_URL` | peekd, Peek.app, CLI | the peek-server origin (`http` is allowed on loopback). Peek.app makes no HTTP calls; it passes the value on and shows it in Settings → Diagnostics. |

Useful companions:

| Variable | Read by | Use |
|---|---|---|
| `SILICON_HOME` | CLI | a scratch Silicon home, so the session lands in `$SILICON_HOME/.peek/` |
| `PEEK_APPLICATIONS_DIR` | peekd | where the self-updater looks for and swaps Peek.app, instead of `~/Applications` |
| `PEEK_UI_EXECUTABLE` | peekd | the exact Peek executable allowed to connect as the UI (by default a sibling of peekd's own bundle, or any `…/Peek.app/Contents/MacOS/Peek` when peekd runs outside a bundle) |
| `PEEK_INSTALL_SUPPORT_DIR`, `PEEK_INSTALL_APPLICATIONS_DIR` | CLI | where `peek app install` (and the post-login install) writes |
| `PEEK_INSTALL_NO_LAUNCH=1` | CLI | the CLI never calls `launchctl` or `open`, so it never starts an app or an agent |
| `PEEKD_LOG` | peekd | log filter (for example `debug`) |

Nothing in an isolated run may register login items or launch agents for your real user, or write to your real `~/Library/Application Support/Peek`. A recipe, from the repository root (every path is scratch):

```sh
R=$(mktemp -d /tmp/peek.XXXXXX)            # short: the socket path must stay under 104 bytes
mkdir -m 700 "$R/run" && mkdir -p "$R/home" "$R/Applications"
iso() { env PEEK_SUPPORT_DIR="$R/support" PEEK_CACHES_DIR="$R/caches" PEEK_DAEMON_SOCKET="$R/run/peekd.sock" \
  PEEK_NO_SERVICES=1 PEEK_API_URL=http://127.0.0.1:8080 PEEK_APPLICATIONS_DIR="$R/Applications" \
  PEEK_INSTALL_SUPPORT_DIR="$R/support" PEEK_INSTALL_APPLICATIONS_DIR="$R/Applications" PEEK_INSTALL_NO_LAUNCH=1 \
  SILICON_HOME="$R/home" "$@"; }

# 1. fake IAM and Ting, then peek-server (see src/README.md; any 47-character ask_… secret works with the fake).
#    Replace the empty PEEK_IAM_APP_SECRET= line instead of appending one: .env never overrides a
#    variable that is already set, so its first occurrence wins and a second line is ignored
#    (IAM routes would then answer 503 iam_misconfigured). The process environment beats .env.
python3 scripts/e2e/fake_services.py --iam-port 8081 --ting-port 8082 &
sed "s/^PEEK_IAM_APP_SECRET=.*/PEEK_IAM_APP_SECRET=ask_$(printf 'd%.0s' $(seq 43))/" .env.example > .env
cargo run -p silicon-peek --bin peek-server &

# 2. peekd, headless: it never launches Peek.app
cargo build -p silicon-peek-daemon -p silicon-peek-cli
iso target/debug/peekd run --headless &

# 3. Peek.app from a Debug build, with services off
iso apps/mac/build/Build/Products/Debug/Peek.app/Contents/MacOS/Peek &

# 4. the CLI, as a scratch Silicon
iso target/debug/peek login oac_anything
iso target/debug/peek register side 3
```

Stop everything with `kill %1 %2 %3 %4` (or by pid) and delete `$R` when you are done.

**Real speech in an isolated run.** By default `.env.example` points Deepgram at a local fake. To hear real speech, give peek-server a real key at runtime only, and keep test phrases short (every request costs money):

```sh
PEEK_DEEPGRAM_BASE_URL=https://api.deepgram.com \
PEEK_DEEPGRAM_API_KEY="$(cat ~/.peek-operator/deepgram-api-key)" \
cargo run -p silicon-peek --bin peek-server
```

Never copy the key into `.env`, a fixture, a test or a log. A key that is not allowed to mint tokens (Deepgram answers 403 to `/v1/auth/grant`) makes peek-server answer in [proxy mode](#speech-tokens-and-the-relay), so the audio flows through your local peek-server.

## The local IPC protocol (version 1)

The CLI and Peek.app talk to peekd over one Unix socket.

**Socket.** `/var/tmp/silicon-peek-<uid>/peekd.sock`. The directory is mode 0700 and owned by the user; the socket is 0600. Both ends call `getpeereid` and require the same uid. A single-instance lock (`peekd.lock`) ensures one helper; a stale socket is removed only after the lock is held. Tests override the path with `PEEK_DAEMON_SOCKET`.

**Framing.** One UTF-8 JSON object per line (at most 1 MiB, duplicate keys rejected). If the object has `"bin":[n1,n2,…]`, exactly `n1+n2+…` raw bytes follow the newline: the frame's blobs, in order. Limits: an image blob 10 MiB, a drawing 256 KiB, a WAV 4 MiB, a TTS chunk 64 KiB, 40 MiB of binary per frame. Requests time out after 30 s, except `send` with `wait` and `register.drawing` (120 s).

**Envelope.**

```jsonc
{"v":1,"id":"<uuid>","op":"<name>", …fields}                         // request
{"v":1,"id":"<uuid>","ok":true,"result":{…}}                          // reply
{"v":1,"id":"<uuid>","ok":false,"error":{"code","message","hint","retryable","details"}}
{"v":1,"event":"<name>", …fields}                                     // event (no id)
```

Unknown ops are refused as `unknown_op`. Unknown fields are ignored, so fields can be added without a new protocol version.

**Handshake**, the first frame on every connection:

```jsonc
// CLI → peekd
{"v":1,"id":"…","op":"hello","role":"cli","cli_version":"0.1.1","protocols":[1],
 "platform":"macos-aarch64","bundled_app":{"build":1001,"short_version":"0.1.1","zip_path":"/…/Peek.app.zip","zip_sha256":"…"}}
// peekd → CLI
{"v":1,"id":"…","ok":true,"result":{"protocol":1,"peekd_version":"0.1.1","app":{"build":1001,"ui_running":true}}}
// UI → peekd
{"v":1,"id":"…","op":"hello","role":"ui","app_build":1001,"app_version":"0.1.1","protocols":[1]}
```

If the protocol sets do not intersect, peekd answers `cli_outdated` (fix: `honeycomb update 'peek'`). If peekd is the older one, the CLI offers its bundled app (`app.offer`) and retries for up to 30 s, then fails with `app_update_pending` (retryable).

**Authentication.** Every CLI op except `hello` and `daemon.status` carries:

```json
"auth":{"home":"/abs/canonical/SILICON_HOME/.peek","home_token":"<64 hex>","api_url":"https://backend.peek.teamofsilicons.com","context":"production"}
```

peekd checks that the home is a private directory owned by the user, that `<home>/daemon-token` equals `home_token` (constant-time), and that `<home>/session.json` holds a valid session for `"<api_url>#<context>"`. The actor and org come from that session, never from request fields.

**CLI ops:** `attach`, `detach`, `status`, `register.side`, `register.drawing`, `unregister`, `send`, `ask.get`, `ask.list`, `ask.cancel`, `history`, `config.sync`, `telemetry`, `app.offer`, `daemon.status`, plus `app.uninstall` and `doctor`, which the CLI degrades gracefully when a helper answers `unknown_op`. Their results are what the matching CLI commands print ([CLI reference](cli.md)).

**`send --wait` and `ask.result.ack`.** With `"wait":true` the `send` reply arrives as usual, and the connection then stays open. When the ask closes (answered, dismissed, expired or cancelled), peekd pushes an `ask.result` event on that connection. Writing the event into the socket does not prove the CLI read it: the CLI may be timing out or exiting at that moment. So the CLI confirms on the same connection:

```jsonc
// peekd → CLI (event)
{"v":1,"event":"ask.result","ask_id":"ask_…","state":"answered","answer":{"kind":"single_choice","option_id":"delete","label":"Delete"},
 "via":"click","transcript":null,"answered_at":"…Z"}
// CLI → peekd (request, fire and forget: the CLI prints the answer without waiting for the reply)
{"v":1,"id":"…","op":"ask.result.ack","ask_id":"ask_…","auth":{…}}
// peekd → CLI
{"v":1,"id":"…","ok":true,"result":{}}
```

`ask.result.ack` exists only in this spot, right after an `ask.result` on a `send --wait` connection. Anywhere else peekd answers `unknown_op`. The answer counts as delivered to the CLI only if a matching `ask.result.ack` arrives within 2 seconds of the event. The delivery then shows `status: "wait"` and no Ting event is sent. If the ack does not come (EOF, a timeout, any other frame, or a different `ask_id`), or the CLI closes the connection before the ask closes (its `--wait` timed out, or ^C), the answer goes out as a normal `peek.ask.answered` Ting event. An answer is therefore never lost, and at worst a script that crashed right after printing also gets the ting.

**`config.sync` and telemetry.** `config.sync` mirrors `{"voice","language","notify","telemetry"}` into peekd. `telemetry` is the home's *effective* setting as the CLI sees it: `false` also when only the CLI's environment opts out (`PEEK_TELEMETRY`, `SPACE_STATION_TELEMETRY`, `SILICON_TELEMETRY`). The additive field `"env_opt_out":true` marks that case. Every CLI command that talked to peekd for its home under such an environment sends one more `config.sync` at the end. peekd then records nothing about the home and sends `X-Peek-Telemetry: off` on its backend calls for it. An environment opt-out ends when a CLI of that home hands peekd a `telemetry` batch again, since the CLI only does that when neither its config nor its environment opts out. A config opt-out never ends this way, and peekd always reads the home's `config.json` as well. `--no-telemetry` is per process and is not forwarded ([Telemetry](telemetry.md#the-helper-and-environment-opt-outs)).

**UI traffic.** peekd sends the app `slots.state`, `peek.show`, `peek.cancel`, `tts.begin`/`tts.chunk`/`tts.end`/`tts.error` (24 kHz mono s16le PCM), `stt.result` and `restarting` events, and the requests `drawing.validate`, `drawing.load`, `app.update.prepare`, `app.quit`, `app.uninstall` and `doctor`. The app sends `answer`, `voice.submit` (the WAV), `message`, `dismissed`, `speech.done`, `shown.done`, `focus`, `drawing.error`, `telemetry`, `settings.changed`, `ui.status` and `presence`. peekd also checks that the peer process of a `ui` connection is `…/Peek.app/Contents/MacOS/Peek`.

- **`voice.submit` names its message.** The reply is `{"message_id":"cmsg_…"}` for a Carbon voice message (`ask_id` null) and `{"message_id":null}` for a voice answer. The later `stt.result` carries the same `message_id` (or the `ask_id`), so the app matches outcomes by id, never by arrival order. peekd starts transcribing only after the reply is queued on the connection, so the `stt.result` never arrives before the reply that names its message, even when it is immediate (silence gives `empty` without any upload). The app still keeps an outcome whose reply it has not handled yet, and applies it when the reply comes. With an older peekd whose reply is `{}`, it falls back to matching messages in order.
- **`message`** (a typed Carbon message) replies `{"message_id":"cmsg_…"}` too.
- **`presence`** tells peekd whether anyone can see the screen: `{"op":"presence","available":bool,"reason":"ok"|"locked"|"asleep"|"display_off"}`, reply `{}`. The app sends it right after its `hello` and on every change (`com.apple.screenIsLocked` / `com.apple.screenIsUnlocked`, `NSWorkspace` screens did sleep / wake, session resign / become active). While `available` is false peekd pushes no `peek.show` and no `tts.*`: new sends stay in the per-position queue (the `send` result has `status:"queued"` and a `carbon_away` warning), asks keep their expiry clock, and speech starts only once the bubble is actually shown. When `available` turns true, queued sends are pushed in order. `status` reports `"carbon":{"available","reason"}`. An app that never sends `presence` (older than 0.1.1) is treated as available.
- **`ui.status`** is additive. The app pushes what only it knows after every successful `hello` and whenever a value changes: `{"mic":"granted|denied|restricted|undetermined","hotkeys":{"modifier","registered":[…],"failed":[…],"problems":[…]},"glass","services","app_build","app_version"}`. peekd keeps the latest report for that connection and replies `{}`. For `peek doctor`, peekd first asks the app live with a `doctor` request (answered with the same object) and reports `"ui_status_source":"live"`. If that request fails (an app that is busy or too old), it falls back to the last push and reports `"ui_status_source":"pushed"`. Every field is optional and read leniently. An older peekd answers `ui.status` with `unknown_op`, and the app then stops pushing until the next connection.

## Where state lives

| Scope | Path | Contents |
|---|---|---|
| per Silicon home | `$SILICON_HOME/.peek/` | `session.json`, `session.lock`, `daemon-token`, `config.json`, `testing.json` |
| per macOS user | `~/Library/Application Support/Peek/` (`PEEK_SUPPORT_DIR`) | `peekd.sqlite` (no tokens), `settings.json`, `drawings/`, `cache/`, `recordings/`, update `offers/`, logs |
| per macOS user | `~/Library/Caches/Peek/` (`PEEK_CACHES_DIR`) | Simulation's generated samples and other disposable caches |
| backend | `/var/lib/peek/peek.sqlite`, `testing.sqlite` | see [Privacy](privacy.md) |

Every row in peekd's database is keyed by `context`, so production and test data never mix. Deliveries are queued per `(api_url, context, org_id, actor_id)`, and one actor's rows are never sent with another actor's token.

## Backend API (v1)

| Method and path | Auth | Purpose |
|---|---|---|
| `GET /healthz`, `GET /readyz` | – | liveness (`{"status":"ok","service":"peek","version"}`); readiness of the databases, IAM, Ting and Deepgram configuration |
| `GET /api/v1/iam` | – (test key optional) | discovery: `app_id`, `api_version`, URLs, `testing_environment{id,name,generation}`, `compatibility` |
| `POST /api/v1/auth/login` | – | `{"slt"}` → session |
| `POST /api/v1/auth/refresh` | – | `{"refresh_token"}` → session |
| `POST /api/v1/auth/logout` | optional Bearer | `{"token","revoke_ting"?}` → 204. Revokes the IAM refresh family. The Silicon's Ting grant is revoked too only with `"revoke_ting":true` and the Bearer (`peek logout --revoke-ting`), because every home of the Silicon shares it |
| `GET /api/v1/auth/me` | Bearer | the introspected identity, display name, org role, scopes, Ting enrollment |
| `POST /api/v1/ting/recipient` | Bearer | enroll as a Ting recipient |
| `POST /api/v1/deliveries` | Bearer | deliver one event through Ting |
| `POST /api/v1/speech/token` | Bearer | `{"purpose":"tts"\|"stt"}` → how to reach Deepgram: `mode` `direct` or `proxy` (below) |
| `POST /api/v1/speech/speak` | Bearer | proxy mode: `{"text","model","sample_rate"}` → Deepgram's `linear16` audio, streamed |
| `POST /api/v1/speech/listen` | Bearer | proxy mode: the recording (at most 4 MiB) → Deepgram's transcription JSON |
| `PUT`/`GET`/`DELETE /api/v1/drawings/current` | Bearer | the Silicon's drawing copy |
| `GET`/`PUT`/`DELETE /api/v1/orgs/{org}/byo/deepgram` | Bearer (writes: org owner or admin) | the org's own Deepgram key |
| `POST /api/v1/reports` | optional Bearer | bug reports (10 per hour per IP) |
| `POST /api/web/telemetry` | – (origin check, rate limit) | the telemetry gateway ([Telemetry](telemetry.md#the-gateway)) |
| `POST /webhooks/iam` | HMAC | IAM membership events |
| `PUT`/`GET /internal/honeycomb/…/operations/{op}` | service token | the testing-environment lifecycle participant |

"Bearer" means `Authorization: Bearer oat_…` plus `X-Org-ID: <org>`, introspected live on every request. Every POST requires an `Idempotency-Key` (16–255 visible ASCII), except the telemetry gateway (browsers never send one; events are deduplicated by id) and the IAM webhook (deduplicated by event id). Errors use `{"error":{"code","message","hint","retryable","request_id","details"?}}`; every response carries `X-Request-ID` and `Cache-Control: no-store`, and `Retry-After` when waiting helps.

### Speech tokens and the relay

`POST /api/v1/speech/token` decides, per key, how peekd reaches Deepgram:

```jsonc
// direct: the key can mint tokens (Deepgram POST /v1/auth/grant succeeded)
{"mode":"direct","access_token":"<JWT>","expires_in":60,"base_url":"https://api.deepgram.com",
 "key_source":"peek"|"org","params":{"mip_opt_out":true,"tags":["peek","production"]}}
// proxy: Deepgram answered 401 or 403 to the grant for this key (no access_token)
{"mode":"proxy","base_url":"https://backend.peek.teamofsilicons.com/api/v1/speech","expires_in":600,
 "key_source":"peek"|"org","params":{"mip_opt_out":true,"tags":["peek","production"]}}
```

- The backend remembers a "grant forbidden" verdict per key for 10 minutes, so it does not ask Deepgram for a token on every request; a proxy reply's `expires_in` is the time left on that verdict (at most 600 s). An org's own key (`peek org byo deepgram`) follows the same logic; there is still no fallback between keys. Test contexts use the test key.
- In proxy mode `base_url` is `<PEEK_PUBLIC_ORIGIN>/api/v1/speech`, and peekd calls, with the same Bearer session:
  - `POST …/speak` with `{"text","model","sample_rate"?}`: `text` 1–2000 characters, `model` an Aura-2 voice such as `aura-2-thalia-en`, `sample_rate` one of 8000, 16000, 24000 (the default), 32000 or 48000. The backend calls Deepgram `/v1/speak` (`encoding=linear16`, `container=none`) and streams the audio back unbuffered, passing Deepgram's `dg-request-id` and `dg-char-count` headers.
  - `POST …/listen` with the raw recording as the body (`Content-Type: audio/wav`, at most 4 MiB; larger is `413 payload_too_large`). Only these query parameters are accepted and forwarded to Deepgram `/v1/listen`: `model` (default `nova-3`), `language`, `detect_language` (repeatable, at most 16), `keyterm` (repeatable, at most 100 of 1–100 characters), `numerals`, `smart_format`; anything else is `400 invalid_input` with `details.allowed`. The answer is Deepgram's JSON, with its `dg-request-id`.
  - Both are ordinary Bearer POSTs, so they also need an `Idempotency-Key`; they are never replayed.
- In both modes every Deepgram request carries `mip_opt_out=true`, `tag=peek` and a tag for the environment; in proxy mode the backend adds them itself.
- Tokens and relayed calls share one rate budget: 120 per minute per Silicon and 1,200 per minute per org (`429 rate_limited` with `Retry-After`). The relay never logs or stores the audio, the text or the transcript.
- peekd caches the token reply (and so the mode) per API URL, context, org, actor and purpose until it expires, and asks again after a Deepgram 401.
- Speech failures are `503 speech_unavailable` with `details.reason`: `not_configured`, `org_key_invalid`, `org_out_of_credits`, `org_model_forbidden`, `peek_key_invalid`, `peek_out_of_credits`, `rate_limited` or `deepgram_unavailable`.

## Build on top of peek

- **From any language**, drive the CLI: every command has `--json`, stable error codes and documented exit codes. `peek commands --json` describes the whole tree.
- **From Rust**, depend on [`silicon-peek-client`](https://crates.io/crates/silicon-peek-client) for the wire types and validators (the show and ask schemas, Ting data types) and the stateless backend client. Enable the `runtime` feature only if you need the store, session refresh or the IPC client.
- **React to the Carbon** by routing the [Ting events](ting.md) in your flow.

## Contribute

1. Reproduce the bug with the smallest command, and capture `peek doctor --json`.
2. Fork the repository, fix it on a branch, and add a test that fails without your fix. Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` (and `npm run build` in `web/` if you changed docs).
3. Open a pull request against `main`, then tell us about it:
   ```sh
   peek report "what broke, how to reproduce it, and what the fix changes" \
     --pr https://github.com/teamofsilicons/silicon-peek/pull/<n> --attach-status
   ```

A report without a fix is welcome too. The more exact the reproduction, the faster it is fixed.

## Release (maintainers)

One version number is used everywhere: every Cargo package, `honeycomb.yaml`, the app's `CFBundleShortVersionString` and the git tag (`v0.1.1`). The release script builds the CLI for all six Honeycomb targets (macOS universal, Linux musl, Windows MSVC static), a universal peekd and Peek.app, signs them with Developer ID and notarizes the app, packs `Peek.app.zip` into both macOS payloads, validates and packs the Honeycomb archive, and runs a loopback install test exactly as Stemcell would. The package ships only the `peek` command; the app is installed by `peek app install` or the first `peek login`.
