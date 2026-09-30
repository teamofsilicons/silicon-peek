# Development

peek is open source: https://github.com/teamofsilicons/silicon-peek. This page explains how the pieces fit, how to build and test each one, the local protocol between them, and how to contribute a fix. If you only use peek, you do not need it.

## Architecture

| Component | Language | Runs | Owns |
|---|---|---|---|
| **Peek.app** (`ai.tos.peek`) | Swift 6, SwiftUI + AppKit | one per macOS user, `~/Applications/Peek.app`, menu-bar only | the 8 position panels, the glass and drawing compositor, one QuickJS VM per drawing, audio playback and microphone capture, hotkeys, backdrop sampling, Settings, Simulation. **No tokens and no network**: everything goes through peekd. |
| **peekd** (`Contents/Helpers/peekd`) | Rust | one per macOS user, a launchd agent registered by the app | positions, the send queues and scheduled sends, expiry, asks and history, the delivery outbox, streamed ElevenLabs TTS playback, OpenAI transcription requests, per-home session refresh, the telemetry relay, Peek.app self-update, the stale-CLI watchdog |
| **peek CLI** (`peek`) | Rust | one copy per Silicon home (installed by Honeycomb), plus an optional Carbon copy | the IAM app contract (`iam`, `login`, `status`, `logout`, `config`), the per-home store `$SILICON_HOME/.peek/`, input validation, reading image and drawing bytes |
| **silicon-peek-client** | Rust library | linked into the CLI, peekd and the server | a stateless HTTP client for the backend, all wire types (IPC, show and ask schemas and validators, delivery bodies, Ting data), error types. Feature `runtime` adds the store, session refresh, locks and the IPC client. |
| **peek-server** | Rust (axum, SQLite) | `https://backend.peek.teamofsilicons.com` | the IAM app secret, SLT exchange, refresh and revoke, bearer introspection, Ting proofs and sends, Ting enrollment, the Deepgram key and temporary TTS token grants, the OpenAI transcription key and completed-recording relay, legacy org key storage, drawing copies, bug reports, the IAM webhook, the Honeycomb lifecycle participant, the telemetry gateway |
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
                           │ HTTPS, Bearer oat_ + X-Org-ID
                           ▼
                  peek-server (service keys)
                   ├─ TTS access: /api/v1/speech/token → Deepgram /v1/auth/grant
                   │    temporary JWT → peekd connects directly to Voice Agent → ElevenLabs v4
                   ├─ OpenAI gpt-transcribe: /api/v1/speech/listen → /v1/audio/transcriptions
                   │    completed WAV → final transcript
                   ├─ IAM, Space Station, GitHub
                   └─ Ting → ting-daemon → Silicon webhook → your flow
```

Design choices that shape everything:

- **The helper lives inside the app**, not in the Honeycomb package, so there is one stable path per macOS user. Honeycomb deletes package directories on every update, and every Silicon home has its own package copy.
- **Client custody of tokens.** Each Silicon's IAM session lives in its own home; the backend stores none. See [IAM and sessions](iam.md).
- **ElevenLabs TTS and OpenAI transcription are driven by peekd.** The helper gets a temporary token from Peek, streams ElevenLabs audio directly from Deepgram, and sends PCM chunks to the app for playback. Completed recordings go through the backend to OpenAI; only final transcripts return to the UI. TTS content never reaches the Peek backend, and the OpenAI relay never stores or logs content. See [Speech tokens and the relay](#speech-tokens-and-the-relay).
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

Tests never need keys or the network. Local HTTP fakes replace the backend, IAM, Ting and OpenAI; a local WebSocket fake supplies the Deepgram voice stream. Stores live in temporary directories, and the helper's socket is overridden with `PEEK_DAEMON_SOCKET`. Never point a test at your real `~/.peek`, `~/.silicon-iam` or `~/Library/Application Support/Peek`. To run the real binaries by hand, use the [isolated run mode](#isolated-run-mode).

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

**Real speech in an isolated run.** Set `PEEK_DEEPGRAM_API_KEY` and `PEEK_OPENAI_API_KEY` in the server's process environment before starting it; keep test phrases short because requests are billed:

```sh
PEEK_ELEVENLABS_AGENT_URL=wss://agent.deepgram.com/v1/agent/converse \
PEEK_OPENAI_BASE_URL=https://api.openai.com \
cargo run -p silicon-peek --bin peek-server
```

TTS needs a `PEEK_DEEPGRAM_API_KEY` with Member permissions or higher to grant temporary tokens; the OpenAI key enables microphone transcription. Test contexts use the separate `PEEK_DEEPGRAM_TEST_API_KEY` and `PEEK_OPENAI_TEST_API_KEY`, never production-key fallback. Do not put keys into tracked files, fixtures or logs. Peek grants a short-lived token for direct TTS and relays OpenAI transcription; neither path exposes a long-lived provider API key to clients.

For `scripts/e2e/run-local.sh` or `scripts/e2e/e2e.py`, export `PEEK_DEEPGRAM_API_KEY` for TTS and `PEEK_OPENAI_API_KEY` for transcription before starting. These scripts pass the keys only to the isolated server, without saving them in `env.sh`, `app.env`, logs or stack metadata; the isolated server does not load the repository's `.env`. `--no-stt` skips transcription while keeping configured ElevenLabs TTS available.

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
{"v":1,"id":"…","op":"hello","role":"cli","cli_version":"0.1.2","protocols":[1],
 "platform":"macos-aarch64","bundled_app":{"build":1002,"short_version":"0.1.2","zip_path":"/…/Peek.app.zip","zip_sha256":"…"}}
// peekd → CLI
{"v":1,"id":"…","ok":true,"result":{"protocol":1,"peekd_version":"0.1.2","app":{"build":1002,"ui_running":true},
 "features":["queue_v2","expiry_all","replace","schedule","notify_shown"]}}
// UI → peekd
{"v":1,"id":"…","op":"hello","role":"ui","app_build":1002,"app_version":"0.1.2","protocols":[1]}
```

If the protocol sets do not intersect, peekd answers `cli_outdated` (fix: `honeycomb update 'peek'`). If peekd is the older one, the CLI offers its bundled app (`app.offer`) and retries for up to 30 s, then fails with `app_update_pending` (retryable).

**Features.** Protocol 1 grows additively, and an older peekd ignores fields it does not know, so the `hello` reply names the capabilities peekd has in `features` (missing from peekd 0.1.1 and older, which means none):

| Feature | Covers |
|---|---|
| `queue_v2` | always-queue FIFO with `queue_full`; the ops `queue.list`, `queue.clear`, `send.cancel` |
| `expiry_all` | `expires_in_s` and `expires_at` on every send kind |
| `replace` | `send.replace` |
| `schedule` | `send.due_at`; the ops `schedule.list`, `schedule.cancel`, `schedule.clear` |
| `notify_shown` | `notify` value `shown` and the `peek.send.shown` event |

The CLI checks them before it sends and fails with `app_update_pending` (`details.missing_features`, `details.peekd_version`) rather than lose an option. It drops `shown` from the `notify` it mirrors in `config.sync` when `notify_shown` is missing. In the other direction, peekd remembers each CLI connection's `cli_version`: for a CLI older than 0.1.2 (or an unparsable version) it answers `slot_busy` instead of `queue_full`, and reports the ask state `replaced` as `cancelled` in `ask.result`, `ask.get`, `ask.list` (where a `cancelled` filter then also matches replaced asks) and `history`.

**Authentication.** Every CLI op except `hello` and `daemon.status` carries:

```json
"auth":{"home":"/abs/canonical/SILICON_HOME/.peek","home_token":"<64 hex>","api_url":"https://backend.peek.teamofsilicons.com","context":"production"}
```

peekd checks that the home is a private directory owned by the user, that `<home>/daemon-token` equals `home_token` (constant-time), and that `<home>/session.json` holds a valid session for `"<api_url>#<context>"`. The actor and org come from that session, never from request fields.

**CLI ops:** `attach`, `detach`, `status`, `register.side`, `register.drawing`, `unregister`, `send`, `ask.get`, `ask.list`, `ask.cancel`, `history`, `config.sync`, `telemetry`, `app.offer`, `daemon.status`, plus `app.uninstall` and `doctor`, which the CLI degrades gracefully when a helper answers `unknown_op`, and since 0.1.2 `queue.list`, `queue.clear` (`{"all":bool}`), `send.cancel` (`{"target":"snd_…|ask_…|sch_…"}`), `schedule.list`, `schedule.cancel` (`{"target":"sch_…|snd_…"}`) and `schedule.clear`. Their results are what the matching CLI commands print ([CLI reference](cli.md)).

**`send` (0.1.2 fields).** Every new field is optional and left out when unused, so a plain send is byte-for-byte what a 0.1.1 CLI sends: `expires_in_s` (now any kind), `expires_at` (RFC 3339; not with `expires_in_s`), `due_at` (schedule instead of queueing), `tz` (the IANA zone the CLI read `--at` in; display only, at most 64 characters of `[A-Za-z0-9_+-/]`) and `replace` (bool). The CLI validates strictly; peekd validates again with 5 s of slack on the lower bounds for the time the request took. The reply always carries `queue_position`, `waiting`, `expires_at`, `schedule_id`, `due_at`, `tz` and `replaced_send_id` (null when they do not apply), and `status` may be `scheduled`.

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

**`config.sync` and telemetry.** `config.sync` mirrors `{"voice","voice_instructions","language","notify","telemetry"}` into peekd. `telemetry` is the home's *effective* setting as the CLI sees it: `false` also when only the CLI's environment opts out (`PEEK_TELEMETRY`, `SPACE_STATION_TELEMETRY`, `SILICON_TELEMETRY`). The additive field `"env_opt_out":true` marks that case. Every CLI command that talked to peekd for its home under such an environment sends one more `config.sync` at the end. peekd then records nothing about the home and sends `X-Peek-Telemetry: off` on its backend calls for it. An environment opt-out ends when a CLI of that home hands peekd a `telemetry` batch again, since the CLI only does that when neither its config nor its environment opts out. A config opt-out never ends this way, and peekd always reads the home's `config.json` as well. `--no-telemetry` is per process and is not forwarded ([Telemetry](telemetry.md#the-helper-and-environment-opt-outs)).

**UI traffic.** peekd sends the app `slots.state`, `peek.show`, `peek.cancel`, `queue.state`, `tts.begin`/`tts.chunk`/`tts.end`/`tts.error` (24 kHz mono s16le PCM), `stt.result` and `restarting` events, and the requests `drawing.validate`, `drawing.load`, `app.update.prepare`, `app.quit`, `app.uninstall` and `doctor`. The app sends `answer`, `voice.submit` (the WAV), `message`, `dismissed`, `shown`, `speech.done`, `shown.done`, `focus`, `drawing.error`, `telemetry`, `settings.changed`, `ui.status` and `presence`. peekd also checks that the peer process of a `ui` connection is `…/Peek.app/Contents/MacOS/Peek`.

- **`peek.show`** carries, besides the content, `queued_behind` (how many of the Silicon's sends wait behind this one: the first value of the `+N` badge), `expires_at` for every kind (asks only before 0.1.2), `replaces` (the send this one takes over after a `--replace`; the app got `peek.cancel` with `"reason":"replaced"` for it just before) and `schedule_id`. peekd pushes the Silicon's current send only; the rest wait in peekd.
- **`queue.state`** (`{"slot","context","send_id","waiting"}`) updates the badge of the bubble showing `send_id` whenever the Silicon's waiting count changes; `waiting: 0` hides it. The app ignores it for any other `send_id`.
- **`peek.cancel`** `reason` is `cancelled_by_silicon`, `unregistered`, `expired` or `replaced`. The bubble slides away, its speech stops, and nothing is sent back.
- **`shown`** (`{"op":"shown","send_id"}`, reply `{}`) tells peekd that the app began presenting a bubble (its off-screen preparation started; it is on screen within 0.6 s). It is sent at most once per send and never for a summoned bubble. peekd then sets `shown_at`, starts the speech, arms the bubble's watchdog and, for `--notify shown` or a scheduled send, records `peek.send.shown`. For an app older than build 1002, which never sends `shown`, peekd does all of that when it pushes `peek.show`, as 0.1.1 did.
- **`speech.done` and `shown.done`.** `speech.done` only reports that playback ended (`peek.speech.finished`); it never closes a send. `shown.done` closes it, for a speak-only bubble after its 1.5 s slide-back too (close reason `speech_done`), and the watchdog covers a lost `shown.done`.
- **`dismissed`** `gesture` is `esc`, `esc_double` (two Escs within 0.4 s on a show; the speech stops too), `down_arrow` or `down_arrow_double`.

- **`voice.submit` names its message.** The reply is `{"message_id":"cmsg_…"}` for a Carbon voice message (`ask_id` null) and `{"message_id":null}` for a voice answer. The later `stt.result` carries the same `message_id` (or the `ask_id`), so the app matches outcomes by id, never by arrival order. peekd starts transcribing only after the reply is queued on the connection, so the `stt.result` never arrives before the reply that names its message, even when it is immediate (silence gives `empty` without any upload). The app still keeps an outcome whose reply it has not handled yet, and applies it when the reply comes. With an older peekd whose reply is `{}`, it falls back to matching messages in order.
- **`message`** (a typed Carbon message) replies `{"message_id":"cmsg_…"}` too.
- **`presence`** tells peekd whether anyone can see the screen: `{"op":"presence","available":bool,"reason":"ok"|"locked"|"asleep"|"display_off","paused":bool}`, reply `{}`. The app sends it right after its `hello` and on every change (`com.apple.screenIsLocked` / `com.apple.screenIsUnlocked`, `NSWorkspace` screens did sleep / wake, session resign / become active, and the Carbon pausing or resuming peeks). While `available` is false peekd pushes no `peek.show` and no `tts.*`: new sends stay in the per-Silicon queue (the `send` result has `status:"queued"` and a `carbon_away` warning), expiry clocks keep running, and speech starts only once the bubble is actually shown. When `available` turns true, queued sends are pushed in order, and every change wakes the timers, so scheduled sends that came due during sleep fire at once. `paused` (default `false`) only changes wording: peekd still pushes while paused (so ⌃⌘N can summon a waiting ask), reports `carbon_paused` and `held:"paused"`, and the app holds the bubbles. `status` reports `"carbon":{"available","reason","paused"}`. An app that never sends `presence` (older than 0.1.1) is treated as available.
- **`ui.status`** is additive. The app pushes what only it knows after every successful `hello` and whenever a value changes: `{"mic":"granted|denied|restricted|undetermined","hotkeys":{"modifier","registered":[…],"failed":[…],"problems":[…]},"glass","services","app_build","app_version"}`. peekd keeps the latest report for that connection and replies `{}`. For `peek doctor`, peekd first asks the app live with a `doctor` request (answered with the same object) and reports `"ui_status_source":"live"`. If that request fails (an app that is busy or too old), it falls back to the last push and reports `"ui_status_source":"pushed"`. Every field is optional and read leniently. An older peekd answers `ui.status` with `unknown_op`, and the app then stops pushing until the next connection.

## Where state lives

| Scope | Path | Contents |
|---|---|---|
| per Silicon home | `$SILICON_HOME/.peek/` | `session.json`, `session.lock`, `daemon-token`, `config.json`, `testing.json` |
| per macOS user | `~/Library/Application Support/Peek/` (`PEEK_SUPPORT_DIR`) | `peekd.sqlite` (no tokens), `settings.json`, `drawings/`, `cache/`, `recordings/`, update `offers/`, logs |
| per macOS user | `~/Library/Caches/Peek/` (`PEEK_CACHES_DIR`) | Simulation's generated samples and other disposable caches |
| backend | `/var/lib/peek/peek.sqlite`, `testing.sqlite` | see [Privacy](privacy.md) |

peekd's database is at schema 2 since 0.1.2: `sends` gained `expires_at`, `queued_at`, `overflow` (a due scheduled send waiting for room), `schedule_id` and `due_at`, and the `scheduled` table holds scheduled sends until they fire (payload, pre-assigned `send_id` and `ask_id`, `due_at`, `expires_at`, `tz`, `replace_current`). Queues are rebuilt from `sends` after a restart, in order, and overdue scheduled sends fire on the first timer pass, oldest `due_at` first. Timers use wall-clock time and wake at least every 15 s while anything is scheduled or can expire, so a jump after sleep is caught up at once. Every row in peekd's database is keyed by `context`, so production and test data never mix. Deliveries are queued per `(api_url, context, org_id, actor_id)`, and one actor's rows are never sent with another actor's token.

## Backend API (v1)

| Method and path | Auth | Purpose |
|---|---|---|
| `GET /healthz`, `GET /readyz` | – | liveness (`{"status":"ok","service":"peek","version"}`); readiness of the databases, IAM, Ting, ElevenLabs TTS and OpenAI transcription configuration |
| `GET /api/v1/iam` | – (test key optional) | discovery: `app_id`, `api_version`, URLs, `testing_environment{id,name,generation}`, `compatibility` |
| `POST /api/v1/auth/login` | – | `{"slt"}` → session |
| `POST /api/v1/auth/refresh` | – | `{"refresh_token"}` → session |
| `POST /api/v1/auth/logout` | optional Bearer | `{"token","revoke_ting"?}` → 204. Revokes the IAM refresh family. The Silicon's Ting grant is revoked too only with `"revoke_ting":true` and the Bearer (`peek logout --revoke-ting`), because every home of the Silicon shares it |
| `GET /api/v1/auth/me` | Bearer | the introspected identity, display name, org role, scopes, Ting enrollment |
| `POST /api/v1/ting/recipient` | Bearer | enroll as a Ting recipient |
| `POST /api/v1/deliveries` | Bearer | deliver one event through Ting |
| `POST /api/v1/speech/token` | Bearer | `{"purpose":"tts"\|"stt"}` → TTS: temporary token with `provider:"elevenlabs"`, `mode:"direct"`; STT: `provider:"openai"`, `mode:"proxy"` without credentials (below) |
| `POST /api/v1/speech/listen` | Bearer | completed recording (at most 4 MiB) → final OpenAI transcript in Peek JSON |
| `PUT`/`GET`/`DELETE /api/v1/drawings/current` | Bearer | the Silicon's drawing copy |
| `GET`/`PUT`/`DELETE /api/v1/orgs/{org}/byo/deepgram` | Bearer (writes: org owner or admin) | legacy org Deepgram key management; unused by current speech |
| `POST /api/v1/reports` | optional Bearer | bug reports (10 per hour per IP) |
| `POST /api/web/telemetry` | – (origin check, rate limit) | the telemetry gateway ([Telemetry](telemetry.md#the-gateway)) |
| `POST /webhooks/iam` | HMAC | IAM membership events |
| `PUT`/`GET /internal/honeycomb/…/operations/{op}` | service token | the testing-environment lifecycle participant |

"Bearer" means `Authorization: Bearer oat_…` plus `X-Org-ID: <org>`, introspected live on every request. Every POST requires an `Idempotency-Key` (16–255 visible ASCII), except the telemetry gateway (browsers never send one; events are deduplicated by id) and the IAM webhook (deduplicated by event id). Errors use `{"error":{"code","message","hint","retryable","request_id","details"?}}`; every response carries `X-Request-ID` and `Cache-Control: no-store`, and `Retry-After` when waiting helps.

### Speech tokens and the relay

**TTS:** The helper calls `POST /api/v1/speech/token` with `{"purpose":"tts"}`, the Silicon's Bearer session and an `Idempotency-Key`. The backend uses Deepgram's `POST /v1/auth/grant` and returns `provider: "elevenlabs"`, `mode: "direct"`, `access_token`, `expires_in`, `key_source: "peek"` and `base_url: "wss://agent.deepgram.com/v1/agent/converse"`. The helper keeps the temporary token only in memory and opens that WebSocket with `Authorization: Bearer <token>`. No TTS text or audio goes through the Peek backend; there is no `/api/v1/speech/speak` route.

The helper pins `eleven_v4`, configures `agent.speak.provider` with `type: "eleven_labs"`, `model_id` and `voice_id`, and requests 24 kHz mono signed 16-bit little-endian PCM with no container. After `SettingsApplied`, it sends the requested speech using `InjectAgentMessage`. Instructions become a leading audio cue; native square-bracket cues remain inline. Angle-tag markup is rejected. The normalized primary language is passed as `language_code` when known. The helper sets `mip_opt_out: true` and `flags.history: false`, forwards binary PCM to the app over IPC, and requires `AgentAudioDone` before it caches a completed stream. Voice, instructions and language are part of the cache identity. An early close fails synthesis.

Text over 300 characters is split at natural sentence boundaries once a piece reaches 90 characters, outside square-bracket cues, and queued in order. The original text is preserved, and the request's delivery instructions prefix every piece. Short messages stay together. This reduces the wait for long narration with sentence breaks; unbroken passages or long cues can still delay first audio, and inline delivery cues may not carry between pieces. The helper sends `KeepAlive` every 4 seconds throughout synthesis and waits for the queued messages and final `AgentAudioDone` before completing.

Speech text and optional voice instructions are each limited to 1–2000 characters. Voice IDs allow 1–128 ASCII letters, digits, underscores and hyphens; George is `JBFqnCBsd6RMkjVDRZzb`. Replace old Google/Aura names with an ElevenLabs ID; there is no silent voice substitution. Cancellation closes the upstream connection. The helper allows 50 seconds for first audio, 35 seconds without audio during a stream, 180 seconds overall, and at most 20 MiB of PCM. Peek adds no TTS concurrency cap; provider account limits govern upstream capacity.

`PEEK_DEEPGRAM_API_KEY` serves production; `PEEK_DEEPGRAM_TEST_API_KEY` serves test contexts, with no production fallback. The key needs Member permissions or higher. `PEEK_ELEVENLABS_AGENT_URL` accepts the canonical Voice Agent URL or a loopback WebSocket for local tests. `PEEK_DEEPGRAM_BASE_URL` controls token grants (default `https://api.deepgram.com`). `PEEK_DEEPGRAM_TOKEN_TTL_SECONDS` defaults to 30 and accepts 1–3600. The token only needs to be valid at the WebSocket handshake: an established connection can finish after token expiry. See [Deepgram's token authentication guide](https://developers.deepgram.com/guides/fundamentals/token-based-authentication).

**STT:** `POST /api/v1/speech/listen` takes a completed recording as the raw body (`Content-Type: audio/wav`, at most 4 MiB; larger gives `413 payload_too_large`), the Silicon's Bearer session and an `Idempotency-Key`. The server calls OpenAI `/v1/audio/transcriptions` as multipart with `model: "gpt-transcribe"`, then returns a final, provider-neutral object:

```json
{"text":"Keep the second one.","request_id":"req_…","detected_language":"en"}
```

`request_id` and `detected_language` are omitted when unavailable. Peek uses recorded-file transcription, not Realtime transcription or partial transcript events. The Mac applies its existing answer matching to the completed transcript.

The accepted query parameters remain `model` (omitted or `gpt-transcribe`), `language`, `detect_language` (repeatable, at most 16), `keyterm` (repeatable, at most 100 of 1–100 characters), `numerals` and `smart_format`. Language hints are normalized to lowercase primary subtags and sent as OpenAI's `languages[]`; keyword hints become `keywords[]`. `numerals=true` supplies a digit-formatting prompt. `smart_format` is accepted for compatibility and is not forwarded. Other parameters or models are rejected. See the [OpenAI file-transcription guide](https://developers.openai.com/api/docs/guides/speech-to-text) for the upstream API.

`PEEK_OPENAI_API_KEY` serves production and `PEEK_OPENAI_TEST_API_KEY` serves test contexts, with no fallback between them. `POST /api/v1/speech/token` with `purpose: "stt"` remains compatibility discovery: it returns `provider: "openai"`, `mode: "proxy"`, `key_source: "peek"` and the Peek speech routing URL, without an access token. There is no direct provider path or Deepgram fallback. Existing org Deepgram keys remain manageable but are never selected for TTS or transcription.

Token and transcription relay calls share the request rate budget: 120 per minute per Silicon and 1,200 per minute per org (`429 rate_limited` with `Retry-After`). The backend never stores or logs transcripts or audio. Provider failures become `speech_unavailable`; see [Troubleshooting](troubleshooting.md).

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

One version number is used everywhere: every Cargo package, `honeycomb.yaml`, the app's `CFBundleShortVersionString` and the git tag (`v<version>`). The release script builds the CLI for all six Honeycomb targets (macOS universal, Linux musl, Windows MSVC static), a universal peekd and Peek.app, signs them with Developer ID and notarizes the app, packs `Peek.app.zip` into both macOS payloads, validates and packs the Honeycomb archive, and runs a loopback install test exactly as Stemcell would. The package ships only the `peek` command, and its two macOS targets carry `install-app.sh` (Manifest A): Honeycomb runs it after `honeycomb install 'peek'`, and it installs the bundled Peek.app after verifying its Developer ID signature, then starts it ([Versioning](versioning.md#the-honeycomb-package)). `peek app install` and the first `peek login` remain the fallback. The loopback test requires the install script to succeed on macOS.

Order matters when a release adds Ting types or event fields: deploy the backend built with the new client library first (it validates event data strictly), register the new Ting types in production and every testing environment (`scripts/testing/bootstrap.sh` does the testing ones), and only then publish the crates, the app and the Honeycomb package.
