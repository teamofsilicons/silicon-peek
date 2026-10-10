# Ting events

Everything the Carbon does in response to a Silicon (an answer, a dismissal, a message), and what becomes of the Silicon's sends when it cannot watch (one expired, a scheduled one came due, one appeared), comes back to that Silicon as a Ting event. This page lists the nine event types, their exact payloads, how delivery behaves, how to route them in a Stemcell flow, and how to receive them without Stemcell.

peek never talks to your Silicon directly. peekd records the event on the Mac, the peek backend sends it using your separately approved Ting permission, and Ting delivers it to your Silicon's webhook. Under Stemcell that is `http://<handle>.<org>.localhost/events`, where Stemcell runs your flow. Without Stemcell, you point Ting at a webhook of your own ([Receive answers without Stemcell](#account-identity-and-delivery)).

## The nine types

| Type | Sent when | Default |
|---|---|---|
| `peek.ask.answered` | the Carbon answered an `--ask` (voice, keyboard or click) | always |
| `peek.ask.dismissed` | the Carbon closed an ask without answering (Esc, down-arrow) | always |
| `peek.ask.expired` | an ask with `--expires-in` or `--expires-at` ran out unanswered, on screen or while it waited | always (only asks with a deadline) |
| `peek.message.received` | the Carbon spoke or typed to the Silicon with no pending ask (ctrl+cmd+N, then `\` or typing) | always |
| `peek.speech.finished` | a `--speak` finished or was stopped | only with `--notify speech_finished` or config `notify` |
| `peek.show.dismissed` | the Carbon closed a `--show` before it retracted on its own | only with `--notify show_dismissed` or config `notify` |
| `peek.send.expired` | a `--speak` or `--show` send with `--expires-in` or `--expires-at` ran out before it finished, on screen or before it was ever shown | always (only sends with a deadline) |
| `peek.schedule.due` | a scheduled send (`--in`, `--at`) came due; says whether it was shown, queued, expired or replaced your active bubble | always (only scheduled sends) |
| `peek.send.shown` | a send appeared on the Carbon's screen | always for scheduled sends; otherwise only with `--notify shown` or config `notify` |

Nothing is sent for your own actions: `peek cancel`, `peek ask cancel`, `peek queue clear`, `peek schedule cancel` and `clear`, `peek unregister`, `peek logout`, and a send you took over with `--replace`. `peek.speech.finished`, `peek.show.dismissed` and `peek.send.shown` are opt-in because a catch-all flow branch (like Flow A) forwards every one of them as a message, and each event requires a Ting request and authorization check.

An expiry is always reported with exactly one type: `peek.ask.expired` for an ask, `peek.send.expired` for anything else. A scheduled send whose deadline passed before it came due gets both `peek.schedule.due` (outcome `expired`) and that expiry event.

The type names are final: Ting types cannot be deleted, so these nine will not be renamed.

## Payloads

Every `data` object carries `"schema":1`, `"slot"` (1–8) and `"context"` (`"production"`). Timestamps are RFC 3339 UTC with `Z`. Images are referenced by option id and label, never as bytes. Fields are only ever **added** within schema 1, so ignore fields you do not know.

```jsonc
// peek.ask.answered
{"schema":1,"ask_id":"ask_0192…","send_id":"snd_0192…","question":"Delete ~/Downloads/old.zip?","ask_type":"single_choice",
 "answer": <one of the shapes below>,
 "via":"voice|keyboard|click","transcript":"no keep it"|null,   // the final transcript, only when via is voice
 "asked_at":"2026-09-26T10:00:00Z","answered_at":"2026-09-26T10:00:07Z","slot":3,"context":"production"}
//   text:            {"kind":"text","text":"…"}
//   single_choice:   {"kind":"single_choice","option_id":"keep","label":"Keep it"}
//   multiple_choice: {"kind":"multiple_choice","option_ids":["a","c"],"labels":["Alpha","Charlie"]}
//   slider:          {"kind":"slider","value":42}
//   range:           {"kind":"range","from":10,"to":30}

// peek.ask.dismissed
{"schema":1,"ask_id","send_id","question","ask_type","gesture":"esc|down_arrow|down_arrow_double","asked_at","dismissed_at","slot","context"}

// peek.ask.expired
{"schema":1,"ask_id","send_id","question","ask_type","asked_at","expired_at","slot","context",
 "shown":true|false}                      // was it ever on screen (added in 0.1.2; absent from older helpers)

// peek.message.received
{"schema":1,"message_id":"cmsg_0192…","text":"remind me about this at 5","via":"voice|keyboard","sent_at","slot","context",
 "in_reply_to":"snd_…"|null}              // your most recent send in that position, if any

// peek.speech.finished
{"schema":1,"send_id","stopped_by_user":false,"played_ms":4210,"total_ms":4210,"finished_at","slot","context"}

// peek.show.dismissed
{"schema":1,"send_id","gesture":"down_arrow|down_arrow_double|esc|esc_double","visible_ms":2300,"dismissed_at","slot","context"}

// peek.send.expired (never for an ask; asks use peek.ask.expired)
{"schema":1,"send_id":"snd_0192…","kind":"speak|show|speak+show",
 "created_at":"2026-09-27T10:00:00.000Z",  // when peek send ran
 "expires_at":"2026-09-27T10:10:00.000Z",  // the deadline you set
 "expired_at":"2026-09-27T10:10:00.004Z",  // when peek expired it: at or after expires_at (later after sleep)
 "shown":false,"shown_at":null,            // shown_at is set exactly when shown is true
 "scheduled":false,"schedule_id":null,     // schedule_id is set exactly when it came from --in/--at
 "slot":3,"context":"production"}

// peek.schedule.due (once, when the scheduled send fires: at its due time, or on catch-up after sleep)
{"schema":1,"schedule_id":"sch_0192…","send_id":"snd_0192…","ask_id":null,"kind":"show",
 "due_at":"2026-09-27T12:30:00.000Z","fired_at":"2026-09-27T12:30:00.012Z",
 "outcome":"queued",                       // shown | queued | expired | replaced
 "replaced_send_id":null,                  // set exactly when outcome is replaced
 "waiting_reason":"behind_others",         // behind_others | queue_full | carbon_away | paused | app_not_running
 "queue_position":2,                       // set exactly when outcome is queued
 "expires_at":null,"slot":3,"context":"production"}

// peek.send.shown
{"schema":1,"send_id":"snd_0192…","ask_id":null,"kind":"speak+show",
 "created_at":"2026-09-27T12:00:00.000Z","shown_at":"2026-09-27T12:30:00.412Z",
 "scheduled":true,"schedule_id":"sch_0192…","slot":3,"context":"production"}
```

Notes on the three types added in 0.1.2:

- **`kind`** is `speak`, `show`, `ask`, `speak+show` or `speak+ask`. `peek.send.expired` never describes an ask, so its `kind` is `speak`, `show` or `speak+show`.
- **`peek.schedule.due` `outcome`:**
  - `shown`: it became your active bubble and went to the screen at once (`peek.send.shown` confirms when it actually appeared);
  - `queued`: it waits; `waiting_reason` says why: `behind_others` (your earlier sends, `queue_position` 1–5), `queue_full` (five were already waiting, so it takes the next free spot, `queue_position` 6 or more), or it is your active bubble but held (`queue_position` 0) by `carbon_away`, `paused` or `app_not_running`;
  - `expired`: its `--expires-at` had passed before it could fire; the expiry event (`peek.ask.expired` or `peek.send.expired`, `"shown":false`) follows;
  - `replaced`: it was a `--replace` send and took over the send in `replaced_send_id`; `waiting_reason` is set too when the new bubble is held.
- **`peek.send.shown`** is sent when Peek.app starts presenting the bubble, at most once per send. A scheduled send always sends it; a regular send only with `--notify shown`.
- **Esc gestures.** A show closed with one Esc reports `esc`; two Escs within 0.4 s (which also stop the speech) report `esc_double`. An ask dismissed by Esc always reports `esc`.

`metadata` is `{"isi":"<ISI>","peek_version":"0.1.2"}`. `isi` is the `$ISI` of the process that ran `peek send`, and is **omitted** when it was unknown or invalid (an invalid `ISI` is dropped with an `isi_ignored` warning; the send still goes out). For `peek.message.received` it is the ISI of your most recent send in that position. For a scheduled send it is the ISI that ran `peek send --in` or `--at`. Route on it (see below); never treat it as authority.

Ids: `ask_`, `snd_`, `sch_` (a scheduled send), `cmsg_` and `evt_`, each followed by 32 hex characters (a UUIDv7, so they sort by time).

## What your webhook receives

```json
{"tings":[{"id":"msg_…","created_at":"2026-09-26T10:00:07Z","type":"peek.ask.answered","data":{…},
  "metadata":{"isi":"deliberate","peek_version":"0.1.2"},"key":"si:dj/ask_0192…/answered"}]}
```

The `key` is `"<your actor id>/<subject>/<event>"`. It is unique per semantic event, so Ting drops duplicates of the same event:

| Type | Subject | Event |
|---|---|---|
| `peek.ask.answered`, `peek.ask.dismissed`, `peek.ask.expired` | `ask_id` | `answered`, `dismissed`, `expired` |
| `peek.message.received` | `message_id` | `message` |
| `peek.speech.finished`, `peek.show.dismissed` | `send_id` | `speech_finished`, `show_dismissed` |
| `peek.send.expired` | `send_id` | `send_expired` |
| `peek.schedule.due` | `schedule_id` | `due` |
| `peek.send.shown` | `send_id` | `shown` |

## Delivery guarantees

- **At least once.** Ting and Stemcell may deliver or replay a batch more than once. Dedupe by the ting `id`; the data also carries `ask_id` or `message_id`.
- **Durable on the Mac.** peekd writes each event to an outbox before trying to deliver it. If the Mac is offline, or the backend or Ting is down, it retries with backoff (1 s, 2 s, 5 s, 15 s, 30 s, 1 min, 2 min, 5 min, 10 min, then every 15 min) until `delivery_max_age_hours` (default 168, at most one week) has passed. Then the event is marked expired.
- **Sent with your own session.** Only the answering Silicon's own peek session can deliver its events, and events of one Silicon are never sent with another's. If your session is rejected, events wait in `authority_required` until you log in again.
- **Exactly one channel with `--wait`.** When a live `peek send --wait` prints the outcome of its ask (`answered`, `dismissed`, `expired` or `cancelled`), that outcome is not also sent through Ting, and `peek ask get` shows `delivery.status:"wait"`. If the wait timed out or the process died first, the event goes through Ting as usual.
- **Nothing is sent to nobody.** While the Carbon's screen is locked or the display is asleep, Peek.app shows nothing: new sends stay queued (`status:"queued"` with a `carbon_away` warning), deadlines keep running, and speech starts only when the bubble is actually shown. `peek.speech.finished`, `peek.show.dismissed` and `peek.send.shown` therefore always describe something the Carbon could see or hear, and the expiry events say with `shown` whether the Carbon ever saw the send.

Check delivery at any time:

```sh
peek ask get ask_0192… --json     # "delivery":{"status":"accepted","ting_id":"msg_…","silent":false}
peek status --json                # "deliveries":{"pending":0,"authority_required":0,"last_error":null}
```

| Delivery status | Meaning | What to do |
|---|---|---|
| `pending` | queued or retrying | nothing; see `last_error` in `peek status` |
| `accepted` | Ting accepted it | – |
| `accepted` with `silent:true` | you muted peek in Ting, so it was stored but never delivered | unmute peek in Ting |
| `authority_required` | your session was rejected, consent is missing, or you are not enrolled | `peek login status --json` shows which; see below |
| `expired` | not delivered within `delivery_max_age_hours` | check why deliveries fail (`peek doctor`) |
| `cancelled` | you logged out before it was delivered | – |
| `failed` | Ting refused it permanently (a bug) | `peek report` with the `ask get` output |

`delivery:"required"` (Ting's "never silent" mode) is not used, because it needs an opt-in from you that Stemcell never performs.


## Account identity and delivery

Peek authenticates with Silicon Accounts and uses App verification and User verification for Ting requests. A reply belongs to the Carbon or Silicon account that originated it, identified by immutable UUID. Switching the active profile never changes a queued reply's destination.

Use `peek ting --help` for enrollment and delivery commands. Temporary delivery failures stay queued for retry. The backend keeps receipts and payload hashes rather than voice recordings or message content.
