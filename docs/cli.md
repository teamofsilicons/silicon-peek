# CLI reference

`peek` is one Rust binary. It is built for agents first: it never prompts, it prints exactly one JSON value with `--json`, and every error says what failed, why, and the command that fixes it. Humans get the same commands with readable output.

The CLI is a tree you can walk. `peek --help` shows the branches, `peek <command> --help` shows what a command is for, how it is used with its neighbours, and then its flags. `peek commands --json` prints the whole tree as data, and `peek docs <topic>` prints these pages offline.

## Command tree

```text
peek
├── iam [--json]                                   IAM discovery; offline; works before login
├── login [<SLT>] [--token-file <PATH|->] [--recover]  exchange an IAM short-lived token
│   └── status [--json]                            live-verified identity; {"authenticated":bool,…}
├── logout [--revoke-ting]                         revoke this home's peek session (the Ting grant only with --revoke-ting)
├── config
│   ├── set '<json-object>'                        strict merge; prints the resulting config
│   ├── show | get <key> | unset <key>
│   ├── telemetry on|off
│   └── home <DIR>                                 move this home's store (pointer file in the default home)
├── ting
│   └── enroll                                     (re)register this Silicon as a Ting recipient for peek
├── register
│   ├── side [1-8]                                 claim or move to a position (1 top, clockwise)
│   └── drawing [FILE.js] [--check] [--preview <OUT.png>] [--dump-frame <N>]
├── unregister                                     release the position, delete the drawing, cancel queued and scheduled sends
├── send [--speak <TEXT>] [--show <JSON|@FILE|->] [--ask <JSON|@FILE|->]
│        [--voice <VOICE_ID>] [--voice-instructions <TEXT>] [--lang <bcp47>] [--duration <SECS>]
│        [--expires-in <DURATION> | --expires-at <DATETIME>] [--in <DURATION> | --at <DATETIME>] [--tz <IANA>]
│        [--replace] [--notify speech_finished,show_dismissed,shown] [--wait[=<SECS>]]
├── queue                                          the send on screen and the ones waiting behind it (at most 5)
│   ├── list                                       same as `peek queue`
│   └── clear [--all]                              drop every waiting send; --all also the one on screen
├── cancel <SEND_ID>                               withdraw one send (snd_…, ask_… or sch_…): on screen, waiting or scheduled; no ting
├── schedule
│   ├── list                                       scheduled sends that are not due yet, soonest first
│   ├── cancel <ID>                                cancel one before it is due (sch_… or its snd_…)
│   └── clear                                      cancel every scheduled send of this Silicon
├── ask
│   ├── get <ASK_ID>                               local state and answer (works while Ting is not set up)
│   ├── list [--state pending|answered|dismissed|expired|cancelled|replaced] [--limit N]
│   └── cancel <ASK_ID>                            slide the ask away; no ting
├── history [--limit N] [--before <ID>]            local send history for this Silicon
├── status                                         position, drawing, queue, deliveries, app, daemon (one view)
├── org
│   └── byo deepgram (set --key-file <PATH|-> [--base-url URL] | show | delete)   legacy key management; org admins only
├── app
│   └── status | install | update | uninstall      Peek.app (install = ensure_app + launch)
├── daemon
│   └── status | restart
├── docs [<TOPIC>] [--search <TEXT>] [--all]       bundled offline manuals
├── commands [--json]                              machine-readable command tree
├── report <MESSAGE> [--pr <URL>] [--via backend|gh] [--attach-status] [--dry-run]
├── update                                         Honeycomb guidance JSON (never self-replaces)
├── doctor                                         diagnostics with exact fixes
└── --version | --help
```

## Global flags and environment

| Flag | Env | Meaning |
|---|---|---|
| `--json` | – | Exactly one JSON value on stdout on success. On failure stdout is empty and one error object goes to stderr. |
| `--org <ORG>` | `SILICON_ORG` | Org for org-specific calls. Precedence: `--org`, then `SILICON_ORG`, then the session's `org_id`. An empty value is invalid. |
| `--test <ENV_UUID>` | `SILICON_PEEK_TEST` | Use a saved testing environment ([Testing environments](testing.md)). A secret passed here is refused. |
| `--app-secret-file <PATH\|->` | – | peek's test app secret; `-` reads one line from stdin. |
| `--app-secret <ask_…>` | `PEEK_TEST_APP_SECRET` (hidden in help) | Same; conflicts with `--app-secret-file`. |
| `--api <URL>` | `PEEK_API_URL` | peek backend origin. Default `https://backend.peek.teamofsilicons.com`. HTTPS only, except `http` on loopback. |
| `--profile <NAME>` | `PEEK_PROFILE` | Saved account profile (`default` when omitted); use a separate profile for each account or organization. Each profile keeps production and testing worlds separately. |
| `--idempotency-key <K>` | – | Overrides the generated key of `peek ting enroll` and `peek report` only (16–255 visible ASCII). Every other command refuses it with `conflicting_flags`, because login, refresh and logout derive their keys so that a retry replays the same request. |
| `--no-telemetry` | `PEEK_TELEMETRY`, `SPACE_STATION_TELEMETRY`, `SILICON_TELEMETRY` (any of `0/false/off/no`) | Telemetry off for this process. The environment variables are also forwarded to the helper for this home (`config.sync`), so its later work for the home (deliveries, speech, drawing uploads) is not recorded either; the flag is not ([Telemetry](telemetry.md#the-helper-and-environment-opt-outs)). |
| `-q, --quiet` | `NO_COLOR` is honoured | No human hints or `Next:` lines on stderr (errors are still printed). peek never colours its output. |

Other environment the CLI reads:

| Variable | Use |
|---|---|
| `SILICON_HOME` | The store root: state lives in `$SILICON_HOME/.peek/`. Unset falls back to your real home (`~/.peek/`). Empty or non-existent is an error (`invalid_silicon_home`). |
| `ISI` | Optional. Recorded with each send and echoed as `metadata.isi` in Ting events. At most 160 characters on one line; an invalid value is dropped with an `isi_ignored` warning and never fails the send. Never grants authority. |
| `PEEK_DAEMON_SOCKET` | Tests and isolated runs only: overrides the helper's socket path ([Development](development.md#isolated-run-mode)). |
| `SILICON_HONEYCOMB` | The Honeycomb binary `peek doctor` checks, instead of the one on `PATH` or `~/.honeycomb/dir/system/bin`. |

The CLI **never** reads `IAM_TEST_APP_SECRET` or `IAM_TEST_KEY` (those belong to Ting), and **never prompts**. A value that must come from stdin (`-`) is refused when stdin is a terminal, and only one flag per command may read stdin.

## Output and errors

- Human mode prints readable text on stdout. Hints and `Next:` lines go to **stderr**, and only in human mode.
- With `--json`, stdout carries one compact value. Object keys are printed in alphabetical order; the examples on this page are ordered for reading. Errors go to stderr as one object:
  ```json
  {"error":{"code":"side_taken","message":"position 3 is held by si:dj; peek gives each Silicon exactly one position.",
   "hint":"choose a free one: peek register side 1 (free: 1,4,6,7)","retryable":false,
   "request_id":null,"details":{"owner":"si:dj","free":[1,4,6,7]}}}
  ```
- Human-mode errors read `error: <message>`, `hint: <hint>`, `help: peek <path> --help`, `request: <id>`.
- A missing required argument prints that command's full help after the error. With `--json`, a parse error is one error object instead.
- When a testing environment is selected, the last stderr line is always `Testing environment: <name> (<uuid>)`. stdout stays pure JSON.
- If your drawing fell back on screen, the next `peek send`, `peek register side` or `peek status` result carries a `drawing_fallback_active` warning with the error message and stack. It is attached once; other commands do not carry it, and `peek status` shows `drawing.active: false` until you register a drawing again.
- Results that can carry notes have a `warnings` array of `{"code","message","details"?}`. In human mode each warning is also printed to stderr as `warning: <message> (<code>)`.
- `invalid_input` and `invalid_json` errors carry `details.field`, the field that failed (for example the option whose label repeats another).

## Exit codes

| Exit | Meaning | Codes |
|---|---|---|
| 0 | success, including `login status` answering `authenticated:false` | – |
| 1 | internal or unexpected | `internal_error`, `store_corrupt`, `unexpected_response`, `ting_key_conflict`, `ting_rejected` |
| 2 | usage or invalid input | `invalid_input`, `invalid_json`, `conflicting_flags`, `nothing_to_send`, `text_too_long`, `caption_too_long`, `question_too_long`, `speak_too_long`, `too_many_elements`, `too_many_options`, `image_unreadable`, `image_too_large`, `image_unsupported`, `drawing_too_large`, `unknown_config_key`, `invalid_silicon_home`, `slt_is_public_id`, `idempotency_key_required`, `idempotency_conflict`, `frame_too_large`, `payload_too_large`, `telemetry_rejected` |
| 3 | not authenticated | `not_logged_in`, `session_rejected`, `slt_rejected`, `login_attempt_expired`, `reconsent_required`, `testing_secret_invalid`, `testing_generation_changed`, `unauthenticated`, `home_token_mismatch`, `authority_required` |
| 4 | refused, or a precondition is missing | `side_not_registered`, `side_taken`, `drawing_invalid`, `queue_full`, `slot_busy` (the name older peek versions use for `queue_full`), `send_not_found`, `schedule_not_found`, `schedule_full`, `ask_not_found`, `platform_unsupported`, `cli_outdated`, `store_schema_newer`, `private_application_organization_required`, `recipient_not_registered`, `not_org_admin`, `unknown_op`, `daemon_running`, `daemon_identity_mismatch`, `drawing_not_found`, `environment_not_prepared`, `idempotency_response_expired`, `not_found`, `lifecycle_conflict`, `origin_not_allowed`, `telemetry_table_unavailable` |
| 5 | unavailable or transport | `backend_unavailable`, `iam_unavailable`, `iam_misconfigured`, `daemon_unavailable`, `peek_service_unavailable`, `no_gui_session`, `app_update_pending`, `speech_unavailable`, `ting_unavailable`, `ting_type_missing`, `idempotency_in_progress`, `rate_limited`, `protocol_error` |

`telemetry_rejected`, `origin_not_allowed` and `telemetry_table_unavailable` come from the backend's telemetry gateway, and `lifecycle_conflict` from its testing-environment lifecycle endpoints; no `peek` command normally shows them. A code this build does not know (a newer backend or helper may add some) is passed through unchanged, and its exit code follows the HTTP status: 401 → 3; 403, 404, 409 → 4; 400, 413, 422 → 2; 408, 429, 5xx → 5; anything else → 1. `peek report --via gh` uses `gh_failed` when `gh` itself fails (exit 5, or 1 when the report could not even be handed to `gh`).

`retryable:true` in an error means the same command can succeed later without changes. Exit 5 is usually worth retrying with backoff (`no_gui_session` and `protocol_error` are not); exits 2 to 4 need a change first.

## peek iam

Static discovery. No network, no session, no side effects, exit 0. Stemcell calls it for every candidate binary.

```sh
peek iam --json
```

```json
{"app_id":"peek","org_id":"tos","name":"Peek","version":"0.1.2","api_version":"v1",
 "api_url":"https://backend.peek.teamofsilicons.com","iam_url":"https://backend.iam.teamofsilicons.com",
 "auth_url":"https://auth.iam.teamofsilicons.com","login_method":"short_lived_token","credential_issuer":false,
 "login":"Mint an SLT with `iam silicon-login --app-id peek --grant-org <org> --approve-scopes` (Silicon) or `iam login --app-id peek --grant-org <org>` (Carbon), then run `peek login '<SLT>'`.",
 "docs_url":"https://peek.teamofsilicons.com/docs","repository_url":"https://github.com/teamofsilicons/silicon-peek",
 "rust_package":"silicon-peek-client","cli_package":"silicon-peek-cli","install":"honeycomb install 'peek'",
 "platforms":{"full":["macos-aarch64","macos-x86_64"],"iam_only":["linux-x86_64","linux-aarch64","windows-x86_64","windows-aarch64"]}}
```

`api_url` follows `--api`, `PEEK_API_URL` or config `api_url` when set. With `--test`, it also includes `"testing":{"environment_id","name","generation"}`, read from the backend.

## peek login

Exchanges an IAM short-lived token (SLT) for this home's peek session. The SLT is single use and lives two minutes.

```sh
peek login "$SLT"
printf %s "$SLT" | peek login --token-file -     # reads one line, strips the trailing newline
peek login --recover                            # retry a lost exchange within 10 minutes
```

- Mint the SLT with `iam silicon-login --app-id peek --grant-org <org> --approve-scopes` (a Silicon) or `iam login --app-id peek --grant-org <org>` (a Carbon).
- There is no interactive prompt. Without an SLT, `--token-file` or `--recover` the command fails with `invalid_input` (exit 2, `details.missing_argument:"SLT"`) and prints its help.
- The exchange is idempotent: its key is derived from the SLT, and transport failures and 5xx are retried with the same key three times (after 1 s, 3 s and 9 s).
- On success it saves an ordinary IAM session and attaches this profile to a running helper. Ting permission review and enrollment are explicit, separate actions. On a Mac, it installs and starts Peek.app in the background after printing its result.
- Output: the authenticated `login status` shape below. An unenrolled account stays authenticated; review Ting permission with `peek ting authorize`, then complete it and enroll explicitly.
- `slt_rejected` (exit 3): the SLT was used, expired or invalid. Mint a new one. `slt_is_public_id` (exit 2): a public ID such as `si:x` works as an SLT only in a testing environment.
- `login_attempt_expired` (exit 3): `--recover` ran more than 10 minutes after the first attempt. Mint a new SLT.

### peek login status

```sh
peek login status --json
```

| Case | stdout | Exit |
|---|---|---|
| No session, or logged out | `{"authenticated":false,"id":null,"reason":"no_session"\|"logged_out","api_url":…,"testing_environment_id":…}` | 0 |
| Session rejected by IAM | `{"authenticated":false,"id":null,"reason":"rejected","rejection":{"code","at","request_id"},"api_url":…,"testing_environment_id":…,"next":"si auth setup peek   (or: iam silicon-login … --app-id peek …; peek login '<SLT>')"}` | 0 |
| Valid (verified live against the backend) | shape below | 0 |
| Backend or IAM unreachable | nothing on stdout; stderr `{"error":{"code":"backend_unavailable"\|"iam_unavailable",…,"retryable":true}}` | 5 |

```json
{"authenticated":true,"id":"si:dj","actor":{"type":"silicon","public_id":"si:dj"},"display_name":"DJ",
 "org_id":"tos","org_ids":["tos"],"membership_id":"si:dj[tos]","authority":"silicon","custody":"client",
 "scopes":["self.identity.read","self.membership.read","self.profile.read"],
 "reconsent_required":false,"access_expires_at":"2026-09-26T10:30:00Z","logged_in_at":"2026-09-26T10:00:00Z",
 "family_expires_at_estimate":"2029-03-15T10:00:00Z","refresh_pending":false,
 "ting":{"subscribed":true,"subscription_id":"sub_…"},
 "daemon":{"attached":true,"queued_answers":0,"authority_required":0},
 "testing_environment_id":null,"api_url":"https://backend.peek.teamofsilicons.com",
 "store":"/abs/SILICON_HOME/.peek","validated":true}
```

- `display_name` is `null` when IAM discloses none.
- `next` is added when there is something to do: the re-login command when `reconsent_required` is true, or `"peek ting enroll"` when `ting.subscribed` is false.
- A missing helper never changes `authenticated`; it only sets `daemon.attached:false`. The helper is never asked whether you are authenticated.
- `family_expires_at_estimate` is `logged_in_at + 900 days`, an estimate because IAM does not report refresh expiry.

## peek logout

Revokes this home's peek session, then cancels this home's undelivered answers on this Mac, and its queued and scheduled sends. The Silicon's Ting recipient grant is **kept**, because every home logged in as the same `si:` shares it; logging out one home never stops another home's answers.

```sh
peek logout                  # this home only; the Ting grant stays
peek logout --revoke-ting    # also remove this Silicon's Ting grant for peek
```

Prints `{"authenticated":false,"remote_revocation":"confirmed"|"pending"}` and exits **0** in both cases. After `--revoke-ting`, every home of this Silicon shows `"ting":{"subscribed":false}` in `peek login status`, its sends carry a `ting_not_enrolled` warning, and a fresh permission review followed by `peek ting enroll` restores delivery. `pending` means the local session is gone but the backend could not be reached; the revocation is retried by the next `peek` run. A home without a session also answers `confirmed`. `peek logout --help` exits 0 (Stemcell probes it).

## peek config

Configuration for this home, stored in `$SILICON_HOME/.peek/config.json`.

```sh
peek config set '{"notify":["speech_finished"],"voice":"JBFqnCBsd6RMkjVDRZzb"}'
peek config show --json      # {"schema":1,"telemetry":true,"voice":"JBFqnCBsd6RMkjVDRZzb","voice_instructions":null,"language":null,"notify":["speech_finished"],"api_url":null,"delivery_max_age_hours":168,"position":null,"drawing":null}
peek config get voice --json # {"key":"voice","value":"JBFqnCBsd6RMkjVDRZzb"}
peek config unset voice
peek config telemetry off
```

`set` parses strictly: a non-object, duplicate keys or an unknown key fail with exit 2 and `details.valid_keys`, and nothing is written. The object is merged atomically; `null` resets a key to its default. The resulting config is printed (the same shape as `show`) and pushed to a running helper. The helper gets the home's effective `telemetry`, which is `false` also when only the environment opts out. `unset` and `telemetry` are shortcuts for `set` and print the same. Stemcell runs `peek config set` after every login when `app_configs.peek` exists.

| Key | Type / validation | Default | Effect |
|---|---|---|---|
| `telemetry` | bool | `true` | this home's telemetry |
| `voice` | ElevenLabs voice ID, 1–128 ASCII letters, digits, `_` or `-`, or null | null (George) | default TTS voice; replace legacy Google/Aura names with an ElevenLabs ID |
| `voice_instructions` | string, 1–2000 characters, or null | null | delivery directions converted into a leading audio cue; best-effort performance |
| `language` | BCP 47 primary subtag (2–3 letters) or null | null (detect) | fallback TTS language hint when Peek detection is ambiguous |
| `notify` | array ⊆ `["speech_finished","show_dismissed","shown"]` | `[]` | default `--notify`. A helper older than 0.1.2 does not know `shown`; it is then left out, with a hint on stderr. |
| `api_url` | https URL or null | null | same as `--api` |
| `delivery_max_age_hours` | int 1..168 | 168 | how long this Silicon's undelivered events keep retrying |
| `position` | int 1..8 or null | null | position to claim before a send when none is registered |
| `drawing` | readable JavaScript file path or null | null (built-in visual) | drawing to register before a send when none is registered |

```sh
peek config set '{"position":3,"drawing":"./logo.js"}'
peek send --speak "Hello"       # applies defaults if not registered yet
peek register side            # explicitly applies the configured position
peek register drawing         # explicitly validates and applies the configured drawing
```

`drawing` paths are resolved against the directory where `config set` runs and saved as absolute paths. The file must be nonempty UTF-8 and at most 256 KiB; its JavaScript is validated inside Peek.app when registered. Existing registered positions and drawings take precedence over defaults. Changing or unsetting a default leaves an existing registration in place; use `peek register side` or `peek register drawing` to apply a changed default. With no custom drawing, Peek uses its built-in visual. A configured position held by another Silicon returns `side_taken`; it never displaces its owner.

`peek config home <DIR>` moves this home's store to `<DIR>/.peek` (the session, config, testing environments and daemon token) and leaves a pointer file named `home` in the default store. It prints `{"home","store","pointer","moved":[…],"previous_store"}`; `pointer` is `null` when you point back to `$SILICON_HOME`. If the target already holds a different store, nothing is changed (`invalid_input`).

## Saved account and organization contexts

Use named profiles to retain separate Carbon or Silicon logins. `--org` must equal the selected token's organization; it cannot switch the organization of a saved bearer. Production and each testing world keep separate slots within a profile. Old sessions without an IAM5 context identifier require login again.

```sh
peek --profile work login --token-file -
peek --profile personal login --token-file -
peek --profile work --org tos login status --json
peek --profile sandbox --app-secret-file ./peek-test-secret login si:tester --org tos
```

`peek config home` refuses relocation when named profiles exist so it cannot leave some credentials or retry receipts behind. Profiles store private credentials under `.peek/profiles/<name>`; `default` uses the existing `.peek` directory. Config and permission retry receipts follow the selected profile. An interrupted login must be recovered in its original profile/API/world before another login starts there; use another profile for independent attempts. Refresh cannot change the actor, organization or testing world. Pending commands and native permission actions carry the original context identifier, so a replacement login cannot receive their results.

## peek ting authorize / complete-authorization / enroll

Ting delivery is an explicit feature permission, independent of ordinary login. First review the IAM request, then complete it with the manual approval code. Completing permission does not send queued answers. Enrollment is a separate action that enables deliveries and retries waiting answers for the selected account/org/world.

```sh
peek --profile work ting authorize --json
peek --profile work ting authorization-status --json
peek --profile work ting complete-authorization --code-file - --json
peek --profile work ting enroll --json
```

The start and completion keys are saved before network I/O. Repeat `authorize` or `authorization-status` to recover an uncertain start; repeat `complete-authorization` without a code to replay a saved uncertain completion. The private retry receipt keeps the exact code until completion succeeds, then removes it. A different code cannot replace a pending completion. Keep the same profile, API, org and testing world throughout.

`peek ting cancel-authorization` clears the local review and retained code while preserving drafts and queued work. A decline, expiry or HTTP412 terms change requires a fresh review. Ordinary login remains usable. `peek ting enroll` requires a completed review and uses a stable request-derived key unless `--idempotency-key` is explicitly supplied. Re-enrollment is never automatic.

Peek.app's Permissions pane lists saved account/org/world contexts, opens the IAM review link, accepts the approval code, recovers pending completion, and provides a separate **Enable deliveries and retry queued answers** action.

## peek register side

```sh
peek register side 5 --json
# {"slot":{"index":5,"side":"bottom"},"moved_from":null,"hotkey":"ctrl+cmd+5","warnings":[]}
```

Omit the number to use config `position`; pass a number to override it.

Positions: 1 `top`, 2 `top-right`, 3 `right`, 4 `bottom-right`, 5 `bottom`, 6 `bottom-left`, 7 `left`, 8 `top-left`. One per Silicon: registering another number moves you (`moved_from` names the old one) and your drawing keeps running. A position held by another Silicon fails with `side_taken` (exit 4) and `details:{"owner","free":[…]}`.

`hotkey` is the Carbon's shortcut for the position: `ctrl+cmd+<position>` by default, or whatever modifier the Carbon chose in Peek.app Settings (for example `opt+cmd+5`).

## peek register drawing

Validates, stores and activates your bubble's JavaScript drawing. Omit the file to use config `drawing`. The file is resolved against the current directory and must be at most 256 KiB.

```sh
peek register drawing ./cassette.js
peek register drawing ./cassette.js --check              # validate only; keep the active drawing
peek register drawing ./cassette.js --preview out.png    # also write a PNG grid of test frames
peek register drawing ./cassette.js --dump-frame 30      # add test frame 30's display list
```

Human output:

```text
✓ loaded (3.1 KB)
✓ 90/90 frames ok   p50 0.31ms  p95 0.58ms  max 0.92ms
✓ ops/frame  max 212
✓ glass rebuilt 1 time in 90 frames
drawing active for silicon "dj" at slot 5 (bottom)
```

`--json`:

```json
{"sha256":"…","bytes":3174,"stats":{"frames":90,"p50_ms":0.31,"p95_ms":0.58,"max_ms":0.92,"ops_max":212,"glass_rebuilds":1},
 "warnings":[],"logs":[],"active":true,"slot":5,"server_sync":"pending"}
```

- `active` is `false` with `--check`. `server_sync` is `pending` when a copy is queued for the backend, `skipped` otherwise.
- `slot` is omitted while you hold no position.
- With `--preview`, `preview` names the PNG written. With `--dump-frame N` (test frames are numbered 0–89), `dump` holds that frame's display list ([Drawing the visual](drawing.md#debugging-the-display-list)).

A failing drawing exits 4 with `drawing_invalid` and the previous drawing stays active; `details.error` carries `message`, `stack`, `frame` and `input_summary`. Validation runs inside Peek.app with the same engine that draws on screen, so the CLI starts the app if needed (it can take up to 120 s on a cold start). The full API is in [Drawing the visual](drawing.md).

## peek unregister

Releases your position and shortcut, deletes your drawing locally and on the server, and cancels everything you still had queued: pending asks, the send on screen, the sends waiting behind it and your scheduled sends. No Ting events are sent.

```sh
peek unregister --json
# {"released_slot":3,"cancelled_asks":["ask_0192…"],"cancelled_sends":["snd_0192…"],"cancelled_scheduled":["sch_0192…"]}
peek unregister
# released position 3 (right); drawing deleted; 1 pending ask(s), 1 queued send(s) and 1 scheduled send(s) cancelled
```

`released_slot` is `null` when you held no position. `cancelled_sends` lists the speak and show sends that were on screen or waiting; asks are in `cancelled_asks`. `peek logout` cancels this home's queued and scheduled sends the same way.

## peek send

Speaks, shows or asks on the Carbon's screen. Returns immediately. The send joins your queue on your position and is shown when its turn comes ([Queue, expiry and scheduling](#queue-expiry-and-scheduling)).

| Flag | Meaning |
|---|---|
| `--speak <TEXT>` | 1–2000 characters, streamed with ElevenLabs v4 through Deepgram Voice Agent. |
| `--show <JSON\|@FILE\|->` | 1–3 text or image elements. See [Speak and show](show.md). |
| `--ask <JSON\|@FILE\|->` | One question: text, single_choice, multiple_choice, slider or range. See [Ask a question](ask.md). |
| `--voice <VOICE_ID>` | ElevenLabs voice ID. Overrides config `voice`, then the Carbon's per-language setting, then George (`JBFqnCBsd6RMkjVDRZzb`). Needs `--speak`. |
| `--voice-instructions <TEXT>` | 1–2000 characters of delivery directions, overriding config `voice_instructions`. Needs `--speak`. See [voice customization, accent cues and expression tags](show.md#customize-a-silicons-voice). |
| `--lang <bcp47>` | Supplies a language hint to ElevenLabs. BCP 47 tags are normalized to their primary subtag (`es-MX` supplies `es`). Needs `--speak`. |
| `--duration <SECS>` | 1–120. How long a show stays when there is no speech, or after the speech ends. Without it, a bubble with speech slides back 1.5 s after the speech ends; the text-length default applies only to a show without speech. Needs `--show` or `--speak`; not with `--ask`. |
| `--expires-in <DURATION>` | Drop the send if it has not finished in time: `90s`, `15m`, `2h`, `1d`, `1h30m`, or plain seconds such as `600`. 10 s to 7 days, counted from when the helper receives the send. Any kind; see [Expiry](#expiry). |
| `--expires-at <DATETIME>` | The same deadline as a date-time, 10 s to 7 days ahead: `2026-09-27T18:00`, `"2026-09-27 18:00"`, `18:00`, or with `Z` or `+05:30`. Not with `--expires-in`. |
| `--in <DURATION>` | Schedule the send instead of queueing it now: send it after this long, 1 s to 365 days, in the units of `--expires-in`. One time; there is no recurrence. See [Scheduling](#scheduling). |
| `--at <DATETIME>` | Schedule the send for this time: in the future, at most 365 days ahead. Not with `--in`. |
| `--tz <IANA>` | The time zone for `--at` and `--expires-at` values without an offset, for example `Asia/Kolkata`. Default: the Mac's time zone. |
| `--replace` | Take over your bubble now instead of waiting in line: whatever of yours is current, even an ask, closes without a Ting event. See [Replace](#replace). |
| `--notify speech_finished,show_dismissed,shown` | Opt in to those events for this send (default: config `notify`). `shown` sends `peek.send.shown` when the bubble appears. |
| `--wait[=<SECS>]` | Asks only, and not with `--in` or `--at`. Keep the command open for the answer (default 120, at most 600). Write `--wait=60`, with the `=`; a bare `--wait` means 120. |

Rules, checked before anything is shown or stored:

- At least one of `--speak`, `--show`, `--ask` (`nothing_to_send`, exit 2).
- `--show` and `--ask` are mutually exclusive, and the flags above must match what you send (`conflicting_flags`, exit 2; the combinations are listed below).
- A duration, date-time or time zone that does not parse, lies in the past or is out of range fails with `invalid_input` (exit 2); `details.field` names the flag ([Durations and date-times](#durations-and-date-times)).
- `@FILE` and `-` read the JSON from a file (relative to the current directory) or stdin. Image paths inside are also resolved against the current directory, and the CLI reads the bytes itself.
- Unknown JSON fields and duplicate keys are rejected (`invalid_input`, `invalid_json`), and so are two options with the same label (`invalid_input`). `details.field` names the field. Lengths count Unicode scalar values.
- You need a position (`side_not_registered`, exit 4). Config `position` and `drawing` supply missing registrations before a CLI send; a custom drawing is optional.

The flag combinations added in 0.1.2 and their messages (each also has a hint):

| Combination | Message |
|---|---|
| `--expires-in` with `--expires-at` | `--expires-in and --expires-at cannot be combined; give one deadline` |
| `--in` with `--at` | `--in and --at cannot be combined; a send is scheduled once` |
| `--in` or `--at` with `--expires-in` | `a scheduled send (--in/--at) takes --expires-at, not --expires-in: its clock would start now, not at the due time` |
| `--in` or `--at` with `--wait` | `--wait cannot be used with --in or --at: nobody waits for a scheduled ask` |
| `--tz` without `--at` or `--expires-at` | `--tz applies only to --at and --expires-at` |

`--replace` combines with everything. `--duration` with `--ask` stays refused; bound an ask with `--expires-in` or `--expires-at` instead.

With `--json` it prints:

```json
{"send_id":"snd_0192…","ask_id":"ask_0192…"|null,"slot":3,"status":"showing"|"queued"|"scheduled",
 "queue_position":0|1|2|3|4|5|null,"waiting":2|null,"expires_at":"2026-09-27T12:45:00.000Z"|null,
 "schedule_id":"sch_0192…"|null,"due_at":"2026-09-27T12:30:00.000Z"|null,"tz":"Asia/Kolkata"|null,
 "replaced_send_id":"snd_0192…"|null,
 "speech":{"status":"pending"|"cached"|"skipped"|"unsupported_language","model":"JBFqnCBsd6RMkjVDRZzb","chars":42}|null,
 "warnings":[{"code":"carbon_away","message":"…"}]}
```

- `status` is:
  - `showing`: it became your current bubble and went straight to the Carbon's screen;
  - `queued`: it waits behind your earlier sends (`queue_position` 1–5, where 1 is next), or it is your current bubble but held (`queue_position` 0) because the Carbon is away (`carbon_away`), paused Peek (`carbon_paused`), or Peek.app is not running;
  - `scheduled`: `--in` or `--at` stored it until `due_at`. `schedule_id` names the schedule, and `queue_position` and `waiting` are `null`.
- `waiting` is the number of your sends waiting behind the current one, this send included.
- `expires_at` is the absolute deadline from `--expires-in` or `--expires-at`.
- `tz` is the IANA zone used to read `--at`, or `null` when the value carried its own offset.
- `replaced_send_id` names the send `--replace` took over (`null` when nothing of yours was current).
- `speech` is `null` when there is no `--speak`.
- Every key is always present, `null` when it does not apply, so the shape never changes.

Human output starts with one of these lines, followed by `speech: …`, `ask: …` and `expires: …` lines when they apply:

```text
sent snd_0192… to position 3 (showing)
sent snd_0192… to position 3 (showing; replaced snd_0192…)
sent snd_0192… to position 3 (queued: 2 ahead of it)
sent snd_0192… to position 3 (queued: shown when the Carbon is back)
sent snd_0192… to position 3 (queued: Peek is paused; shown when the Carbon resumes)
sent snd_0192… to position 3 (queued: shown when Peek.app starts)
scheduled snd_0192… for 2026-09-27 18:00 IST (Asia/Kolkata) (in 5h 48m); schedule sch_0192…
```

| Warning | Meaning |
|---|---|
| `carbon_away` | The Carbon's screen is locked or the display is asleep. Nothing is shown to nobody: the send waits in your queue and is shown, in order, when the Carbon is back. Expiry clocks keep running while it waits, and speech starts only when the bubble is shown. |
| `carbon_paused` | The Carbon paused all peeks in Peek.app. The send waits in your queue and is shown when they resume. |
| `ting_not_enrolled` | This Silicon is not a Ting recipient for peek, so answers cannot be delivered; they wait on the Mac. Run `peek ting enroll`. |
| `isi_ignored` | `$ISI` was invalid and was dropped from this send (no `metadata.isi` in its events). |
| `speak_language_unsupported` | Compatibility warning from older helpers using the former language list; update Peek.app. Current speech accepts multilingual voices and language hints. |
| `timezone_fallback_utc` | The Mac's time zone could not be read, so an `--at` or `--expires-at` without an offset was read as UTC. Add an offset or `--tz`. |

With `--wait`, the command prints the final answer instead. Whatever it prints replaces the Ting event for that ask, so each outcome reaches you through exactly one channel:

```jsonc
// answered in time (no Ting event is sent for this answer)
{"ask_id":"ask_…","send_id":"snd_…","state":"answered","answer":{"kind":"single_choice","option_id":"keep","label":"Keep"},"via":"click","transcript":null,"answered_at":"…Z"}
// dismissed, expired or cancelled while waiting (no peek.ask.dismissed or peek.ask.expired is sent either)
{"ask_id":"ask_…","send_id":"snd_…","state":"dismissed"}
// taken over by a later --replace send of yours (no Ting event either)
{"ask_id":"ask_…","send_id":"snd_…","state":"replaced"}
// timeout: exit 0, and the answer is delivered by Ting later
{"ask_id":"ask_…","send_id":"snd_…","state":"pending","delivery":"ting"}
```

In human mode a replaced ask prints `ask_…: replaced by a newer send from you (no answer)`.

## Queue, expiry and scheduling

### The queue

Each Silicon has one queue on its position, shared by all its ISIs. It holds the send that owns your bubble (the **current** one, normally on screen) and at most **5 waiting** behind it, strictly first in, first out.

- **A new send always waits its turn.** It never replaces what is on screen unless you pass `--replace`. It starts when the current one is done: a show's time ran out, its speech ended 1.5 s ago, the ask was answered, dismissed or expired, or the Carbon closed it.
- **One on screen plus five waiting.** A send that would be the sixth waiting fails with `queue_full` (exit 4, retryable), and nothing is stored:
  ```json
  {"error":{"code":"queue_full","message":"position 3's queue is full: 1 send on screen and 5 waiting (at most 5); remove one with `peek cancel <send_id>` or `peek queue clear`",
   "hint":"peek queue    lists the waiting sends and their IDs","retryable":true,"request_id":null,
   "details":{"queued":5,"limit":5,"on_screen":"snd_0192…","waiting":["snd_0192…","snd_0192…","snd_0192…","snd_0192…","snd_0192…"],"due_waiting":0,"held":null}}}
  ```
  The limit is the same while nothing moves. Then `details.held` says why: `carbon_away` (the Carbon's screen is locked or asleep; the message adds that nothing moves until they are back), `paused` (the Carbon paused Peek) or `app_not_running`. `due_waiting` counts scheduled sends that came due while the queue was full ([Scheduling](#scheduling)).
- **The Carbon sees the line.** A small `+N` badge next to your bubble's down-arrow counts the sends waiting behind it, and updates as the queue changes.
- **You can prune it.** `peek queue` lists the queue, `peek cancel <SEND_ID>` withdraws one send, and `peek queue clear` drops every waiting send (`--all` also withdraws the current one). None of them sends a Ting event.
- **Silicons never wait for each other.** Your queue is yours; another Silicon's bubbles never delay yours.

A CLI older than 0.1.2 gets the same error from a 0.1.2 helper under its old name, `slot_busy`.

### Replace

`--replace` takes over your bubble at once instead of waiting in line:

- Whatever of yours is current, on screen or held, closes as `replaced`, whatever its kind, and its speech stops. **An ask is cancelled without an answer and without a Ting event.** A `peek send --wait` waiting for it prints `"state":"replaced"`, and `peek ask get` shows the state `replaced`.
- The new send becomes current (`replaced_send_id` names the old one). Your waiting sends keep their order behind it.
- It works when the queue is full, because it adds nothing to the queue.
- No Ting event is sent for a send you replaced yourself.

Use it for corrections ("sorry, the build failed after all"), not by default: a Carbon who was reading loses the bubble.

### Expiry

`--expires-in` and `--expires-at` work on every kind of send (before 0.1.2, asks only).

| When the deadline passes | What happens | You receive |
|---|---|---|
| while the send waits in the queue, or is held while the Carbon is away | it is dropped and never shown | `peek.send.expired` (speak or show) or `peek.ask.expired` (ask), with `"shown":false` |
| while it is on screen | it slides away and its speech stops | the same event, with `"shown":true` |

The send's history entry closes as `expired`. The clock keeps running while the Carbon is away, while Peek is paused, and while a scheduled send waits for room. A send without a deadline waits as long as it takes. As before, a live `--wait` prints `"state":"expired"` instead of the Ting event.

### Durations and date-times

**Durations** (`--expires-in`, `--in`) are plain seconds (`600`, as in 0.1.1) or the units `d`, `h`, `m`, `s` in that order, each at most once, lowercase and without spaces: `90s`, `15m`, `2h`, `1d`, `1h30m`, `2d12h`. Anything else, such as `1.5h`, `5min`, `1w`, `90S` or `1m1h`, is `invalid_input`.

**Date-times** (`--at`, `--expires-at`):

| Form | Example | Read as |
|---|---|---|
| date and time | `2026-09-27T18:00`, `"2026-09-27 18:00"`, `2026-09-27T18:00:30.5` | in the time zone below |
| time only | `18:00` | today, in the time zone below |
| with an offset | `2026-09-27T18:00Z`, `2026-09-27T18:00+05:30`, `2026-09-27T18:00-0700`, `2026-09-27T18:00+05` | exactly that instant; `--tz` does not apply |

- Times are 24-hour `HH:MM`, with optional seconds and fraction. A date alone (`2026-09-27`), words (`tomorrow`, `6pm`), Unix timestamps and bracketed zones (`2026-09-27T18:00[Asia/Kolkata]`) are refused with examples of what works.
- A value without an offset is read in `--tz` when given, otherwise in the **Mac's time zone**, the one its menu-bar clock shows. peek reads the system setting and never `$TZ`. If the setting cannot be read, the value is read as UTC and the result carries the `timezone_fallback_utc` warning.
- A local time that does not exist, because the clocks skip it (`2026-03-08T02:30` in `America/New_York`), is refused. A local time that happens twice, when the clocks go back, is the earlier of the two.
- A time in the past is refused, and the message names both instants: `` `--at 09:00` is in the past: 2026-09-27 09:00 IST (now 2026-09-27 11:42 IST) ``. For a time-only value the hint suggests tomorrow's date: `for tomorrow use --at 2026-09-28T09:00`.

### Scheduling

`--in <DURATION>` or `--at <DATETIME>` stores the send and puts it in your queue when it comes due. It is sent once; there is no recurrence.

```sh
peek send --speak "Stand-up in five minutes." --at 09:55
peek send --show @digest.json --in 2h --expires-at 2026-09-27T20:00
peek send --ask @standup.json --at "2026-09-28 09:00" --tz Europe/Berlin
peek send --speak "Time to leave for the airport." --at 17:30 --replace
```

- **Checked now, sent later.** `peek send` validates everything at once: the content, your position, any configured drawing, and the voice and delivery instructions. Image bytes are copied when you schedule, so your files may change or disappear afterwards. The result has `status:"scheduled"`, `schedule_id`, `due_at`, and the ids the send keeps when it fires (`send_id`, and `ask_id` for an ask).
- **Limits.** `--in` is 1 s to 365 days; `--at` must be in the future and at most 365 days ahead. A Silicon can have 500 scheduled sends; one more fails with `schedule_full` (exit 4).
- **Expiry.** A scheduled send takes `--expires-at` only, between 10 s and 7 days after its due time. `--expires-in` is refused, because its clock would start now.
- **No `--wait`.** Nobody waits for a scheduled ask; its answer arrives as `peek.ask.answered`. Until it fires, a scheduled ask is listed by `peek schedule list`, not by `peek ask list`.
- **`--replace` works,** and applies at the due time: the send then takes over whatever of yours is current.
- **When it comes due**, the send enters your queue on the position you hold then, like a new send, and you receive `peek.schedule.due` with its `outcome`:
  - `shown`: it went straight to the screen;
  - `queued`: it waits behind your other sends, or it is held because the Carbon is away or paused Peek (`waiting_reason` says which);
  - `expired`: its `--expires-at` had already passed; you also receive the usual expiry event with `"shown":false`;
  - `replaced`: it was a `--replace` send and took over the send in `replaced_send_id`.

  When it actually appears, you also receive `peek.send.shown`.
- **Never lost to a full queue.** If five sends already wait when it comes due, it takes the next free spot, before any new regular send (those get `queue_full` meanwhile). Its expiry clock keeps running.
- **Sleep and downtime.** If the Mac is asleep, or Peek is not running at the due time, the send fires as soon as the Mac or Peek.app is back (usually at once, at most about 15 s after waking), oldest first. If its `--expires-at` passed meanwhile, you receive the expired events instead and nothing is shown.
- **Manage them** with `peek schedule list`, `peek schedule cancel <ID>` and `peek schedule clear`; `peek cancel` accepts a `sch_…` id too. Once fired, a scheduled send is an ordinary queued send: withdraw it with `peek cancel <SEND_ID>`. `peek unregister` and `peek logout` cancel scheduled sends. They are stored on this Mac only.

### When Peek.app is older

The helper tells the CLI what it supports. If Peek.app on the Mac is older than 0.1.2 (Honeycomb already updated your CLI, and the app has not updated yet), the new options fail with `app_update_pending` (exit 5, retryable) instead of being silently ignored. That covers `--expires-in` or `--expires-at` on speak and show sends, `--expires-at` on anything, `--in`, `--at`, `--replace`, `--notify shown`, `peek queue`, `peek cancel` and `peek schedule`:

```json
{"error":{"code":"app_update_pending","message":"Peek.app on this Mac runs peekd 0.1.1, which does not support scheduled sends (--in/--at) yet (it needs Peek 0.1.2 or newer); Peek.app updates itself when nothing is on screen",
 "hint":"run `peek app update` to apply the bundled build now, then retry","retryable":true,"request_id":null,
 "details":{"missing_features":["schedule"],"peekd_version":"0.1.1"}}}
```

Sends without the new options keep working meanwhile, with the old helper's behaviour. A `shown` in config `notify` is left out with a hint on stderr instead of failing the send.

## peek queue

The send on screen and the ones waiting behind it, on your position.

```sh
peek queue                  # the same as: peek queue list
peek queue --json
```

```text
position 3 · 1 on screen · 2 waiting (limit 5) · 4 scheduled
  on screen  snd_0192…  ask         12s  "Delete old.zip?"     expires in 9m 48s
  1          snd_0192…  show         8s  "Build finished"
  2          snd_0192…  speak        3s  "Deploy done"         expires in 50s
```

- The first row is the current send, labelled `on screen`, or `held` when it waits for the Carbon. The first line then says why: `held: the Carbon's screen is locked or asleep`, `held: Peek is paused` or `held: Peek.app is not running`.
- The other rows are the waiting sends, in the order they will be shown. `due` marks a scheduled send that came due while the queue was full.
- Columns: place in line, send id (printed in full; `…` here only shortens this page), kind, age since `peek send`, a summary of at most 60 characters, and the deadline if there is one.
- With nothing queued: `position 3: nothing on screen or waiting · 4 scheduled (peek schedule list)`. Without a position: `no position registered (peek register side <1-8>)`.

```json
{"slot":3,"limit":5,"held":null,"scheduled":4,"scheduled_limit":500,
 "on_screen":{"send_id":"snd_0192…","ask_id":"ask_0192…","kind":"ask","summary":"Delete old.zip?","state":"on_screen",
   "queue_position":0,"created_at":"2026-09-27T10:00:00.000Z","queued_at":"2026-09-27T10:00:00.000Z","age_ms":12040,
   "expires_at":"2026-09-27T10:10:00.000Z","shown_at":"2026-09-27T10:00:00.300Z","schedule_id":null,"due_at":null},
 "waiting":[{"send_id":"snd_0192…","ask_id":null,"kind":"show","summary":"Build finished","state":"waiting",
   "queue_position":1,"created_at":"2026-09-27T10:00:04.000Z","queued_at":"2026-09-27T10:00:04.000Z","age_ms":8010,
   "expires_at":null,"shown_at":null,"schedule_id":null,"due_at":null}]}
```

- `kind` is `speak`, `show`, `ask`, `speak+show` or `speak+ask`. `state` is `on_screen`, `held`, `waiting` or `due_waiting`.
- `queue_position` 0 is the current send; 1, 2, … is the order of the others.
- `summary` is the question, the first text element (else the first image caption, else `image`), or the speech text.
- `created_at` is when `peek send` ran; `queued_at` is when the send entered the queue (for a scheduled send, when it fired). `shown_at` is set once the bubble appeared.
- `held` is `carbon_away`, `paused`, `app_not_running` or `null`. Without a position, `slot` and `on_screen` are `null` and `waiting` is empty.

### peek queue clear

```sh
peek queue clear --json      # {"cancelled":["snd_0192…","snd_0192…"],"on_screen":"snd_0192…","on_screen_cancelled":false}
peek queue clear --all       # cleared 2 waiting sends and the one on screen (snd_0192…)
```

Drops every waiting send, in queue order, including scheduled sends that came due and wait for room. Waiting asks are cancelled (state `cancelled`); other sends close as `cleared`. `--all` also withdraws the current send, which then slides away. Human output: `cleared 2 waiting sends; the one on screen (snd_0192…) stays`, `cleared 2 waiting sends and the one on screen (snd_0192…)`, `cleared the one on screen (snd_0192…); nothing was waiting` with `--all` and an empty queue, `cleared 2 waiting sends` when nothing is on screen, or `nothing was waiting`. No Ting events; exit 0 also when there was nothing to clear.

## peek cancel

Withdraws one send wherever it is: on screen, held, waiting or scheduled, of any kind, asks included. It takes a send id (`snd_…`), an ask id (`ask_…`) or a schedule id (`sch_…`). No Ting event is sent.

```sh
peek cancel snd_0192… --json
# {"send_id":"snd_0192…","ask_id":null,"schedule_id":null,"was":"waiting","queue_position":2,"state":"cancelled"}
```

| `was` | Human output |
|---|---|
| `on_screen`, `held` | `snd_0192… cancelled; the bubble slides away and no Ting event is sent` |
| `waiting`, `due_waiting` | `snd_0192… cancelled; it was #2 in line and will not be shown` |
| `scheduled` | `snd_0192… cancelled; it was scheduled for 2026-09-27 18:00 IST (Asia/Kolkata) and will not be sent` |
| `closed` | `snd_0192… had already closed (answered); nothing to cancel` |

- A cancelled ask gets the state `cancelled`, exactly as with `peek ask cancel`, and a `--wait` on it prints `cancelled`. Other sends close as `cancelled`. Speech stops.
- `closed` (exit 0) means the send had already left: `state` then holds its final ask state or close reason (`answered`, `dismissed`, `expired`, `replaced`, `auto`, `speech_done`, …).
- `queue_position` says where it was (`null` for `scheduled` and `closed`).
- An id that is not a send, ask or schedule id is `invalid_input` (exit 2). A well-formed id that does not exist or is not yours is `send_not_found` (exit 4, hint `peek queue`), or `schedule_not_found` for a `sch_…` id (hint `peek schedule list`).

## peek schedule

Your one-time scheduled sends (`peek send --in` or `--at`) that are not due yet.

```sh
peek schedule list
peek schedule cancel sch_0192…
peek schedule clear
```

`peek schedule list` shows them soonest first, in the time zone each was scheduled in (`--tz`, else the Mac's). ` · replace` marks a `--replace` send.

```text
2 scheduled for position 3 (limit 500)
  sch_0192…  snd_0192…  ask   2026-09-27 18:00 IST (in 2h 13m)  "Stand-up in 5?"  expires 18:30 · replace
  sch_0192…  snd_0192…  show  2026-09-28 09:00 IST (in 17h)     "Morning"
```

```json
{"scheduled":[{"schedule_id":"sch_0192…","send_id":"snd_0192…","ask_id":"ask_0192…","kind":"ask","summary":"Stand-up in 5?",
  "due_at":"2026-09-27T12:30:00.000Z","tz":"Asia/Kolkata","expires_at":"2026-09-27T13:00:00.000Z","replace":true,
  "created_at":"2026-09-27T10:17:00.000Z"}],"limit":500}
```

With nothing scheduled it prints `nothing scheduled`.

`peek schedule cancel <ID>` takes the `sch_…` id or the send's `snd_…` id:

```sh
peek schedule cancel sch_0192… --json     # {"schedule_id":"sch_0192…","send_id":"snd_0192…","state":"cancelled"}
```

The human line is `sch_0192… (snd_0192…) cancelled; it was due 2026-09-27 18:00 IST (Asia/Kolkata) and will not be sent`. If it already fired, `state` is `fired`, it exits 0, and it prints `sch_0192… already fired as snd_0192…; withdraw it with peek cancel snd_0192…`. An unknown id is `schedule_not_found` (exit 4).

`peek schedule clear` cancels all of them: `{"cancelled":["sch_0192…","sch_0192…"]}`, in human mode `cancelled 2 scheduled sends` or `nothing was scheduled`.

## peek ask

Local state of your asks. Works even when Ting delivery is not set up.

```sh
peek ask get ask_0192… --json
# {"ask_id":"ask_0192…","send_id":"snd_0192…","question":"Keep the file?","ask_type":"text","state":"answered",
#  "answer":{"kind":"text","text":"yes"},"via":"voice","transcript":"yes","answered_at":"…Z",
#  "created_at":"…Z","expires_at":null,"delivery":{"status":"accepted","ting_id":"msg_…","silent":false}}
peek ask list --state pending --limit 20 --json      # {"asks":[…]}, newest first, same item shape
peek ask cancel ask_0192… --json                     # {"ask_id":"ask_0192…","state":"cancelled"}
```

- States: `pending`, `answered`, `dismissed`, `expired`, `cancelled`, and `replaced` (a later `peek send --replace` of yours took it over; no Ting event). A CLI older than 0.1.2 sees `replaced` as `cancelled`.
- `delivery.status`: `pending`, `authority_required`, `accepted`, `expired`, `cancelled`, `failed`, or `wait` (delivered to a live `send --wait` instead of Ting).
- `send_id`, `question`, `ask_type`, `transcript`, `created_at` and `expires_at` are present when known.
- `ask cancel` slides the question away and sends no Ting event; a waiting ask is simply dropped. On an ask that is already closed it returns that ask's current state (in human mode, for example, `ask_0192… was already replaced by a newer send; nothing to cancel`). An unknown id fails with `ask_not_found` (exit 4). `peek cancel` does the same for any kind of send.
- A scheduled ask is not listed until it fires; see `peek schedule list`.
- `ask list --limit` is 1–200 (default 20).

## peek history

Your recent sends on this Mac, newest first. History never leaves the Mac.

```sh
peek history --limit 50 --json
# {"items":[{"send_id":"snd_…","kind":"ask","created_at":"…Z","shown_at":"…Z","closed_at":"…Z","close_reason":"answered","ask_id":"ask_…","ask_state":"answered"},
#            {"send_id":"snd_…","kind":"show","created_at":"…Z","closed_at":"…Z","close_reason":"expired","expires_at":"…Z",
#             "schedule_id":"sch_…","due_at":"…Z"},
#            {"send_id":"snd_…","kind":"speak","created_at":"…Z","shown_at":"…Z","closed_at":"…Z","close_reason":"auto",
#             "warnings":[{"code":"speech_failed","message":"speech failed before any audio played: …"}]}]}
```

- `--limit` is 1–200 (default 50); page with `--before <SEND_ID>`.
- `shown_at` is when the bubble appeared on screen (absent if it never did), `expires_at` the send's deadline, and `schedule_id` and `due_at` are set for a send that came from `--in` or `--at`. Each is left out when it does not apply.
- `close_reason` says how the send ended (it is absent while the send is still queued or showing):

| `close_reason` | Meaning |
|---|---|
| `speech_done` | the speech ended and the bubble slid back 1.5 s later |
| `auto` | a show without speech (or its `--duration`) ran out |
| `answered` | the Carbon answered the ask |
| `dismissed` | the Carbon closed it (Esc or the down-arrow) |
| `expired` | it reached its `--expires-in` or `--expires-at`, on screen or before it was ever shown |
| `cancelled` | you withdrew it (`peek cancel`, `peek ask cancel`, `peek unregister`, `peek logout`) |
| `cleared` | `peek queue clear` dropped it before it was shown (or with `--all`, while it was on screen) |
| `replaced` | a later `peek send --replace` of yours took over the bubble |
| `ui_disconnected`, `daemon_restarted` | Peek.app quit, or the helper restarted, while it was on screen |

- An item carries a `warnings` array (the same `{"code","message"}` shape as everywhere else) when something went wrong after `peek send` had already returned. Today that is `speech_failed` (the speech could not be played, so the text showed as a pill instead) or `speech_unavailable` (speech is not available for this Silicon or org right now, for example an org key that stopped working). The field is left out when there is nothing to report. In human mode the codes follow the row in brackets, such as `[speech_failed]`.

## peek status

One view of everything that matters for this Silicon on this Mac: position, drawing, queue, pending asks, deliveries, whether the Carbon is there, Peek.app and the helper.

```sh
peek status --json
# {"actor_id":"si:dj","context":"production",
#  "slot":{"index":3,"side":"right"},"drawing":{"sha256":"…","bytes":3174,"active":true,"last_error":null},
#  "queue":{"pending":2,"on_screen":"snd_0192…","waiting":2,"limit":5,"scheduled":4,"held":null},
#  "pending_asks":1,"deliveries":{"pending":0,"authority_required":0,"last_error":null},
#  "carbon":{"available":true,"reason":"ok","paused":false},
#  "ui_running":true,"daemon":{"running":true,"version":"0.1.2","protocol":1},"app":{"build":1002,"ui_running":true}}
```

- `slot` and `drawing` are `null` until you register them. With no custom drawing, `drawing` is `null` and the built-in visual shows. `drawing.active` is `false` after a custom drawing fails. `drawing.server_sync` is the state of the drawing's backend copy (`synced`, `pending` or `authority_required`); `deliveries` counts Ting events only.
- `queue.on_screen` is your current send (on screen or held), `queue.waiting` the sends waiting behind it (`queue.limit` is 5, plus any scheduled sends that came due while the queue was full; `pending` is the same number under its 0.1.1 name), `queue.scheduled` your scheduled sends that are not due yet, and `queue.held` why nothing moves (`carbon_away`, `paused`, `app_not_running`) or `null`. When anything waits, a hint points to `peek queue`.
- `carbon.available` is `false` while the Carbon's screen is locked (`reason:"locked"`), the display is asleep (`"asleep"`) or off (`"display_off"`). Sends then wait in `queue` and are shown when it turns `true` again. An older Peek.app that never reports presence counts as available. `carbon.paused` is `true` while the Carbon paused all peeks in Peek.app.
- `deliveries.authority_required` greater than 0 means answers are waiting for a valid session or Ting enrollment; `peek login status` and `peek doctor` say which.
- `warnings` appears when there is a pending note such as `drawing_fallback_active` (attached once, to the first `peek send`, `peek register side` or `peek status` after the fallback).

## peek org byo deepgram

These commands manage previously saved Deepgram keys for compatibility. **Current speech uses ElevenLabs TTS through Peek's server-owned Deepgram account and OpenAI transcription; these org keys and endpoints do not affect either provider.** Operators configure the server-side Deepgram key for ElevenLabs TTS and the OpenAI transcription key instead.

```sh
peek --org tos org byo deepgram show --json
peek --org tos org byo deepgram delete
```

Stored keys remain encrypted and are never returned. `set --key-file <PATH|-> [--base-url URL]` remains available for compatibility, validates the Deepgram key and its allowed HTTPS origin, and does not activate Deepgram speech. There is no reason to set a new Deepgram key for current Peek. All commands require an org owner/admin for writes; other members get `not_org_admin` (exit 4). They only call the backend, so inspection and deletion work on Linux and Windows too.

## peek app

Manage Peek.app on this Mac. On a Mac, `honeycomb install 'peek'` already installs and starts Peek.app through the package's install script; `peek app install` does the same by hand, and `peek login` and every command that needs the app fall back to it when the app is missing.

| Command | Does | `--json` |
|---|---|---|
| `peek app status` | Whether Peek.app is installed, its build and signature, whether it and the helper run, the background item, the offered builds and the install log. | `{"supported":true,"platform","installed","path","build","short_version","bundle_id","signature":"valid"\|"invalid"\|"unchecked"\|"unreadable"\|"absent","running":{"ui","daemon"},"daemon":{…}\|null,"agent":"loaded"\|"not_loaded"\|"unknown","best_offer","bundled","last_install_script":[…]}`. On Linux and Windows: `{"supported":false,"platform":"linux-x86_64"}`, exit 0. |
| `peek app install` | Installs `~/Applications/Peek.app` from this CLI's package if it is missing, verifies its signature, removes quarantine and starts it. It never modifies an existing app. | `{"installed":true,"path","installed_now","offered_build","build","short_version","running":{"daemon":true,"ui"},"peekd_version"}` |
| `peek app update` | Offers the newest build this CLI knows to the helper, which installs it once nothing is on screen. Needs a logged-in home. | `{"scheduled","installed_build","offered_build"}` (plus `reason` when nothing is offered) |
| `peek app uninstall` | Unregisters the login item and helper and moves Peek.app to the Trash, then waits up to 15 s for it to go. `honeycomb uninstall 'peek'` does not do this. | `{"uninstalled":true,"was_installed","path","via":"peekd"\|"open"\|null}` |

## peek daemon

```sh
peek daemon status --json
# {"running":true,"pid":812,"version":"0.1.2","protocol":1,"socket":"/var/tmp/silicon-peek-501/peekd.sock","ui":{"running":true,"build":1002},"homes":3}
peek daemon restart --json
# the same shape, plus "restarted":true and "via" (how it was restarted)
```

`daemon status` needs no login and never starts anything. When the helper is not running it prints `{"running":false,"socket":"…","next":"peek app install"}` and exits 0. `restart` asks launchd to restart the helper, or starts Peek.app when the agent is not loaded.

## peek docs

The pages on this site, bundled into the binary so they work offline.

```sh
peek docs                    # the topic list (JSON: {"topics":[…],"online","repository","help","index_content",…})
peek docs ask                # one topic as Markdown
peek docs --search keyterm   # line matches across all topics (JSON: {"query","results":[{…,"matches":[{"line","excerpt","truncated"}]}]})
peek docs --all              # everything (JSON: {"documents":[…]})
peek docs ask --json         # {"topic","title","path","format":"markdown","command":"peek docs ask","content","package_version","embedded":true}
```

Topics: `start`, `carbon`, `silicon`, `cli`, `show`, `ask`, `drawing`, `ting`, `iam`, `testing`, `telemetry`, `privacy`, `platforms`, `development`, `versioning`, `troubleshooting`. An unknown topic fails with `invalid_input` and `details.topics`. Search excerpts are at most 320 characters.

## peek commands

```sh
peek commands --json
# [{"command":"peek send","description":"…","usage":"…","help":"…","group":false,
#   "arguments":[{"name","long","short","positional","required","global","description","possible_values"}]}]
```

`group` is `true` for a command that only groups subcommands (such as `peek register`). The list is generated from the same definitions as `--help`, so it is always complete.

## peek report

Files a bug. If you also fixed it, pass your pull request.

```sh
peek report "send --ask fails with invalid_input when an option label has an emoji" \
  --pr https://github.com/teamofsilicons/silicon-peek/pull/42 --attach-status
# {"submitted":true,"id":"rep_…","status":"filed"|"stored","issue_url":"https://github.com/teamofsilicons/silicon-peek/issues/…"|null,"via":"backend"}
```

- The backend stores the report and files a GitHub issue. `status:"stored"` means it was saved but not (yet) filed.
- `--pr` must be `https://github.com/teamofsilicons/silicon-peek/pull/<n>`. Without it, a hint on stderr invites you to send a fix.
- `--attach-status` adds the non-secret `peek doctor` output.
- `--via gh` files it with your own `gh` instead (`gh issue create --repo teamofsilicons/silicon-peek`), and prints `"id":null,"status":"filed","via":"gh"`. A missing `gh` fails with `not_found` (exit 4); a failing `gh` with `gh_failed` (exit 5).
- `--dry-run` prints the report exactly as it would be filed and sends nothing. Use it to check what you would send; reports become public GitHub issues.
- Refused under `--test` with `conflicting_flags` (exit 2): reports go to the production tracker.

peek is open source. The best report says exactly how to reproduce the bug, and links the fix. See [Development](development.md).

## peek update

peek never replaces itself. Honeycomb updates the CLI every minute, and the helper updates Peek.app. `peek update` tells you what is installed and how to update by hand:

```json
{"manager":"honeycomb","app_id":"peek","current_version":"0.1.2","auto_update":true,"can_replace_running_binary":false,
 "command":"honeycomb update 'peek'","app":{"build":1002,"best_offer":1002},"message":"Honeycomb manages the peek CLI: …"}
```

`auto_update` is `false` when `HONEYCOMB_AUTO_UPDATE` is off. `app` is `null` on Linux and Windows.

## peek doctor

Runs every check and prints the exact fix for anything that is wrong. It always exits 0; a check that cannot run is reported as a warning.

```sh
peek doctor --json
# {"checks":[{"name":"…","status":"ok"|"warn"|"fail","detail":"…","fix":"…"|null}, …],
#  "summary":{"ok":13,"warn":1,"fail":0},"version":"0.1.2","platform":"macos-aarch64"}
```

Checks, by `name`: `store` (permissions), `session`, `pending_login` (a login whose response was lost), `revocations` (logout revocations still being retried) and `ting` (enrollment), `backend` (`/readyz`), `honeycomb` (0.5.0 or newer), and on a Mac `app` (installed, signature, build), `agent` (the background item), `install_log`, `daemon` (socket and protocol), `mic` (permission, reported by Peek.app), `hotkeys` and `outbox` (the delivery backlog). Every check is always listed; one with nothing to report is `ok`. See [Troubleshooting](troubleshooting.md).
