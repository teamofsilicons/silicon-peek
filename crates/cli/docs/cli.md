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
│   ├── side <1-8>                                 claim or move to a position (1 top, clockwise)
│   └── drawing <FILE.js> [--check] [--preview <OUT.png>] [--dump-frame <N>]
├── unregister                                     release the position and delete the drawing (local + server)
├── send [--speak <TEXT>] [--show <JSON|@FILE|->] [--ask <JSON|@FILE|->]
│        [--voice <aura-2-name-lang>] [--lang <bcp47>] [--duration <SECS>] [--expires-in <SECS>]
│        [--notify speech_finished,show_dismissed] [--wait[=<SECS>]]
├── ask
│   ├── get <ASK_ID>                               local state and answer (works while Ting is not set up)
│   ├── list [--state pending|answered|dismissed|expired|cancelled] [--limit N]
│   └── cancel <ASK_ID>                            slide the ask away; no ting
├── history [--limit N] [--before <ID>]            local send history for this Silicon
├── status                                         position, drawing, queue, deliveries, app, daemon (one view)
├── org
│   └── byo deepgram (set --key-file <PATH|-> [--base-url URL] | show | delete)   org admins only
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
| 4 | refused, or a precondition is missing | `side_not_registered`, `drawing_not_registered`, `side_taken`, `drawing_invalid`, `slot_busy`, `ask_not_found`, `platform_unsupported`, `cli_outdated`, `store_schema_newer`, `private_application_organization_required`, `recipient_not_registered`, `not_org_admin`, `unknown_op`, `daemon_running`, `daemon_identity_mismatch`, `drawing_not_found`, `environment_not_prepared`, `idempotency_response_expired`, `not_found`, `lifecycle_conflict`, `origin_not_allowed`, `telemetry_table_unavailable` |
| 5 | unavailable or transport | `backend_unavailable`, `iam_unavailable`, `iam_misconfigured`, `daemon_unavailable`, `peek_service_unavailable`, `no_gui_session`, `app_update_pending`, `speech_unavailable`, `ting_unavailable`, `ting_type_missing`, `idempotency_in_progress`, `rate_limited`, `protocol_error` |

`telemetry_rejected`, `origin_not_allowed` and `telemetry_table_unavailable` come from the backend's telemetry gateway, and `lifecycle_conflict` from its testing-environment lifecycle endpoints; no `peek` command normally shows them. A code this build does not know (a newer backend or helper may add some) is passed through unchanged, and its exit code follows the HTTP status: 401 → 3; 403, 404, 409 → 4; 400, 413, 422 → 2; 408, 429, 5xx → 5; anything else → 1. `peek report --via gh` uses `gh_failed` when `gh` itself fails (exit 5, or 1 when the report could not even be handed to `gh`).

`retryable:true` in an error means the same command can succeed later without changes. Exit 5 is usually worth retrying with backoff (`no_gui_session` and `protocol_error` are not); exits 2 to 4 need a change first.

## peek iam

Static discovery. No network, no session, no side effects, exit 0. Stemcell calls it for every candidate binary.

```sh
peek iam --json
```

```json
{"app_id":"peek","org_id":"tos","name":"Peek","version":"0.1.1","api_version":"v1",
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
- On success it enrolls you as a Ting recipient, attaches this home to a running helper, and (on a Mac) installs and starts Peek.app in the background after printing its result. Its output and exit code never depend on that last step.
- Output: the authenticated `login status` shape below. If Ting enrollment did not complete, `ting` is `{"subscribed":false,…,"error":{"code","message"}}`, `next` is `"peek ting enroll"`, and a hint says so on stderr.
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
 "scopes":["obo:ting:subscriptions.register","obo:ting:subscriptions.revoke","obo:ting:tings.send","self.identity.read","self.membership.read","self.profile.read"],
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

Revokes this home's peek session, then cancels this home's undelivered answers on this Mac. The Silicon's Ting recipient grant is **kept**, because every home logged in as the same `si:` shares it; logging out one home never stops another home's answers.

```sh
peek logout                  # this home only; the Ting grant stays
peek logout --revoke-ting    # also remove this Silicon's Ting grant for peek
```

Prints `{"authenticated":false,"remote_revocation":"confirmed"|"pending"}` and exits **0** in both cases. After `--revoke-ting`, every home of this Silicon shows `"ting":{"subscribed":false}` in `peek login status`, its sends carry a `ting_not_enrolled` warning, and `peek ting enroll` (or a new login) restores delivery. `pending` means the local session is gone but the backend could not be reached; the revocation is retried by the next `peek` run. A home without a session also answers `confirmed`. `peek logout --help` exits 0 (Stemcell probes it).

## peek config

Configuration for this home, stored in `$SILICON_HOME/.peek/config.json`.

```sh
peek config set '{"notify":["speech_finished"],"voice":"aura-2-thalia-en"}'
peek config show --json      # {"schema":1,"telemetry":true,"voice":"aura-2-thalia-en","language":null,"notify":["speech_finished"],"api_url":null,"delivery_max_age_hours":168}
peek config get voice --json # {"key":"voice","value":"aura-2-thalia-en"}
peek config unset voice
peek config telemetry off
```

`set` parses strictly: a non-object, duplicate keys or an unknown key fail with exit 2 and `details.valid_keys`, and nothing is written. The object is merged atomically; `null` resets a key to its default. The resulting config is printed (the same shape as `show`) and pushed to a running helper. The helper gets the home's effective `telemetry`, which is `false` also when only the environment opts out. `unset` and `telemetry` are shortcuts for `set` and print the same. Stemcell runs `peek config set` after every login when `app_configs.peek` exists.

| Key | Type / validation | Default | Effect |
|---|---|---|---|
| `telemetry` | bool | `true` | this home's telemetry |
| `voice` | `^aura-2-[a-z]+-(en\|es\|de\|fr\|nl\|it\|ja)$` or null | null (per-language default) | default TTS voice |
| `language` | BCP 47 primary subtag (2–3 letters) or null | null (detect) | TTS language when detection is ambiguous |
| `notify` | array ⊆ `["speech_finished","show_dismissed"]` | `[]` | default `--notify` |
| `api_url` | https URL or null | null | same as `--api` |
| `delivery_max_age_hours` | int 1..168 | 168 | how long this Silicon's undelivered events keep retrying |

`peek config home <DIR>` moves this home's store to `<DIR>/.peek` (the session, config, testing environments and daemon token) and leaves a pointer file named `home` in the default store. It prints `{"home","store","pointer","moved":[…],"previous_store"}`; `pointer` is `null` when you point back to `$SILICON_HOME`. If the target already holds a different store, nothing is changed (`invalid_input`).

## peek ting enroll

Registers (or re-registers) this Silicon as a Ting recipient for peek. Login does this automatically; run it yourself when `peek login status` shows `"ting":{"subscribed":false}`, a send carries a `ting_not_enrolled` warning, or a delivery is stuck on `recipient_not_registered`. peek never re-enrolls on its own, because that would undo a grant you revoked on purpose. Events waiting in `authority_required` for the enrollment are retried right away.

```sh
peek ting enroll --json        # {"subscribed":true,"subscription_id":"sub_…"}
```

## peek register side

```sh
peek register side 5 --json
# {"slot":{"index":5,"side":"bottom"},"moved_from":null,"hotkey":"ctrl+cmd+5","warnings":[]}
```

Positions: 1 `top`, 2 `top-right`, 3 `right`, 4 `bottom-right`, 5 `bottom`, 6 `bottom-left`, 7 `left`, 8 `top-left`. One per Silicon: registering another number moves you (`moved_from` names the old one) and your drawing keeps running. A position held by another Silicon fails with `side_taken` (exit 4) and `details:{"owner","free":[…]}`.

`hotkey` is the Carbon's shortcut for the position: `ctrl+cmd+<position>` by default, or whatever modifier the Carbon chose in Peek.app Settings (for example `opt+cmd+5`).

## peek register drawing

Validates, stores and activates your bubble's JavaScript drawing. The file is resolved against the current directory and must be at most 256 KiB.

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

Releases your position and shortcut, deletes your drawing locally and on the server, and cancels your pending asks. No Ting events are sent.

```sh
peek unregister --json        # {"released_slot":3,"cancelled_asks":["ask_0192…"]}
```

`released_slot` is `null` when you held no position.

## peek send

Speaks, shows or asks on the Carbon's screen. Returns immediately.

| Flag | Meaning |
|---|---|
| `--speak <TEXT>` | 1–2000 characters, spoken with Deepgram Aura-2. |
| `--show <JSON\|@FILE\|->` | 1–3 text or image elements. See [Speak and show](show.md). |
| `--ask <JSON\|@FILE\|->` | One question: text, single_choice, multiple_choice, slider or range. See [Ask a question](ask.md). |
| `--voice <aura-2-name-lang>` | Overrides config `voice`, which overrides the per-language default. Needs `--speak`. |
| `--lang <bcp47>` | Forces the TTS language instead of detecting it. Any BCP 47 tag works; its primary subtag picks the language (`es-MX` speaks Spanish). Needs `--speak`. |
| `--duration <SECS>` | 1–120. How long a show stays when there is no speech, or after the speech ends. Without it, a bubble with speech slides back 1.5 s after the speech ends; the text-length default applies only to a show without speech. Needs `--show` or `--speak`; not with `--ask`. |
| `--expires-in <SECS>` | 10 s to 7 days, asks only. An unanswered ask then slides away and you receive `peek.ask.expired`. |
| `--notify speech_finished,show_dismissed` | Opt in to those events for this send (default: config `notify`). |
| `--wait[=<SECS>]` | Asks only. Keep the command open for the answer (default 120, at most 600). Write `--wait=60`, with the `=`; a bare `--wait` means 120. |

Rules, checked before anything is shown:

- At least one of `--speak`, `--show`, `--ask` (`nothing_to_send`, exit 2).
- `--show` and `--ask` are mutually exclusive, and the flags above must match what you send (`conflicting_flags`, exit 2).
- `@FILE` and `-` read the JSON from a file (relative to the current directory) or stdin. Image paths inside are also resolved against the current directory, and the CLI reads the bytes itself.
- Unknown JSON fields and duplicate keys are rejected (`invalid_input`, `invalid_json`), and so are two options with the same label (`invalid_input`). `details.field` names the field. Lengths count Unicode scalar values.
- You need a position and a drawing (`side_not_registered`, `drawing_not_registered`, exit 4).

```json
{"send_id":"snd_0192…","ask_id":"ask_0192…"|null,"slot":3,"status":"showing"|"queued",
 "speech":{"status":"pending"|"cached"|"skipped"|"unsupported_language","model":"aura-2-thalia-en","chars":42}|null,
 "warnings":[{"code":"carbon_away","message":"…"}]}
```

`speech` is `null` when there is no `--speak`. `status` is `queued` when an ask of yours is on screen, or when the Carbon is away.

| Warning | Meaning |
|---|---|
| `carbon_away` | The Carbon's screen is locked or the display is asleep. Nothing is shown to nobody: the send waits in your queue and is shown, in order, when the Carbon is back. An ask keeps its `--expires-in` clock while it waits, and speech starts only when the bubble is shown. |
| `ting_not_enrolled` | This Silicon is not a Ting recipient for peek, so answers cannot be delivered; they wait on the Mac. Run `peek ting enroll`. |
| `isi_ignored` | `$ISI` was invalid and was dropped from this send (no `metadata.isi` in its events). |
| `speak_language_unsupported` | The speech language has no voice (`details.language`); the text shows as a pill instead. |

With `--wait`, the command prints the final answer instead. Whatever it prints replaces the Ting event for that ask, so each outcome reaches you through exactly one channel:

```jsonc
// answered in time (no Ting event is sent for this answer)
{"ask_id":"ask_…","send_id":"snd_…","state":"answered","answer":{"kind":"single_choice","option_id":"keep","label":"Keep"},"via":"click","transcript":null,"answered_at":"…Z"}
// dismissed, expired or cancelled while waiting (no peek.ask.dismissed or peek.ask.expired is sent either)
{"ask_id":"ask_…","send_id":"snd_…","state":"dismissed"}
// timeout: exit 0, and the answer is delivered by Ting later
{"ask_id":"ask_…","send_id":"snd_…","state":"pending","delivery":"ting"}
```

Queueing is per position: a new show replaces a visible show; while an ask is pending, or while the Carbon is away, up to 5 new sends queue, then `slot_busy` (exit 4). Your ISIs share this queue.

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

- States: `pending`, `answered`, `dismissed`, `expired`, `cancelled`.
- `delivery.status`: `pending`, `authority_required`, `accepted`, `expired`, `cancelled`, `failed`, or `wait` (delivered to a live `send --wait` instead of Ting).
- `send_id`, `question`, `ask_type`, `transcript`, `created_at` and `expires_at` are present when known.
- `ask cancel` slides the question away and sends no Ting event. On an ask that is already closed it returns that ask's current state. An unknown id fails with `ask_not_found` (exit 4).
- `ask list --limit` is 1–200 (default 20).

## peek history

Your recent sends on this Mac, newest first. History never leaves the Mac.

```sh
peek history --limit 50 --json
# {"items":[{"send_id":"snd_…","kind":"ask","created_at":"…Z","closed_at":"…Z","close_reason":"answered","ask_id":"ask_…","ask_state":"answered"},
#            {"send_id":"snd_…","kind":"speak","created_at":"…Z","closed_at":"…Z","close_reason":"auto",
#             "warnings":[{"code":"speech_failed","message":"speech failed before any audio played: …"}]}]}
```

- `--limit` is 1–200 (default 50); page with `--before <SEND_ID>`.
- `close_reason` says how the bubble left the screen (it is absent while the send is still queued or showing):

| `close_reason` | Meaning |
|---|---|
| `speech_done` | the speech ended and the bubble slid back 1.5 s later |
| `auto` | a show without speech (or its `--duration`) ran out |
| `answered` | the Carbon answered the ask |
| `dismissed` | the Carbon closed it (Esc or the down-arrow) |
| `expired` | the ask reached its `--expires-in` |
| `cancelled` | you withdrew it (`peek ask cancel`, `peek unregister`) |
| `replaced` | your next show replaced it |
| `ui_disconnected`, `daemon_restarted` | Peek.app quit, or the helper restarted, while it was on screen |

- An item carries a `warnings` array (the same `{"code","message"}` shape as everywhere else) when something went wrong after `peek send` had already returned. Today that is `speech_failed` (the speech could not be played, so the text showed as a pill instead) or `speech_unavailable` (speech is not available for this Silicon or org right now, for example an org key that stopped working). The field is left out when there is nothing to report. In human mode the codes follow the row in brackets, such as `[speech_failed]`.

## peek status

One view of everything that matters for this Silicon on this Mac: position, drawing, queue, pending asks, deliveries, whether the Carbon is there, Peek.app and the helper.

```sh
peek status --json
# {"actor_id":"si:dj","context":"production",
#  "slot":{"index":3,"side":"right"},"drawing":{"sha256":"…","bytes":3174,"active":true,"last_error":null},
#  "queue":{"pending":0},"pending_asks":1,"deliveries":{"pending":0,"authority_required":0,"last_error":null},
#  "carbon":{"available":true,"reason":"ok"},
#  "ui_running":true,"daemon":{"running":true,"version":"0.1.1","protocol":1},"app":{"build":1001,"ui_running":true}}
```

- `slot` and `drawing` are `null` until you register them. `drawing.active` is `false` while the fallback visual shows. `drawing.server_sync` is the state of the drawing's backend copy (`synced`, `pending` or `authority_required`); `deliveries` counts Ting events only.
- `carbon.available` is `false` while the Carbon's screen is locked (`reason:"locked"`), the display is asleep (`"asleep"`) or off (`"display_off"`). Sends then wait in `queue` and are shown when it turns `true` again. An older Peek.app that never reports presence counts as available.
- `deliveries.authority_required` greater than 0 means answers are waiting for a valid session or Ting enrollment; `peek login status` and `peek doctor` say which.
- `warnings` appears when there is a pending note such as `drawing_fallback_active` (attached once, to the first `peek send`, `peek register side` or `peek status` after the fallback).

## peek org byo deepgram

Org admins can make peek use the org's own Deepgram key for every member's speech and transcription. The key is stored sealed on the backend and never returned.

```sh
peek --org tos org byo deepgram set --key-file - --base-url https://api.eu.deepgram.com < key.txt
peek --org tos org byo deepgram show --json      # {"configured":true,"org_id":"tos","updated_at":"…Z","base_url":"https://api.eu.deepgram.com"}
peek --org tos org byo deepgram delete           # {"configured":false,"org_id":"tos","updated_at":null,"base_url":null}
```

- `--base-url` must be an `https://` origin (host and optional port; no credentials, path or query; a trailing `/` is dropped) on a Deepgram host the backend allows. The backend operator sets the allowed hosts with `PEEK_BYO_DEEPGRAM_HOSTS` (comma-separated `host`, `*.domain` or `*`); the default is `*.deepgram.com`, so `https://api.eu.deepgram.com` works everywhere. `http://`, IP addresses and `localhost` are always refused, whatever the allowlist says. The CLI refuses a non-https URL, an IP address, `localhost`, credentials or a query before calling the backend (`invalid_input`); the backend refuses a host outside its allowlist with `invalid_input` and `details.reason: "base_url_not_allowed"`. Leave `--base-url` out to use `https://api.deepgram.com`.
- The allowlist is checked again every time the key is used. If the operator narrows it later, a stored base URL outside it makes speech fail with `speech_unavailable` and `reason: "org_base_url_not_allowed"`. peek never quietly swaps in its own key; an admin sets the key again with an allowed `--base-url`.
- The key must have Deepgram's Member role or higher; it is validated before it is saved.
- Once set there is no silent fallback to peek's key: a key that stops working makes speech fail with `speech_unavailable` and a `reason` (`org_key_invalid`, `org_out_of_credits`, `org_model_forbidden`, `org_base_url_not_allowed`).
- Non-admins get `not_org_admin` (exit 4).
- These commands only call the backend, so they work on Linux and Windows too.

## peek app

Manage Peek.app on this Mac.

| Command | Does | `--json` |
|---|---|---|
| `peek app status` | Whether Peek.app is installed, its build and signature, whether it and the helper run, the background item, the offered builds and the install log. | `{"supported":true,"platform","installed","path","build","short_version","bundle_id","signature":"valid"\|"invalid"\|"unchecked"\|"unreadable"\|"absent","running":{"ui","daemon"},"daemon":{…}\|null,"agent":"loaded"\|"not_loaded"\|"unknown","best_offer","bundled","last_install_script":[…]}`. On Linux and Windows: `{"supported":false,"platform":"linux-x86_64"}`, exit 0. |
| `peek app install` | Installs `~/Applications/Peek.app` from this CLI's package if it is missing, verifies its signature, removes quarantine and starts it. It never modifies an existing app. | `{"installed":true,"path","installed_now","offered_build","build","short_version","running":{"daemon":true,"ui"},"peekd_version"}` |
| `peek app update` | Offers the newest build this CLI knows to the helper, which installs it once nothing is on screen. Needs a logged-in home. | `{"scheduled","installed_build","offered_build"}` (plus `reason` when nothing is offered) |
| `peek app uninstall` | Unregisters the login item and helper and moves Peek.app to the Trash, then waits up to 15 s for it to go. `honeycomb uninstall 'peek'` does not do this. | `{"uninstalled":true,"was_installed","path","via":"peekd"\|"open"\|null}` |

## peek daemon

```sh
peek daemon status --json
# {"running":true,"pid":812,"version":"0.1.1","protocol":1,"socket":"/var/tmp/silicon-peek-501/peekd.sock","ui":{"running":true,"build":1001},"homes":3}
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
{"manager":"honeycomb","app_id":"peek","current_version":"0.1.1","auto_update":true,"can_replace_running_binary":false,
 "command":"honeycomb update 'peek'","app":{"build":1001,"best_offer":1001},"message":"Honeycomb manages the peek CLI: …"}
```

`auto_update` is `false` when `HONEYCOMB_AUTO_UPDATE` is off. `app` is `null` on Linux and Windows.

## peek doctor

Runs every check and prints the exact fix for anything that is wrong. It always exits 0; a check that cannot run is reported as a warning.

```sh
peek doctor --json
# {"checks":[{"name":"…","status":"ok"|"warn"|"fail","detail":"…","fix":"…"|null}, …],
#  "summary":{"ok":13,"warn":1,"fail":0},"version":"0.1.1","platform":"macos-aarch64"}
```

Checks, by `name`: `store` (permissions), `session`, `pending_login` (a login whose response was lost), `revocations` (logout revocations still being retried) and `ting` (enrollment), `backend` (`/readyz`), `honeycomb` (0.5.0 or newer), and on a Mac `app` (installed, signature, build), `agent` (the background item), `install_log`, `daemon` (socket and protocol), `mic` (permission, reported by Peek.app), `hotkeys` and `outbox` (the delivery backlog). Every check is always listed; one with nothing to report is `ok`. See [Troubleshooting](troubleshooting.md).
