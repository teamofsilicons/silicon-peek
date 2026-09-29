# Peek for Silicons

This page is for Silicons and the people who write them. It covers what is mandatory, the exact commands, and how answers come back into your flow. Every command here is non-interactive: peek never prompts, so it is safe under Stemcell (stdin is `/dev/null`) and inside ISIs.

## What is expected of you

| Must | Detail |
|---|---|
| Be logged in to peek | Stemcell does it when `peek` is in `silicon.apps`. By hand: `peek login <SLT>`. |
| Hold exactly one position | `peek register side <1-8>`, or save `position` in config to claim it on first send. Your ISIs share it. |
| Route peek events in your flow | Answers arrive as Ting events (`peek.ask.answered` and friends). Stemcell has no default flow: add a branch such as [Flow A](#flow-a-send-every-peek-event-to-one-isi), or events are dropped. Without Stemcell, register a webhook of your own ([below](#receive-answers-without-stemcell)). |
| Keep asks self-contained | The Carbon sees only the bubble: at most 80 characters of question, and options that make sense on their own. |
| Dedupe by ting `id` | Delivery is at least once. The answer also carries `ask_id`. |

`peek send` never blocks on the Carbon. It prints the send and ask ids and exits once queued (the first configured drawing may take longer to validate); the answer arrives later as a new message from your flow.

Pass `--json` whenever a program reads the output. Then stdout holds exactly one JSON value on success; on failure stdout is empty and one error object goes to stderr. Without `--json`, peek prints readable text and puts hints on stderr.

## Add peek to `silicon.yaml`

```yaml
silicon:
  id: si:cleanup
  org_id: tos
  token: REPLACE_WITH_SILICON_TOKEN
  timezone: Asia/Kolkata
  SILICON_HOME: ! pwd
  inference_providers:
    - all-available-providers
  apps:
    - peek                      # bare app id; iam and ting are managed for you
  app_configs:                  # optional; becomes `peek config set '{…}'` after every login
    peek:
      notify: [speech_finished]
      delivery_max_age_hours: 48
```

Merge these keys into your existing `silicon.yaml`: `silicon compile` also needs your `isi`, `access` and `flow` sections, and `flow` needs a peek branch ([Flow A](#flow-a-send-every-peek-event-to-one-isi) is the minimum). Then compile and connect:

```sh
silicon compile /abs/path/silicon.yaml
silicon connect /abs/path/silicon.yaml
```

On `connect`, Stemcell installs peek into your private package registry, runs `peek iam --json`, checks `peek login status --json`, mints a short-lived token with `iam silicon-login … --app-id peek --grant-org <org> --approve-scopes`, runs `peek login <SLT>`, checks the status again, and finally runs `peek config set` with your `app_configs.peek`. It skips the login when it checked less than 48 hours ago, or when `peek login status` still says `true`.

From inside a running ISI you can do the same without editing the YAML by hand:

```sh
si app install peek      # installs, logs in, and appends peek to silicon.apps
si auth setup peek       # forces a fresh login if peek reports logged out
```

`app_configs.peek` accepts only these keys. Anything else makes `peek config set` fail with `unknown_config_key` (exit 2), and Stemcell reports the error under `silicon.app_configs.peek`.

| Key | Type | Default | Effect |
|---|---|---|---|
| `telemetry` | bool | `true` | this home's telemetry ([Telemetry](telemetry.md)) |
| `voice` | Google voice name or custom ID, or null | null (`Kore`) | your default speaking voice |
| `voice_instructions` | string, 1–2000 characters, or null | null | delivery directions; accents, emotion, pace, pitch and tone |
| `language` | BCP 47 primary subtag or null | null (detect) | fallback language hint when Peek detection is ambiguous |
| `notify` | subset of `["speech_finished","show_dismissed","shown"]` | `[]` | opt-in events for every send |
| `api_url` | https URL or null | null | the peek backend (same as `--api`) |
| `delivery_max_age_hours` | int 1..168 | 168 | how long undelivered answers keep retrying |

peek is a public app on Honeycomb: `honeycomb install 'peek'` needs no Honeycomb login, and any org can log in.

## Log in without Stemcell

A Silicon's peek state lives in `$SILICON_HOME/.peek/` (or `~/.peek/` when `SILICON_HOME` is unset). An empty or non-existent `SILICON_HOME` is an error, never a fallback.

```sh
H=/abs/path/to/silicon/home
SLT=$(SILICON_HOME=$H iam -o json silicon-login --app-id peek --grant-org <org> --approve-scopes | jq -r .slt)
SILICON_HOME=$H peek login "$SLT"            # or: printf %s "$SLT" | SILICON_HOME=$H peek login --token-file -
SILICON_HOME=$H peek login status --json
```

Set `SILICON_HOME` per command, as above. Do not `export` it in a shell you use for other tools: it re-roots IAM, Honeycomb, Ting and Space Station state at the same time.

The SLT is single use and lives two minutes. If the login response was lost (network drop), `peek login --recover` retries the same exchange within 10 minutes. Details: [IAM and sessions](iam.md).

`peek logout` ends this home's session but keeps your Ting grant, because other homes of the same Silicon (a Stemcell home and a hand-run one, say) share it. `peek logout --revoke-ting` removes the grant too; then every home of this Silicon needs `peek ting enroll` before answers flow again.

## One-time setup: position and optional drawing

```sh
peek register side 3 --json                 # {"slot":{"index":3,"side":"right"},"moved_from":null,"hotkey":"ctrl+cmd+3","warnings":[]}
peek register drawing ./drawings/logo.js    # optional; validated for 90 frames, then activated
```

- To save defaults instead, run `peek config set '{"position":3,"drawing":"./drawings/logo.js"}'`. Sends apply missing registrations from these defaults; existing registrations win. Omit `drawing` for the built-in visual.
- If the position is taken, you get `side_taken` (exit 4) with the owner and the free positions: `{"details":{"owner":"si:dj","free":[1,4,6,7]}}`. Pick a free one. Running `register side` again with another number moves you there; your drawing keeps running.
- The drawing path is resolved against your **current directory**. The CLI reads the bytes, so Peek.app never opens your files.
- A drawing that fails validation is not activated and the previous one stays. Use `--check` to validate without replacing, `--preview out.png` to see the test frames. See [Drawing the visual](drawing.md).
- `peek unregister` releases the position, deletes the drawing (locally and on the server) and cancels your pending asks, queued sends and scheduled sends. No Ting events are sent for your own actions.

## Send

```sh
# say something (the bubble shows your drawing while it speaks)
peek send --speak "Backup finished. 214 files, 3.2 GB."

# glanceable update: speech plus up to three text or image elements
peek send --speak "Now playing Low Tide." \
  --show '{"elements":[{"type":"image","path":"./covers/low-tide.jpg","caption":"Low Tide by The Silicons"},{"type":"text","text":"2022"}]}'

# a quick question; the answer comes back through Ting
peek send --speak "Found 3 GB of old builds." \
  --ask '{"question":"Delete old builds?","type":"single_choice","options":["Delete","Keep"]}'
```

Speech uses Google Gemini 3.8 Flash TTS and starts playing as audio arrives. Customize it for your Carbon from the CLI:

```sh
peek config set '{"voice":"Kore","voice_instructions":"Gentle, conversational delivery with a relaxed pace."}'
peek send --speak 'Now playing <indian accent>Anuv Jain</indian accent>. <short pause> Enjoy.'
peek send --speak 'The build passed! <chuckle>' --voice-instructions 'Sound delighted.'
```

Peek translates paired accent spans into Google's style metadata; delivery is best effort. For a consistent regional accent, use a suitable regional or custom voice. The [voice guide](show.md#customize-a-silicons-voice) includes persistent defaults, per-send overrides and Google's full recommended expression-tag list.

Output with `--json`:

```json
{"send_id":"snd_0192…","ask_id":"ask_0192…","slot":3,"status":"showing","queue_position":0,"waiting":0,
 "expires_at":null,"schedule_id":null,"due_at":null,"tz":null,"replaced_send_id":null,
 "speech":{"status":"pending","model":"Kore","chars":25},"warnings":[]}
```

`status` is `queued` when an earlier send of yours is still on screen (`queue_position` says how many are ahead), or when the Carbon is away or paused Peek. Keep the `ask_id`: it is in the answer, and `peek ask get <ASK_ID>` shows the local state at any time.

Warnings in the result tell you what the Carbon will or will not get. The send itself still went through:

| Warning | Meaning | What to do |
|---|---|---|
| `carbon_away` | The Carbon's screen is locked or the display is asleep. The send waits in your queue and is shown, in order, when the Carbon is back. Deadlines keep running while it waits, and speech starts only when the bubble is actually shown. | Nothing. `peek status --json` shows `"carbon":{"available":false,"reason":"locked"\|"asleep"\|"display_off"}`. |
| `carbon_paused` | The Carbon paused all peeks. The send waits in your queue and is shown when they resume. | Nothing; do not resend. |
| `ting_not_enrolled` | You are not a Ting recipient for peek (the grant was revoked, or enrollment failed), so answers cannot reach you. They wait on the Mac. | `peek ting enroll`; waiting answers are then retried right away. |
| `isi_ignored` | `$ISI` was invalid (over 160 characters, or more than one line), so it was dropped from this send and its events carry no `metadata.isi`. | Fix `ISI`, or unset it. |
| `speak_language_unsupported` | An older helper rejected the language. | Update Peek.app; current Gemini speech no longer uses the former seven-language list. |
| `timezone_fallback_utc` | The Mac's time zone could not be read, so an `--at` or `--expires-at` without an offset was read as UTC. | Give the time with an offset (`18:00+05:30`) or pass `--tz`. |

The JSON schemas, limits and defaults are in [Speak and show](show.md) and [Ask a question](ask.md).

## Queue, expire, replace, schedule

Your sends take turns on your position, first in, first out. A new send never replaces the one on screen: it waits until that one is done. At most 5 wait behind the one on screen; one more fails with `queue_full` (exit 4, retryable). All your ISIs share this queue, so a busy ISI can fill it for the others.

```sh
peek queue                                   # what is on screen and what waits, with ids and ages
peek cancel snd_0192…                        # withdraw one send (snd_…, ask_… or sch_…); no Ting event
peek queue clear                             # drop every waiting send; --all also the one on screen

peek send --show @status.json --expires-in 15m          # drop it if it is not done in 15 minutes
peek send --speak "Correction: the deploy failed." --replace   # take over the bubble now
peek send --speak "Stand-up in five minutes." --at 09:55       # once, at 09:55 on the Mac's clock
peek send --ask @q.json --in 2h --expires-at 2026-09-27T20:00  # a scheduled ask with a deadline
peek schedule list                            # scheduled sends not due yet; cancel with peek schedule cancel <ID>
```

- **Expiry** (`--expires-in 90s|15m|2h|1d`, or `--expires-at <DATETIME>`) works on every kind of send. A send still waiting when its deadline passes is never shown; one on screen slides away. You receive `peek.send.expired` (or `peek.ask.expired` for an ask), and its `shown` field says whether the Carbon ever saw it.
- **`--replace`** closes whatever of yours is current, even a pending ask (cancelled without an answer or a Ting event), and shows the new send at once. Your waiting sends keep their order.
- **Scheduling** (`--in <DURATION>` or `--at <DATETIME>`, one time, up to 365 days ahead, 500 per Silicon) stores the send on the Mac and queues it at its time. `--at 18:00` means 18:00 on the Mac's clock; write `18:00+05:30` or pass `--tz Asia/Kolkata` to be explicit, which is wise when your own `timezone` differs from the Carbon's. A scheduled send takes only `--expires-at` and never `--wait`. When it comes due you receive `peek.schedule.due` (outcome `shown`, `queued`, `expired` or `replaced`), and `peek.send.shown` when it appears. If the Mac was asleep, it fires on wake unless it expired.

All flags, outputs and limits: [CLI reference](cli.md#queue-expiry-and-scheduling).

## Put a blurb in `tools.md`

Stemcell does not inject app docs into prompts. Copy this into your `tools.md` (or `prompts/tools.md`) so your ISIs know when to use peek:

```text
> [peek]
`peek --help`
peek pops a small bubble on your carbon's Mac screen (one of 8 positions, yours alone) to speak, show, or ask something quick.
use it for one-off, no-history moments: "is this file safe to delete?", "now playing …", a nudge. use [dm] for anything that needs a thread.
first time: `peek config set '{"position":3}'` or `peek register side N`. Peek has a built-in visual; optionally set `drawing` to a JavaScript file path for your own logo.
`peek send --speak "..." --show '{...}'` or `peek send --speak "..." --ask '{...}'` returns immediately. the answer arrives later as a peek.ask.answered message.
your sends queue one at a time (max 5 waiting; `peek queue`, `peek cancel <id>`). add `--expires-in 15m` when a send goes stale, `--at 09:55` or `--in 2h` to schedule one.
set a voice with `peek config set '{"voice":"Kore","voice_instructions":"Warm, relaxed delivery."}'`; override one send with `--voice-instructions`. `peek docs show` lists vocal tags and Peek accent spans such as `<indian accent>Anuv Jain</indian accent>`.
keep asks self-contained: the carbon sees nothing but the bubble. dedupe answers by ask_id.
```

## Receive answers without Stemcell

Without Stemcell nothing listens on `http://<handle>.<org>.localhost/events`, so give Ting a webhook of your own. Log in to Ting in the same home as peek, pick the org, and register a local URL:

```sh
H=/abs/path/to/silicon/home
# a Silicon already logged in to IAM in this home
SILICON_HOME=$H iam -o json silicon-login --app-id ting --grant-org <org> --approve-scopes | jq -r .slt | SILICON_HOME=$H ting login --token-stdin
# or with the Silicon's own credentials (what Stemcell runs)
SILICON_HOME=$H iam -o json silicon-login --sid si:<handle> --stk <STK> --app-id ting --grant-org <org> --approve-scopes | jq -r .slt | SILICON_HOME=$H ting login --token-stdin
SILICON_HOME=$H ting org use <org>
SILICON_HOME=$H ting webhook http://127.0.0.1:8787/peek     # reports state=connected
```

Ting then POSTs each batch to that URL. The body is the same `{"tings":[…]}` shown in the next section; answer `204` once you have stored it (anything else makes Ting retry). A ten-line Python receiver:

```python
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
class Peek(BaseHTTPRequestHandler):
    def do_POST(self):
        batch = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        for t in batch["tings"]:                      # dedupe by t["id"]; at least once
            print(t["type"], json.dumps(t["data"]), flush=True)
        self.send_response(204); self.end_headers()
    def log_message(self, *a): pass
HTTPServer(("127.0.0.1", 8787), Peek).serve_forever()
```

Details and the full walk-through: [Ting events](ting.md#receive-answers-without-stemcell).

## Receive answers in your flow

peek records `$ISI` with every send and puts it in the event's `metadata.isi`, so your flow can route an answer back to the ISI that asked. A Carbon-initiated message carries the ISI of the most recent send in that position, if any. `ISI` is optional context: peek works the same without it, and it never grants authority. An invalid `ISI` never fails a send; it is dropped with an `isi_ignored` warning.

Your Stemcell webhook receives:

```json
{"tings":[{"id":"msg_…","created_at":"2026-09-26T10:00:07Z","type":"peek.ask.answered",
  "data":{"schema":1,"ask_id":"ask_0192…","send_id":"snd_0192…","question":"Delete old builds?","ask_type":"single_choice",
          "answer":{"kind":"single_choice","option_id":"1","label":"Delete"},"via":"click","transcript":null,
          "asked_at":"2026-09-26T10:00:00Z","answered_at":"2026-09-26T10:00:07Z","slot":3,"context":"production"},
  "metadata":{"isi":"deliberate","peek_version":"0.1.2"},"key":"si:cleanup/ask_0192…/answered"}]}
```

Stemcell has no default flow, so without one of these branches peek events are dropped (an empty `flow: []` compiles and drops every ting). Both flows below were compiled with `silicon compile` and executed against a sample batch.

### Flow A: send every peek event to one ISI

The simplest branch. It is batch-safe: one `send` per kind, built with `filter`, `map` and `join`.

```yaml
flow:
  - if:
      condition: '{request.tings.exists(t, t.type.startsWith("peek."))}'
      then:
        - send:
            isi: intuit
            message: '{request.tings.filter(t, t.type.startsWith("peek.")).map(t, "Peek " + t.type + " from " + (has(t.metadata.isi) ? t.metadata.isi : "carbon") + ":\n" + make_readable(t.data)).join("\n\n")}'
            catch:
              - log:
                  message: 'peek delivery failed: {error}'
  - if:
      condition: '{request.tings.exists(t, !t.type.startsWith("peek."))}'
      then:
        - send:
            isi: intuit
            message: '{request.tings.filter(t, !t.type.startsWith("peek.")).map(t, make_readable(t)).join("\n\n")}'
```

### Flow B: route each answer to the ISI that asked

Uses `metadata.isi`, including session-mode addresses (`worker.terminal:build-17` goes to ISI `worker.terminal`, session `build-17`), and falls back to `intuit`. It is injection-safe: ting data is only ever read through `request.tings[var.i]` and never pasted into the generated expression.

```yaml
flow:
  - var: {name: i, value: 0}
  - |-
      {request.tings.map(ting, [
        {"var": {"name": "t", "value": "{request.tings[var.i]}"}},
        {"var": {"name": "asker", "value": "{has(var.t.metadata.isi) && var.t.metadata.isi != null ? var.t.metadata.isi : \"\"}"}},
        {"var": {"name": "asker_isi", "value": "{var.asker.split(\":\")[0]}"}},
        {"if": {"condition": "{var.t.type.startsWith(\"peek.\") && var.asker_isi in isi && isi[var.asker_isi].primary_send_mode == \"global\"}",
          "then": [{"send": {"isi": "{var.asker_isi}", "message": "Peek {var.t.type} (ting {var.t.id}):\n{make_readable(var.t.data)}",
                             "catch": [{"send": {"isi": "intuit", "message": "Peek ting for {var.asker_isi} could not be delivered: {error}\n{make_readable(var.t)}"}}]}}]}},
        {"if": {"condition": "{var.t.type.startsWith(\"peek.\") && var.asker_isi in isi && isi[var.asker_isi].primary_send_mode == \"session\" && size(var.asker.split(\":\")) == 2}",
          "then": [{"send": {"isi": "{var.asker_isi}", "session_id": "{var.asker.split(\":\")[1]}", "message": "Peek {var.t.type} (ting {var.t.id}):\n{make_readable(var.t.data)}"}}]}},
        {"if": {"condition": "{var.t.type.startsWith(\"peek.\") && !(var.asker_isi in isi)}",
          "then": [{"send": {"isi": "intuit", "message": "Peek {var.t.type} (ting {var.t.id}):\n{make_readable(var.t.data)}"}}]}},
        {"if": {"condition": "{!var.t.type.startsWith(\"peek.\")}",
          "then": [{"send": {"isi": "intuit", "message": "New Ting notification:\n{make_readable(var.t)}"}}]}},
        {"var": {"name": "i", "value": "{var.i + 1}"}}
      ]).flatten()}
```

Notes that matter:

- Inside a generated-flow block scalar (`|-`), write `\n`, not `\\n`. CEL decodes `\n`; `\\n` arrives as a literal backslash and `n`.
- Never build the generated expression from ting fields. Go through `request.tings[var.i]`, or text inside an answer becomes executable CEL.
- Sending to a session-mode ISI whose ephemeral session has ended creates a new session with that id. Route session-mode answers to your coordinator if you prefer.
- Flows are re-read from disk for every batch, so you can add the peek branch without reconnecting.
- Delivery is at least once, and a crash can replay flow actions. Make side effects idempotent by ting `id` or `ask_id`.

### Test your flow locally

Inject a fake batch into your own Silicon (replace `<handle>` and `<org>`):

```sh
curl --fail-with-body -H 'Content-Type: application/json' --data '{"tings":[{"id":"local-1","type":"peek.ask.answered","data":{"ask_id":"a1","answer":{"kind":"text","text":"yes"}},"metadata":{"isi":"intuit"}}]}' http://<handle>.<org>.localhost/events
```

## Or wait for the answer inline

For a simple script, `--wait` keeps the command open until the Carbon answers (default 120 s, at most 600 s):

```sh
peek send --json --ask '{"question":"Ship it?","type":"single_choice","options":["Ship","Hold"]}' --wait=60
# answered: {"ask_id":"ask_…","send_id":"snd_…","state":"answered","answer":{"kind":"single_choice","option_id":"1","label":"Ship"},"via":"click","transcript":null,"answered_at":"…Z"}
# timeout:  {"ask_id":"ask_…","send_id":"snd_…","state":"pending","delivery":"ting"}
```

Whatever a live `--wait` prints **replaces** the Ting event: an answer, and also a dismissal or an expiry (`{"state":"dismissed"}`, `{"state":"expired"}`, `{"state":"cancelled"}`, or `{"state":"replaced"}` when a later `--replace` of yours took over). So each outcome reaches you through exactly one channel. If the wait times out or the process dies, the outcome is delivered by Ting as usual. Model tool calls often time out after about two minutes; keep `--wait` short there, or do not use it.

## Check your state

```sh
peek status --json            # position, drawing, queue, pending asks, deliveries, Carbon presence, app and helper
peek queue                    # the send on screen and the ones waiting behind it
peek schedule list            # scheduled sends that are not due yet
peek ask get ask_0192…        # pending | answered | dismissed | expired | cancelled | replaced, with the answer
peek ask list --state pending
peek history --limit 20       # your recent sends on this Mac
peek doctor                   # every check, with the exact fix
```

## Errors you will meet

| Code | Exit | Meaning and fix |
|---|---|---|
| `side_not_registered` | 4 | "no position registered for si:cleanup; run `peek register side <1-8>` first (free: 1,4,6,7)" |
| `side_taken` | 4 | Another Silicon holds it; `details.free` lists the free ones. |
| `queue_full` | 4 | Five of your sends already wait behind the one on screen. `peek queue`, then `peek cancel <SEND_ID>` or `peek queue clear`; or retry later. (`slot_busy` in older versions.) |
| `schedule_full` | 4 | 500 sends are already scheduled. `peek schedule cancel <ID>` or `peek schedule clear`. |
| `send_not_found`, `schedule_not_found` | 4 | `peek cancel` or `peek schedule cancel` with an id that is not yours or does not exist. `peek queue`, `peek schedule list`. |
| `app_update_pending` | 5 | Peek.app on the Mac is older than your CLI and does not support a new option yet (`details.missing_features`). `peek app update`, then retry. |
| `invalid_input` | 2 | The JSON does not match the schema, for example two options with the same label. `details.field` names the offending field. |
| `not_logged_in`, `session_rejected` | 3 | Log in again (`si auth setup peek`, or a new SLT and `peek login`). |
| `platform_unsupported` | 4 | This `peek` runs on Linux or Windows; bubbles need macOS. See [Platforms](platforms.md). |
| `peek_service_unavailable` | 5 | Peek.app or its helper did not start. Run `peek doctor`. |

When your drawing crashed on screen, your next `peek send`, `peek register side` or `peek status` result carries a `drawing_fallback_active` warning with the message and stack (once; `peek status` keeps showing `drawing.active: false` until you register again). The full list is in [CLI reference](cli.md) and [Troubleshooting](troubleshooting.md).

## Next

- [Ting events](ting.md): every event type and payload.
- [Drawing the visual](drawing.md): the drawing API, with six examples.
- [Testing environments](testing.md): run all of this against isolated test identities.
