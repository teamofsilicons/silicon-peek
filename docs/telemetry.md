# Telemetry

peek records operational telemetry by default, so failures can be traced from the CLI through the helper and the backend. It never records what anyone said, showed, asked or answered. Any one of the opt-outs below turns it off, and an explicit opt-out always wins.

## Turn it off

| Where | How |
|---|---|
| One Silicon home (CLI and its helper traffic) | `peek config telemetry off`, or `peek config set '{"telemetry":false}'`. Stored in `$SILICON_HOME/.peek/config.json` and pushed to the helper. |
| One process | `peek --no-telemetry …`. Only that CLI process: the helper keeps following the home's config. |
| Environment (any of `0`, `false`, `off`, `no`) | `PEEK_TELEMETRY`, `SPACE_STATION_TELEMETRY`, `SILICON_TELEMETRY`. Stemcell sets `SPACE_STATION_TELEMETRY=0` when its own telemetry is off, so a Silicon that opted out of Stemcell telemetry is opted out of peek's too. The CLI forwards this opt-out to the helper (below), so the helper's work for that home is not recorded either. |
| Peek.app | **Settings → General → Share usage and diagnostics**. Turning it off also clears the helper's unsent telemetry. |
| The website | the "Share anonymous usage" switch in the page footer (stored in this browser's `localStorage` as `peek.telemetry`) |
| Every request to the peek backend | the header `X-Peek-Telemetry: off` makes the backend skip its request events for that call. peek's CLI and helper send it for you when you opted out. |
| The whole backend (operators) | `PEEK_TELEMETRY=off` disables every recorder, and the gateway accepts and drops relayed events. |

### The helper and environment opt-outs

The helper (peekd) keeps working for a home after a command returns: it delivers answers, plays speech and uploads drawings. It does not share the CLI's environment, so it decides per home from that home's `config.json` and from what the CLI last told it with `config.sync`:

- `peek config telemetry off` (or `set`, `unset`) pushes the home's effective setting. When the file allows telemetry but the CLI's environment opts out, the CLI sends `telemetry: false` and marks it as an environment opt-out.
- Every command that talks to the helper for its home under an environment opt-out ends by forwarding it the same way. This includes `send`, `status`, `register`, `ask`, `history` and `login`.
- While the opt-out is forwarded, the helper records no events about that home and sends `X-Peek-Telemetry: off` on every backend call it makes for it.
- The opt-out stays until a command of that home runs without it and hands the helper its telemetry, or until `peek config …` syncs without it. An opt-out in `config.json` is never lifted this way.
- `--no-telemetry` covers only its own process and is not forwarded.

The IAM SDK inside peek's backend has its own process-wide switch, `PEEK_IAM_SDK_TELEMETRY`, which follows `PEEK_TELEMETRY` unless an operator sets it. It records no content either.

## Where it goes

peek uses [Space Station](https://spacestation.teamofsilicons.com), the TOS event store, with four tables in org `tos`:

| Table | Written by |
|---|---|
| `peekbackend` | the peek backend, directly |
| `peekclidaemon` | the CLI and the helper (peekd), relayed through the backend |
| `peekfrontendanalytics` | the website's automatic analytics, and Peek.app's automatic analytics (`source:"mac"`) |
| `peekfrontendevents` | the website's explicit events, and Peek.app's explicit events (`source:"mac"`) |

Table keys exist **only** on the backend. They are never in the CLI, the helper, the app, the website bundle or a release artifact. Everything else sends events to the backend's gateway (`POST /api/web/telemetry`, below), which forwards them.

- The **CLI** hands its event to the helper and returns immediately. With no helper (Linux, Windows, no GUI session) it posts directly with a 300 ms timeout and drops the event on failure. Telemetry never slows a command down.
- The **helper** keeps at most 1,000 unsent events in its database (oldest dropped first) and sends batches of up to 40 every 10 seconds.
- **Peek.app** sends its events to the helper; it has no network access of its own.
- The **website** uses `@teamofsilicons/space-station-web` through the same-origin `/api/web/telemetry` route.

## The gateway

`POST /api/web/telemetry` takes the body `@teamofsilicons/space-station-web` sends, `{"table","events":[…]}`, and answers `204`:

- **No `Idempotency-Key` is needed** (browsers never send one). Space Station deduplicates events by their id, so a retried batch is harmless.
- **Tables:** only `peekclidaemon`, `peekfrontendanalytics` and `peekfrontendevents`. Any other table gets `403 telemetry_table_unavailable`; `peekbackend` is written by the backend alone.
- **Origin:** a request that carries an `Origin` header must come from an origin in the backend's `PEEK_WEB_ORIGINS` (in production, `https://peek.teamofsilicons.com`, which reaches it through the same-origin `/api` rewrite); anything else gets `403 origin_not_allowed`. The CLI and the helper send no `Origin`.
- **Size and rate:** 1–40 events and at most 64 KiB per request (`invalid_input`, `payload_too_large`), and 6,000 events per minute in total (`429 rate_limited` with `Retry-After`).
- **Silent drops, still `204`:** when the table has no key configured on the backend, when the request carries `X-Peek-Telemetry: off`, when it comes from a testing environment (`X-Testing-Environment-Key`), and when the backend's `PEEK_TELEMETRY` is off. Nothing is forwarded in those cases, and the sender cannot tell, by design.

## What is recorded

Every event is self-contained:

```jsonc
{"schema_version":1,"app":"peek","service":"peek-backend|peek-cli|peek-daemon|peek-mac|peek-web",
 "source":"backend|cli|daemon|mac|web","version":"0.1.2","environment":"production|testing|development",
 "instance_id":"<uuidv7 per process>","trace_id":"<uuidv7 created by the CLI and carried in X-Peek-Trace-Id>",
 "step":"send.validate","event":"send.displayed","progress":1,"outcome":"ok|error|skipped|timeout",
 "duration_ms":412,"error_code":null,"isi":"deliberate",
 "actor":{"kind":"silicon","hash":"<first 16 hex of sha256('tos:si:dj')>"},
 "client":{"os":"macos","os_version":"27.0","arch":"aarch64","locale":"en_IN"},
 "context":{ /* allowlisted keys only */ }}
```

- **Actors are hashed.** `actor.hash` is the first 16 hex characters of `sha256("<org>:<actor>")`. Raw ids such as `si:dj` are never recorded.
- **`context` accepts only these keys:** `slot`, `mode`, `appearance`, `speak_chars`, `show_elements`, `show_kinds`, `ask_type`, `options_count`, `drawing_bytes`, `drawing_sha256`, `answer_latency_ms`, `gesture`, `input`, `shortcut`, `ting_status`, `attempt`, `http_route` (a route template, never a concrete URL), `method`, `status`, `update_from`, `update_to`, `command`, `tts_model`, `tts_ttfb_ms`, `stt_ms`, `stt_language`, `stt_request_id`, `matched`, `dg_request_id` (legacy), `key_source`, `queue_waiting` (how many of the Silicon's sends were waiting), `scheduled` (whether the send came from `--in`/`--at`).

Events by source:

| Source | Events |
|---|---|
| backend | `http.completed` (route template, method, status, duration; not for telemetry or health routes), `auth.login`, `auth.refresh`, `auth.logout`, `ting.enroll`, `ting.send` (type, status, attempt, silent), `speech.token` (purpose, key source, `method` proxy; direct is legacy), `speech.proxy` (relayed speech: `tts_model`, `speak_chars`, `tts_ttfb_ms` or `stt_ms`, `stt_request_id`, key source; never the text, voice instructions or audio), `drawing.put`, `report.created`, `webhook.received` (event type), `participant.op` (action, state) |
| cli | `command.finished` {command, outcome, error_code, exit_code, duration_ms}, once per invocation |
| helper | `daemon.started`, `ipc.request` {op, outcome, duration_ms}, `send.displayed` and `send.queued` {slot, queue_waiting}, `send.shown` {slot}, `send.scheduled` {slot, scheduled}, `schedule.fired` {slot, status: the outcome}, `send.expired` {slot, status: shown or not_shown}, `send.cancelled` {slot, status: where it was}, `queue.cleared` {slot, queue_waiting}, `send.replaced` {slot}, `tts.request` {tts_model, speak_chars, tts_ttfb_ms, duration_ms, status, dg_request_id}, `stt.request` {duration_ms, stt_ms, stt_language, stt_request_id, matched, status}, `delivery.attempt` {type, status, attempt, error_code}, `session.refresh` {outcome}, `app.update` {update_from, update_to, outcome}, `update.cli_watchdog` |
| Peek.app, analytics | `app_launched`, `peek_visible` {slot, mode, visible_ms}, `render_error`, `fallback_visual` {reason}, `glass_mode` {live or frosted}, `display_changed`, `appearance_changed` |
| Peek.app, events | `shortcut_used` {shortcut}, `mic_pressed`, `keyboard_pressed`, `answer_submitted` {ask_type, input, answer_latency_ms}, `dismissed` {gesture}, `settings_changed` {key}, `simulation_run` |
| website | the SDK's automatic analytics (page views, clicks with only the element tag, role and an explicit marker, errors, timings), plus `install_command_copied`, `docs_search` {results} and `download_cta` |

## What is never recorded

- the text of any `--speak`, show element, caption, question, option, answer, message or transcript;
- audio, image bytes, image paths, drawing source;
- SLTs, access or refresh tokens, app secrets, Google/OpenAI API keys or legacy Deepgram keys;
- raw Silicon or Carbon ids, and search queries typed on the website (only the number of results).

Counts and lengths (for example `speak_chars`, `options_count`) are recorded because they explain latency and failures without revealing content.

## Testing contexts

Events from a testing environment are tagged `environment:"testing"` and are never written to the production tables; the gateway drops them or keeps them local, bounded at 1,000. Tests of peek itself inject a fake event sink and never reach Space Station.

## Verify (operators)

With a Space Station session for org `tos`:

```sh
spacestation --org tos query "SELECT record.source::String s, record.event::String e, count() n FROM peekclidaemon GROUP BY s,e ORDER BY n DESC"
spacestation --org tos query "SELECT record.source::String s, record.event::String e, count() n FROM peekbackend GROUP BY s,e ORDER BY n DESC"
spacestation --org tos query "SELECT record.source::String s, record.event::String e, count() n FROM peekfrontendanalytics GROUP BY s,e ORDER BY n DESC"
spacestation --org tos query "SELECT record.source::String s, record.event::String e, count() n FROM peekfrontendevents GROUP BY s,e ORDER BY n DESC"
```

To confirm an opt-out, count the CLI's completion events, run a few commands with `PEEK_TELEMETRY=off` (for example `PEEK_TELEMETRY=off peek iam --json`), wait a minute, and count again. The number must not change:

```sh
spacestation --org tos query "SELECT count() n FROM peekclidaemon WHERE record.event::String = 'command.finished'"
```

## Next

- [Privacy](privacy.md): what data leaves the Mac, apart from telemetry.
