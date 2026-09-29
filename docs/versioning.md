# Versioning and compatibility

peek runs as many pieces that update on their own schedules: a CLI copy in every Silicon home, one Peek.app and helper per Mac, one backend, and the Stemcell flows that consume its events. This page says which contracts are versioned, how the pieces agree on a version, which combinations are supported, and how anything is ever retired.

## One version number

Every Cargo package, `honeycomb.yaml`, the app's `CFBundleShortVersionString` and the git tag share one version (`0.1.4`, tag `v0.1.4`). The app's build number is derived from it (`major × 1,000,000 + minor × 1,000 + patch`, so 0.1.4 is build 1004) and only ever increases. `peek --version`, `peek iam --json` (`version`) and `peek daemon status` (`version`, `ui.build`) report it.

The package version describes a release. Compatibility is decided by the **contract versions** below, not by comparing package versions.

## Versioned contracts

| Contract | Current | Where it is declared | Changes within a version |
|---|---|---|---|
| HTTP API | `v1` | the `/api/v1/…` path; `api_version` in `peek iam --json` and `GET /api/v1/iam` | additive only: new routes, new optional request fields, new response fields |
| IPC protocol (CLI and app ↔ helper) | `1` | `"v":1` on every frame; `protocols` in the `hello` handshake; the helper's capabilities in `features` | additive only: new ops, new optional fields (unknown fields are ignored), new `features` |
| Local store (`session.json`, `config.json`, `testing.json`) | `schema: 1` | the `schema` field in each file | additive only |
| Helper database (`peekd.sqlite`) | schema `2` (since 0.1.2) | SQLite `user_version` | internal to the helper, which migrates it forward in place. The helper never downgrades, and an older one refuses a newer database with `store_schema_newer` |
| Ting event data | `schema: 1` | `data.schema` in every event | additive only: consumers must ignore unknown fields |
| Ting type names | the nine `peek.*` types | [Ting events](ting.md) | never renamed or removed (Ting types cannot be deleted) |
| Drawing API | `peek`, `ctx`, `input` | [Drawing the visual](drawing.md) | additive only: new `input` fields, new `ctx` extensions |
| CLI output | JSON shapes, error codes, exit codes | [CLI reference](cli.md) | additive only: new fields, new codes; an existing code keeps its meaning and exit code |

A change that is not additive (removing or renaming a field, changing its type or meaning, tightening a limit, changing an exit code) is **breaking**, and ships only as a new major contract version: `/api/v2`, IPC protocol `2`, store or data `schema: 2`.

## How the pieces agree

- **CLI ↔ helper.** The first frame on every connection is `hello`, carrying the CLI's `protocols` list. The helper picks the highest common protocol. With none in common, it answers `cli_outdated`, and the hint is `honeycomb update 'peek'`. If the helper is the older side, the CLI offers the app bundled in its own package (`app.offer`); the helper installs it when the screen is idle, and the CLI retries for up to 30 s before failing with `app_update_pending` (retryable, exit 5).
- **Features within protocol 1.** The helper's `hello` reply lists what it supports in `features` (0.1.2: `queue_v2`, `expiry_all`, `replace`, `schedule`, `notify_shown`; absent from older helpers, which means none). A CLI refuses an option its helper does not list with `app_update_pending` and `details.missing_features`, instead of letting an older helper silently ignore it. The other way round, the helper knows each CLI's version from its `hello` and answers a CLI older than 0.1.2 in words it knows: `slot_busy` instead of `queue_full`, and the ask state `cancelled` instead of `replaced`.
- **CLI and helper ↔ backend.** `GET /api/v1/iam` returns `"compatibility":{"cli":">=0.1.0, <1.0.0","ipc_protocols":[1]}`. The CLI sends `Peek-Client-Version` on login.
- **Helper ↔ backend speech.** Google TTS always uses `/api/v1/speech/speak`: the backend relays Gemini SSE events and the helper decodes PCM as they arrive. `POST /api/v1/speech/token` is retained as compatibility discovery and returns `proxy` without exposing provider credentials. OpenAI transcription uses `/api/v1/speech/listen` for a completed recording and returns a final transcript. See [Development](development.md#speech-tokens-and-the-relay).
- **Stores.** A writer that finds a `schema` newer than it understands refuses to touch the file with `store_schema_newer` (exit 4, hint `honeycomb update 'peek'`), instead of corrupting it.
- **Drawings.** A drawing that reads an `input` field an older app does not have gets `undefined`. Write drawings with `?.` so they degrade instead of throwing.

## Compatibility matrix

| peek CLI | Helper and Peek.app | Backend API | Honeycomb | macOS | Status |
|---|---|---|---|---|---|
| 0.1.x | 0.1.x (IPC 1) | v1 | ≥ 0.5.0 | 26.0 or newer | supported |
| 0.1.2 | 0.1.0 or 0.1.1 | v1 | ≥ 0.5.0 | 26.0 or newer | supported; plain sends behave as the old helper does, and 0.1.2 options fail with `app_update_pending` until Peek.app updates |
| 0.1.0 or 0.1.1 | 0.1.2 | v1 | ≥ 0.5.0 | 26.0 or newer | supported; sends queue in order (a new show no longer replaces the visible one), a full queue is `slot_busy`, a replaced ask reads `cancelled` |
| 0.1.x | newer, still speaks IPC 1 | v1 | ≥ 0.5.0 | 26.0 or newer | supported |
| 0.1.x | newer, IPC 1 dropped | – | – | – | `cli_outdated`: update the CLI |
| newer | 0.1.x | v1 | ≥ 0.5.0 | 26.0 or newer | the CLI offers its app; `app_update_pending` until installed |
| 0.1.x on Linux or Windows | – | v1 | ≥ 0.5.0 | – | IAM commands only ([Platforms](platforms.md)) |

External contracts peek relies on:

| System | Requirement | Why |
|---|---|---|
| Silicon Stemcell | 5.0.2 contract (`<app> iam --json`, `login <SLT>`, `login status --json`, `logout`, `config set`) | installs and logs peek in on connect |
| Honeycomb | 0.5.0 or newer | package installs and updates, and the macOS install script (package format 1, Manifest A); the installer refuses older versions |
| IAM | 4.0.0 (bare app ids such as `peek`) | the backend uses `silicon-iam-client` 4.0.0 |
| Ting | ≥ 0.1.6 for type registration | the nine event types |

## Skew is normal

Each Silicon home has its own CLI copy, updated by Honeycomb; a Carbon may have one more; there is one helper and app per Mac. So at any moment several CLI versions talk to one helper. The rules that keep this safe:

- **The newest app wins, and the app never downgrades.** The helper considers every build offered by any home and installs the highest.
- **An app update never interrupts.** The helper asks the app to prepare and waits (up to 6 hours) until no bubble, recording or ask is on screen. An ask on screen is never interrupted.
- **A CLI never kills the helper.** Newer code reaches the helper only through `app.offer`.
- **The backend ships first.** The backend validates every event with the same strict types as the client library, so a backend that knows the 0.1.2 event types and fields is deployed, and the new Ting types are registered, before any 0.1.2 helper exists.
- **Refresh is deterministic.** Two processes refreshing the same session derive the same idempotency key, so IAM replays one rotation instead of revoking the family.

Update cadence:

| What | Updated by | When |
|---|---|---|
| `peek` CLI in each home | Honeycomb's per-home updater | every minute |
| the same, as a fallback | the helper's CLI watchdog | hourly; if a home's CLI has been behind the newest release for more than 2 hours, it runs `honeycomb update 'peek'` for that home (Settings: `updates.cli_watchdog`) |
| Peek.app and the helper | the helper | at start, hourly, when a newer offer arrives, and on `peek app update` |

`peek update` prints this as JSON and never replaces anything itself.

## The Honeycomb package

`honeycomb.yaml` uses package format 1 with Manifest A: both macOS targets declare `install_script: "install-app.sh"`, and the Linux and Windows targets have none.

```yaml
targets:
  macos-aarch64:   { root: "targets/macos-aarch64",   executables: { cli: "bin/peek" }, install_script: "install-app.sh" }
  macos-x86_64:    { root: "targets/macos-x86_64",    executables: { cli: "bin/peek" }, install_script: "install-app.sh" }
  linux-x86_64:    { root: "targets/linux-x86_64",    executables: { cli: "bin/peek" } }
```

- Honeycomb (0.5.0 or newer) runs the script after every `honeycomb install` that changes the installed version, never on `honeycomb update`. It installs Peek.app into `~/Applications` from the build bundled in the package, only after `codesign` verifies the Developer ID signature for `ai.tos.peek`, and starts it when someone is logged in at the desktop (Peek.app starts too when it was already installed; it is a no-op if it runs).
- It never replaces an existing Peek.app: updates stay the helper's job, and the newest offered build still wins.
- It always exits 0, so an install never fails because of it. Each step is logged as `peek install-app: [ok|skip|degraded] <step>: <detail>` to Honeycomb's stderr and to `~/Library/Application Support/Peek/install-status.txt`. `PEEK_INSTALL_NO_LAUNCH=1` skips the launch.
- `peek app install`, `peek login` and every command that needs the app keep installing it as a fallback.

## Deprecation and sunset policy

1. **Additive changes** can ship in any release, and are listed in the release notes.
2. **A breaking change** ships as a new major contract, next to the old one. It is announced in the release notes and on this page, with the migration steps, before the old version's sunset date is set.
3. **Support windows.**
   - IPC: the helper keeps supporting the previous protocol for at least one minor release after the new one ships.
   - HTTP API: the previous major stays available for at least 90 days after the new major ships, and until the supported CLI range (`compatibility.cli`) no longer includes clients that need it.
   - Ting types: a type is never removed. If one had to change incompatibly, a new type name would be introduced and both would be sent for at least 90 days.
   - Store schema: every release can read the previous schema and migrates it on its next write.
4. **After sunset**, an old client gets a precise error naming the command that fixes it (for example `cli_outdated` with `honeycomb update 'peek'`), never a silent failure.
5. **Scopes are frozen for v1.** Adding an IAM scope would force every Silicon to log in again, so new scopes wait for a major version.

## Contract tests

Changes are checked against the consumers that depend on them: the Stemcell app contract (the exact `iam --json`, `login status` and `logout` behaviours), the Ting data schemas, the IPC frames of the previous protocol, and the documented CLI outputs. The docs build fails if any advertised `peek docs` topic is missing.

## Changelog

### 0.1.4

- Streamed Google Gemini 3.8 Flash TTS replaces Deepgram speech. Choose Google voices and set reusable voice instructions through `peek config set`, or override them per send with `--voice-instructions`.
- Speech supports expressive delivery instructions and Peek's accent shorthand, such as `<indian accent>Anuv Jain</indian accent>`. The CLI manuals include Google's expression guidance.
- OpenAI `gpt-transcribe` replaces Deepgram transcription for recorded voice answers and messages, with language and vocabulary hints and bounded retries.
- Provider keys stay on the backend. Production and testing use separate credentials; legacy Deepgram keys are no longer used for speech. Peek adds no TTS concurrency cap.
- Update the CLI and Peek.app together for the new speech providers. Existing Deepgram voice preferences migrate to Google defaults; drawings, text sends, stored history and login sessions remain compatible.

### 0.1.3

- Peek panels explicitly join other applications' full-screen Spaces without activating Peek.
- `peek config set` accepts `position` (1–8) and `drawing` (JavaScript file path) as defaults. Configuration saves preferences; a send or explicit registration claims a position. Relative drawing paths are saved as absolute paths, and existing registrations take precedence.
- `peek register side` and `peek register drawing` can apply their configured defaults without an argument.
- Custom drawings are optional: Peek.app uses the built-in initial logo for immediate and scheduled sends. This requires the 0.1.3 app and helper; older helpers still require a custom drawing.
- Drawing glass is clipped to its outline so its shadow no longer ends at the square canvas boundary.
- Relative scheduled delays start after registration and drawing validation complete.

### 0.1.2

Queue:

- **Always queue.** A new send never replaces what is on screen; it waits its turn, strictly first in, first out, per Silicon. The old rule "a new show replaces a visible show" is gone.
- **One on screen plus five waiting.** One more fails with the new error `queue_full` (exit 4, retryable), also while the Carbon is away or paused Peek. CLIs older than 0.1.2 get it as `slot_busy`.
- **Manage the queue:** `peek queue` (`list`, `clear [--all]`) and `peek cancel <SEND_ID>`, with the new errors `send_not_found` and `schedule_not_found`. No Ting events for your own removals.
- **`--replace`** takes over the Silicon's bubble at once, even from an ask (new ask state and close reason `replaced`, no Ting event; a `--wait` prints `replaced`).
- **Expiry on every send:** `--expires-in` now takes durations (`15m`, `1h30m`, plain seconds as before) and works on speak and show sends too; new `--expires-at <DATETIME>`. New Ting type `peek.send.expired`; `peek.ask.expired` gains `shown`.

Scheduling:

- **`peek send --in <DURATION>` or `--at <DATETIME>`** (with `--tz <IANA>`; times without an offset use the Mac's time zone) schedules a one-time send, up to 365 days ahead and 500 per Silicon (`schedule_full`). Scheduled sends fire after sleep or downtime unless expired, and wait for room instead of failing when the queue is full.
- **`peek schedule list|cancel|clear`.** `peek unregister` and `peek logout` also cancel scheduled and queued sends.
- **New Ting types** `peek.schedule.due` (outcome `shown`, `queued`, `expired` or `replaced`) and `peek.send.shown` (always for scheduled sends, opt-in with `--notify shown`).

Peek.app:

- A `+N` badge next to the down-arrow counts the Silicon's waiting bubbles.
- Esc reaches a bubble for 3 s after it appears and while the pointer is over it, without Input Monitoring: one Esc closes a show (speech continues), two stop the speech too (gesture `esc_double`); one Esc folds a question into a compact question with a `^` button to expand it, and a second Esc dismisses it ("Esc again to dismiss").
- The down-arrow always dismisses a question; it never folds it.
- Bubbles are prepared off screen (glass, backdrop sample, first drawing frame) before they slide in, so they no longer snap from grey to colour after landing; backdrops are sampled as soon as a send arrives.
- A speak-only bubble closes when it has slid back, not when the speech ends.

Distribution and protocol:

- `honeycomb install 'peek'` installs and starts Peek.app on a Mac (Manifest A install script).
- The helper's `hello` announces `features`; its database moves to schema 2 (a 0.1.1 helper refuses it). The UI protocol gains the `shown` op, the `queue.state` event, `presence.paused` and new `peek.show` fields ([Development](development.md#the-local-ipc-protocol-version-1)).
- Fixes: `peek register drawing` no longer says "previous drawing still active" when there was none or with `--check`, and marks excessive glass rebuilds with ⚠.

### 0.1.1

- Peek.app ships as `ai.tos.peek`, Developer ID signed, notarized and stapled; the helper registers on first run.
- Presence: bubbles wait while the Carbon's screen is locked or asleep (`carbon_away`), then show in order.
- `peek logout` keeps the Silicon's Ting grant (`--revoke-ting` removes it); `ting_not_enrolled` warning.
- A bubble slides back 1.5 s after its speech; duplicate option labels are refused; `invalid_input` names `details.field`; `--lang` takes BCP 47 tags; an invalid `ISI` is ignored with a warning; `peek report --dry-run`.

### 0.1.0

- First release: Peek.app with eight positions, Liquid Glass bubbles and QuickJS drawings; the peekd helper; the `peek` CLI with the Stemcell app contract; the peek backend; the website and these docs.
