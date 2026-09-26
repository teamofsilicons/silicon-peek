# Peek for Silicons

This page is for Silicons and the people who write them. It covers what is mandatory, the exact commands, and how answers come back into your flow. Every command here is non-interactive: peek never prompts, so it is safe under Stemcell (stdin is `/dev/null`) and inside ISIs.

## What is expected of you

| Must | Detail |
|---|---|
| Be logged in to peek | Stemcell does it when `peek` is in `silicon.apps`. By hand: `peek login <SLT>`. |
| Hold exactly one position | `peek register side <1-8>`. Your ISIs share it. |
| Have a registered drawing | `peek register drawing ./logo.js`. It is your face on the Carbon's screen. |
| Route peek events in your flow | Answers arrive as Ting events (`peek.ask.answered` and friends). Without a flow branch they reach `intuit` through Stemcell's default branch. |
| Keep asks self-contained | The Carbon sees only the bubble: at most 80 characters of question, and options that make sense on their own. |
| Dedupe by ting `id` | Delivery is at least once. The answer also carries `ask_id`. |

`peek send` never blocks on the Carbon. It prints the send and ask ids and exits in well under a second; the answer arrives later as a new message from your flow.

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

Then compile and connect:

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
| `voice` | `aura-2-<name>-<en\|es\|de\|fr\|nl\|it\|ja>` or null | null | your default speaking voice |
| `language` | BCP 47 primary subtag or null | null (detect) | the speech language when detection is ambiguous |
| `notify` | subset of `["speech_finished","show_dismissed"]` | `[]` | opt-in events for every send |
| `api_url` | https URL or null | null | the peek backend (same as `--api`) |
| `delivery_max_age_hours` | int 1..168 | 168 | how long undelivered answers keep retrying |

### Availability before public listing

Until peek's public listing is approved, only members of org `tos` can install it or log in. Any other org gets `403 private_application_organization_required`. Stemcell installs apps without a Honeycomb session, so `silicon connect` with `apps: [peek]` can fail for such an org until then. Leave `peek` out of `silicon.apps` and use a Carbon-installed `peek` with `SILICON_HOME=<your home>` in the meantime (see "Log in without Stemcell" below).

## Log in without Stemcell

A Silicon's peek state lives in `$SILICON_HOME/.peek/` (or `~/.peek/` when `SILICON_HOME` is unset). An empty or non-existent `SILICON_HOME` is an error, never a fallback.

```sh
H=/abs/path/to/silicon/home
SLT=$(iam -o json silicon-login --app-id peek --grant-org tos --approve-scopes | jq -r .slt)
SILICON_HOME=$H peek login "$SLT"            # or: printf %s "$SLT" | SILICON_HOME=$H peek login --token-file -
SILICON_HOME=$H peek login status --json
```

Set `SILICON_HOME` per command, as above. Do not `export` it in a shell you use for other tools: it re-roots IAM, Honeycomb, Ting and Space Station state at the same time.

The SLT is single use and lives two minutes. If the login response was lost (network drop), `peek login --recover` retries the same exchange within 10 minutes. Details: [IAM and sessions](iam.md).

## One-time setup: position and drawing

```sh
peek register side 3 --json                 # {"slot":{"index":3,"side":"right"},"moved_from":null,"hotkey":"ctrl+cmd+3","warnings":[]}
peek register drawing ./drawings/logo.js    # validated for 90 frames, then activated
```

- If the position is taken, you get `side_taken` (exit 4) with the owner and the free positions: `{"details":{"owner":"si:dj","free":[1,4,6,7]}}`. Pick a free one. Running `register side` again with another number moves you there; your drawing keeps running.
- The drawing path is resolved against your **current directory**. The CLI reads the bytes, so Peek.app never opens your files.
- A drawing that fails validation is not activated and the previous one stays. Use `--check` to validate without replacing, `--preview out.png` to see the test frames. See [Drawing the visual](drawing.md).
- `peek unregister` releases the position, deletes the drawing (locally and on the server) and cancels your pending asks. No Ting events are sent for your own actions.

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

Output with `--json`:

```json
{"send_id":"snd_0192…","ask_id":"ask_0192…","slot":3,"status":"showing",
 "speech":{"status":"pending","model":"aura-2-thalia-en","chars":25},"warnings":[]}
```

`status` is `queued` when an earlier ask of yours is still on screen; up to 5 sends wait behind it, then `slot_busy` (exit 4). Keep the `ask_id`: it is in the answer, and `peek ask get <ASK_ID>` shows the local state at any time.

The JSON schemas, limits and defaults are in [Speak and show](show.md) and [Ask a question](ask.md).

## Put a blurb in `tools.md`

Stemcell does not inject app docs into prompts. Copy this into your `tools.md` (or `prompts/tools.md`) so your ISIs know when to use peek:

```text
> [peek]
`peek --help`
peek pops a small bubble on your carbon's Mac screen (one of 8 positions, yours alone) to speak, show, or ask something quick.
use it for one-off, no-history moments: "is this file safe to delete?", "now playing …", a nudge. use [dm] for anything that needs a thread.
first time: `peek register side N` and `peek register drawing ./path.js`. `peek send` fails until both are set.
`peek send --speak "..." --show '{...}'` or `peek send --speak "..." --ask '{...}'` returns immediately. the answer arrives later as a peek.ask.answered message.
keep asks self-contained: the carbon sees nothing but the bubble. dedupe answers by ask_id.
```

## Receive answers in your flow

peek records `$ISI` with every send and puts it in the event's `metadata.isi`, so your flow can route an answer back to the ISI that asked. A Carbon-initiated message carries the ISI of the most recent send in that position, if any. `ISI` is optional context: peek works the same without it, and it never grants authority.

Your Stemcell webhook receives:

```json
{"tings":[{"id":"msg_…","created_at":"2026-09-26T10:00:07Z","type":"peek.ask.answered",
  "data":{"schema":1,"ask_id":"ask_0192…","send_id":"snd_0192…","question":"Delete old builds?","ask_type":"single_choice",
          "answer":{"kind":"single_choice","option_id":"1","label":"Delete"},"via":"click","transcript":null,
          "asked_at":"2026-09-26T10:00:00Z","answered_at":"2026-09-26T10:00:07Z","slot":3,"context":"production"},
  "metadata":{"isi":"deliberate","peek_version":"0.1.0"},"key":"si:cleanup/ask_0192…/answered"}]}
```

Both flows below were compiled with `silicon compile` and executed against a sample batch.

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

An answer printed to a live `--wait` **replaces** the Ting event, so each answer reaches you through exactly one channel. If the wait times out or the process dies, the answer is delivered by Ting as usual. Model tool calls often time out after about two minutes; keep `--wait` short there, or do not use it.

## Check your state

```sh
peek status --json            # position, drawing, queue, pending asks, deliveries, app and helper
peek ask get ask_0192…        # pending | answered | dismissed | expired | cancelled, with the answer
peek ask list --state pending
peek history --limit 20       # your recent sends on this Mac
peek doctor                   # every check, with the exact fix
```

## Errors you will meet

| Code | Exit | Meaning and fix |
|---|---|---|
| `side_not_registered` | 4 | "no position registered for si:cleanup; run `peek register side <1-8>` first (free: 1,4,6,7)" |
| `drawing_not_registered` | 4 | "no drawing registered for si:cleanup; run `peek register drawing ./logo.js` first" |
| `side_taken` | 4 | Another Silicon holds it; `details.free` lists the free ones. |
| `slot_busy` | 4 | Five sends already wait behind a pending ask. Wait for the answer, or `peek ask cancel`. |
| `not_logged_in`, `session_rejected` | 3 | Log in again (`si auth setup peek`, or a new SLT and `peek login`). |
| `platform_unsupported` | 4 | This `peek` runs on Linux or Windows; bubbles need macOS. See [Platforms](platforms.md). |
| `peek_service_unavailable` | 5 | Peek.app or its helper did not start. Run `peek doctor`. |

When your drawing crashed on screen, your next `peek send`, `peek register side` or `peek status` result carries a `drawing_fallback_active` warning with the message and stack (once; `peek status` keeps showing `drawing.active: false` until you register again). The full list is in [CLI reference](cli.md) and [Troubleshooting](troubleshooting.md).

## Next

- [Ting events](ting.md): every event type and payload.
- [Drawing the visual](drawing.md): the drawing API, with six examples.
- [Testing environments](testing.md): run all of this against isolated test identities.
