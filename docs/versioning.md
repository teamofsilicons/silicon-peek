# Versioning and compatibility

peek runs as many pieces that update on their own schedules: a CLI copy in every Silicon home, one Peek.app and helper per Mac, one backend, and the Stemcell flows that consume its events. This page says which contracts are versioned, how the pieces agree on a version, which combinations are supported, and how anything is ever retired.

## One version number

Every Cargo package, `honeycomb.yaml`, the app's `CFBundleShortVersionString` and the git tag share one version (`0.1.0`, tag `v0.1.0`). The app's build number is derived from it (`major × 1,000,000 + minor × 1,000 + patch`, so 0.1.0 is build 1000) and only ever increases. `peek --version`, `peek iam --json` (`version`) and `peek daemon status` (`version`, `ui.build`) report it.

The package version describes a release. Compatibility is decided by the **contract versions** below, not by comparing package versions.

## Versioned contracts

| Contract | Current | Where it is declared | Changes within a version |
|---|---|---|---|
| HTTP API | `v1` | the `/api/v1/…` path; `api_version` in `peek iam --json` and `GET /api/v1/iam` | additive only: new routes, new optional request fields, new response fields |
| IPC protocol (CLI and app ↔ helper) | `1` | `"v":1` on every frame; `protocols` in the `hello` handshake | additive only: new ops, new optional fields (unknown fields are ignored) |
| Local store (`session.json`, `config.json`, `testing.json`) | `schema: 1` | the `schema` field in each file | additive only |
| Ting event data | `schema: 1` | `data.schema` in every event | additive only: consumers must ignore unknown fields |
| Ting type names | the six `peek.*` types | [Ting events](ting.md) | never renamed or removed (Ting types cannot be deleted) |
| Drawing API | `peek`, `ctx`, `input` | [Drawing the visual](drawing.md) | additive only: new `input` fields, new `ctx` extensions |
| CLI output | JSON shapes, error codes, exit codes | [CLI reference](cli.md) | additive only: new fields, new codes; an existing code keeps its meaning and exit code |

A change that is not additive (removing or renaming a field, changing its type or meaning, tightening a limit, changing an exit code) is **breaking**, and ships only as a new major contract version: `/api/v2`, IPC protocol `2`, store or data `schema: 2`.

## How the pieces agree

- **CLI ↔ helper.** The first frame on every connection is `hello`, carrying the CLI's `protocols` list. The helper picks the highest common protocol. With none in common, it answers `cli_outdated`, and the hint is `honeycomb update 'peek'`. If the helper is the older side, the CLI offers the app bundled in its own package (`app.offer`); the helper installs it when the screen is idle, and the CLI retries for up to 30 s before failing with `app_update_pending` (retryable, exit 5).
- **CLI and helper ↔ backend.** `GET /api/v1/iam` returns `"compatibility":{"cli":">=0.1.0, <1.0.0","ipc_protocols":[1]}`. The CLI sends `Peek-Client-Version` on login.
- **Helper ↔ backend speech.** `POST /api/v1/speech/token` says in `mode` whether the helper calls Deepgram directly (`direct`, with a short-lived token) or through the backend's relay (`proxy`, `/api/v1/speech/speak` and `/listen`). Both are part of API v1; the backend picks per key, so the helper follows whatever the reply says ([Development](development.md#speech-tokens-and-the-relay)).
- **Stores.** A writer that finds a `schema` newer than it understands refuses to touch the file with `store_schema_newer` (exit 4, hint `honeycomb update 'peek'`), instead of corrupting it.
- **Drawings.** A drawing that reads an `input` field an older app does not have gets `undefined`. Write drawings with `?.` so they degrade instead of throwing.

## Compatibility matrix

| peek CLI | Helper and Peek.app | Backend API | Honeycomb | macOS | Status |
|---|---|---|---|---|---|
| 0.1.x | 0.1.x (IPC 1) | v1 | ≥ 0.5.0 | 26.0 or newer | supported |
| 0.1.x | newer, still speaks IPC 1 | v1 | ≥ 0.5.0 | 26.0 or newer | supported |
| 0.1.x | newer, IPC 1 dropped | – | – | – | `cli_outdated`: update the CLI |
| newer | 0.1.x | v1 | ≥ 0.5.0 | 26.0 or newer | the CLI offers its app; `app_update_pending` until installed |
| 0.1.x on Linux or Windows | – | v1 | ≥ 0.5.0 | – | IAM commands only ([Platforms](platforms.md)) |

External contracts peek relies on:

| System | Requirement | Why |
|---|---|---|
| Silicon Stemcell | 5.0.2 contract (`<app> iam --json`, `login <SLT>`, `login status --json`, `logout`, `config set`) | installs and logs peek in on connect |
| Honeycomb | 0.5.0 or newer | package installs and updates; the installer refuses older versions |
| IAM | 4.0.0 (bare app ids such as `peek`) | the backend uses `silicon-iam-client` 4.0.0 |
| Ting | ≥ 0.1.6 for type registration | the six event types |

## Skew is normal

Each Silicon home has its own CLI copy, updated by Honeycomb; a Carbon may have one more; there is one helper and app per Mac. So at any moment several CLI versions talk to one helper. The rules that keep this safe:

- **The newest app wins, and the app never downgrades.** The helper considers every build offered by any home and installs the highest.
- **An app update never interrupts.** The helper asks the app to prepare and waits (up to 6 hours) until no bubble, recording or ask is on screen. An ask on screen is never interrupted.
- **A CLI never kills the helper.** Newer code reaches the helper only through `app.offer`.
- **Refresh is deterministic.** Two processes refreshing the same session derive the same idempotency key, so IAM replays one rotation instead of revoking the family.

Update cadence:

| What | Updated by | When |
|---|---|---|
| `peek` CLI in each home | Honeycomb's per-home updater | every minute |
| the same, as a fallback | the helper's CLI watchdog | hourly; if a home's CLI has been behind the newest release for more than 2 hours, it runs `honeycomb update 'peek'` for that home (Settings: `updates.cli_watchdog`) |
| Peek.app and the helper | the helper | at start, hourly, when a newer offer arrives, and on `peek app update` |

`peek update` prints this as JSON and never replaces anything itself.

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
