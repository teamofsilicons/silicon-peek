# Silicon Peek

Quick, voice-first exchanges between Carbons and Silicons on a Mac, with no conversation to open.

A Silicon (an AI agent) claims one of eight positions around the edge of the screen and registers a
small JavaScript drawing for its bubble. From then on it can use the `peek` CLI to do three things:
speak a sentence, show up to three text or image elements, or ask one self-contained question
(text, single choice, multiple choice, slider or range). The bubble slides in with Liquid Glass and
plays the speech. The Carbon (the human) answers by voice, by keyboard or with a click, and the
answer goes back to the asking Silicon as a [Ting](https://ting.teamofsilicons.com) event.

Some things peek is good for:

- a cleanup Silicon asking "Delete ~/Downloads/old.zip?";
- a DJ Silicon showing cover art, title, artist and year for ten seconds;
- a reminder that pops up and goes away again.

Nothing here needs a chat history. History stays on the Mac.

- **Website and docs:** https://peek.teamofsilicons.com (docs at `/docs`, the installer at `/install.sh`)
- **Backend:** https://backend.peek.teamofsilicons.com
- **IAM application:** `peek` in org `tos` · **Honeycomb:** `honeycomb install 'peek'`
- **Rust client:** [`silicon-peek-client`](https://crates.io/crates/silicon-peek-client)

## Install

Run one command on macOS 26 or newer:

```sh
curl -fsSL https://peek.teamofsilicons.com/install.sh | sh
```

It installs [Honeycomb](https://honeycomb.teamofsilicons.com) if it is missing, then the `peek` CLI,
then `Peek.app` into `~/Applications`, and starts it in the menu bar. No login happens here. On
Linux and Windows, `honeycomb install 'peek'` installs just the CLI. It implements the full IAM
contract (`iam`, `login`, `status`, `logout`, `config`). Mac-bound commands on those systems answer
`platform_unsupported`.

## Quick start

### For Carbons

1. Allow the microphone when Peek first asks. A recording is made only while you answer by voice, and
   it goes to Deepgram (directly, or through the peek backend's relay, which never stores or logs it).
2. Press **ctrl+cmd+1 … ctrl+cmd+8** (⌃⌘1…⌃⌘8) to open a Silicon's bubble. Position 1 is top centre,
   then clockwise. The modifier is configurable in Settings (cmd, opt+cmd, shift+cmd and others).
3. Press `\` to answer by voice, or just start typing and press Return. Esc cancels.
4. The down arrow closes a bubble. A double click also stops the speech.
5. The menu bar item holds Settings (normal or compact mode, hotkeys, voices, telemetry) and Simulation.

### For Silicons

Stemcell: add `peek` to `apps` in `silicon.yaml`, and `silicon connect` handles login. By hand:

```sh
peek iam --json                                   # {"app_id":"peek", …}; offline, no side effects
iam silicon-login --app-id peek --grant-org <org> --approve-scopes   # prints a short-lived token (SLT)
peek login '<SLT>'
peek register side 3                              # one position per Silicon; taken positions list the free ones
peek register drawing ./logo.js                   # optional; otherwise Peek uses its built-in visual
peek send --speak "Clean-up finished" --show '{"elements":[{"type":"text","text":"12 GB freed"}]}'
peek send --speak "Delete old.zip?" \
  --ask '{"question":"Delete ~/Downloads/old.zip?","type":"single_choice","options":["Keep","Delete"]}'
```

Answers arrive as Ting events of type `peek.ask.answered`. The other event types are
`peek.ask.dismissed`, `peek.ask.expired` and `peek.message.received`, plus two opt-ins:
`peek.speech.finished` and `peek.show.dismissed`. Route them in your flow on `metadata.isi`.
`peek send --ask … --wait` returns the answer on stdout instead of sending a ting.

Save defaults with `peek config set '{"position":3,"drawing":"./logo.js"}'` (omit `drawing` for the built-in visual).
CLI sends use these defaults when a position or drawing has not been registered; explicit registrations win.

Every command explains itself: `peek --help`, `peek <command> --help`, `peek commands --json`,
`peek docs <topic>`. Report bugs with `peek report "<what happened>" [--pr <url>]`.

## Architecture

```
 Silicon (ISI) ──$ peek send …──┐                 Carbon (hotkeys, mic, keyboard, clicks)
 Stemcell ──$ peek login/status─┤                               │
                                ▼                               ▼
          peek CLI (one per SILICON_HOME)             Peek.app (SwiftUI + AppKit, QuickJS, audio)
            $SILICON_HOME/.peek/session.json                    │  ~/Applications/Peek.app
            (the Silicon's own IAM tokens)                      │
                  │ Unix socket, role=cli                       │ Unix socket, role=ui
                  ▼                                             ▼
        /var/tmp/silicon-peek-<uid>/peekd.sock ◄──► peekd (Peek.app/Contents/Helpers, one per OS user)
                                                    queue, asks, history, outbox, self-update
                  ┌─────────────────────────────────────────┴───────────────┐
                  ▼ HTTPS, Bearer <Silicon's token>                           ▼ direct mode: HTTPS, ≤60 s JWT
        peek-server (Rust/axum on EC2, holds the app secret) ─proxy mode─► Deepgram (TTS Aura-2, STT Nova-3)
         ├─ IAM: SLT exchange, refresh, introspection, OBO proofs
         ├─ Ting: recipient enrollment and delivery → the Silicon's flow
         ├─ Deepgram: short-lived JWT minting (direct mode) or a streaming relay (proxy mode)
         └─ Space Station telemetry gateway, drawing copies, bug reports
```

- **Client custody.** A Silicon's IAM tokens live only in its own `$SILICON_HOME/.peek/`. The
  backend stores no IAM tokens. It mints one Ting OBO proof per delivery, with the Silicon as both
  actor and recipient.
- **One app per OS user.** `peekd` ships inside Peek.app and runs as a launchd agent. Honeycomb
  installs the CLI per Silicon home. The CLI offers the bundled `Peek.app.zip`, and peekd updates the
  app: the newest build wins and it never downgrades.
- **Speech has two modes, picked by the backend per Deepgram key.** peekd first asks peek-server
  for a speech token (`POST /api/v1/speech/token`). In **direct** mode the backend mints a Deepgram
  token that lives at most 60 seconds and peekd calls Deepgram itself, so audio never touches the
  backend. In **proxy** mode (a key that cannot mint tokens) peekd sends the speak text or the
  finished recording to peek-server, which relays it to Deepgram and streams the answer straight
  back: the audio passes through peek-server but is **never stored or logged** (only request
  metadata is recorded). Every Deepgram request carries `mip_opt_out=true` in both modes.
- **Testing environments** use the same code paths as production (`peek --test <env-uuid> …`), and
  test bubbles carry a TEST pill.

The IPC protocol, state layout and every decision are documented in `docs/development.md` and
`docs/versioning.md`.

## Repository layout

| Path | What |
|---|---|
| `src/`, `migrations/` | `peek-server`, the backend (package `silicon-peek`) |
| `crates/client/` | `silicon-peek-client`: stateless HTTP client and wire types; feature `runtime` adds the store, refresh and IPC |
| `crates/cli/` | `silicon-peek-cli`, bin `peek`; `docs/` inside it is a generated copy of `/docs` |
| `crates/daemon/` | `silicon-peek-daemon`, bin `peekd` (macOS only) |
| `apps/mac/` | Peek.app: `project.yml` (xcodegen), Swift sources, vendored QuickJS |
| `web/` | SolidJS website and prerendered docs; `web/public/install.sh` |
| `docs/` | canonical manuals (usage and development) |
| `deploy/` | CloudFormation stack, systemd units, Caddy, backups, `deploy.py` and the [operator runbook](deploy/README.md) |
| `scripts/` | release builds, Honeycomb packaging, docs sync, installer, operator toolbox, testing-environment bootstrap |
| `honeycomb.yaml` | Honeycomb package manifest (Manifest B: CLI on six targets, Peek.app as a macOS payload) |
| `application.json.example` | the Honeycomb/IAM app registration, without secrets |

## Build from source

Toolchain: Rust 1.98 (`rust-toolchain.toml`), Xcode 26.4 with the macOS 26.4 SDK, xcodegen 2.46,
Node 24, and Python 3.11 or newer.

```sh
cargo test --workspace                              # client, CLI, daemon (macOS) and server
cargo run -p silicon-peek-cli -- iam --json
(cd apps/mac && xcodegen generate && xcodebuild -project Peek.xcodeproj -scheme Peek \
   -configuration Debug -destination 'platform=macOS' -derivedDataPath build build)   # dev build: ai.tos.peek.dev
(cd web && npm ci && npm run dev)                   # http://127.0.0.1:4317
python3 scripts/sync-cli-docs.py                    # after editing docs/
```

A release candidate is one command: `scripts/build-release.sh`. It builds the six CLI targets and
the universal, signed Peek.app, then stages and validates the Honeycomb package, packs it, and
installs it against a loopback fake API. Nothing is uploaded. Publishing and the backend deployment
follow [deploy/README.md](deploy/README.md).

## Links

- Docs: https://peek.teamofsilicons.com/docs
- IAM: https://github.com/teamofsilicons/silicon-iam
- Ting: https://github.com/teamofsilicons/silicon-ting
- Honeycomb: https://github.com/teamofsilicons/silicon-honeycomb
- Stemcell: https://github.com/teamofsilicons/silicon-stemcell
- Issues and fixes: https://github.com/teamofsilicons/silicon-peek (or `peek report --pr <url>`)

## License

[MIT](LICENSE) © 2026 Team of Silicons
