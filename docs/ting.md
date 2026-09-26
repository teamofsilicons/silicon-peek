# Ting events

Everything the Carbon does in response to a Silicon (an answer, a dismissal, a message) comes back to that Silicon as a Ting event. This page lists the six event types, their exact payloads, how delivery behaves, and how to route them in a Stemcell flow.

peek never talks to your Silicon directly. peekd records the event on the Mac, the peek backend proves it to Ting with your own session, and Ting delivers it to your Silicon's local webhook (`http://<handle>.<org>.localhost/events`), where Stemcell runs your flow.

## The six types

| Type | Sent when | Default |
|---|---|---|
| `peek.ask.answered` | the Carbon answered an `--ask` (voice, keyboard or click) | always |
| `peek.ask.dismissed` | the Carbon closed an ask without answering (Esc, down-arrow) | always |
| `peek.ask.expired` | an ask with `--expires-in` ran out unanswered | always (only asks that set the flag) |
| `peek.message.received` | the Carbon spoke or typed to the Silicon with no pending ask (ctrl+cmd+N, then `\` or typing) | always |
| `peek.speech.finished` | a `--speak` finished or was stopped | only with `--notify speech_finished` or config `notify` |
| `peek.show.dismissed` | the Carbon closed a `--show` before it retracted on its own | only with `--notify show_dismissed` or config `notify` |

Nothing is sent for your own actions: `peek ask cancel`, `peek unregister`, `peek logout` and a show replaced by your next send. The last two types are opt-in because Stemcell's default flow forwards every unknown type to `intuit` as a message, and each event costs an IAM proof and a Ting verification.

The type names are final: Ting types cannot be deleted, so these six will not be renamed.

## Payloads

Every `data` object carries `"schema":1`, `"slot"` (1–8) and `"context"` (`"production"` or `"testing"`). Timestamps are RFC 3339 UTC with `Z`. Images are referenced by option id and label, never as bytes. Fields are only ever **added** within schema 1, so ignore fields you do not know.

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
{"schema":1,"ask_id","send_id","question","ask_type","asked_at","expired_at","slot","context"}

// peek.message.received
{"schema":1,"message_id":"cmsg_0192…","text":"remind me about this at 5","via":"voice|keyboard","sent_at","slot","context",
 "in_reply_to":"snd_…"|null}              // your most recent send in that position, if any

// peek.speech.finished
{"schema":1,"send_id","stopped_by_user":false,"played_ms":4210,"total_ms":4210,"finished_at","slot","context"}

// peek.show.dismissed
{"schema":1,"send_id","gesture":"down_arrow|down_arrow_double|esc","visible_ms":2300,"dismissed_at","slot","context"}
```

`metadata` is `{"isi":"<ISI>","peek_version":"0.1.0"}`. `isi` is the `$ISI` of the process that ran `peek send`, and is **omitted** when it was unknown. For `peek.message.received` it is the ISI of your most recent send in that position. Route on it (see below); never treat it as authority.

Ids: `ask_`, `snd_`, `cmsg_` and `evt_`, each followed by 32 hex characters (a UUIDv7, so they sort by time).

## What your webhook receives

```json
{"tings":[{"id":"msg_…","created_at":"2026-09-26T10:00:07Z","type":"peek.ask.answered","data":{…},
  "metadata":{"isi":"deliberate","peek_version":"0.1.0"},"key":"si:dj/ask_0192…/answered"}]}
```

The `key` is `"<your actor id>/<ask_id | send_id | message_id>/<answered | dismissed | expired | message | speech_finished | show_dismissed>"`. It is unique per semantic event, so Ting drops duplicates of the same event.

## Delivery guarantees

- **At least once.** Ting and Stemcell may deliver or replay a batch more than once. Dedupe by the ting `id`; the data also carries `ask_id` or `message_id`.
- **Durable on the Mac.** peekd writes each event to an outbox before trying to deliver it. If the Mac is offline, or the backend or Ting is down, it retries with backoff (1 s, 2 s, 5 s, 15 s, 30 s, 1 min, 2 min, 5 min, 10 min, then every 15 min) until `delivery_max_age_hours` (default 168, at most one week) has passed. Then the event is marked expired.
- **Sent with your own session.** Only the answering Silicon's own peek session can deliver its events, and events of one Silicon are never sent with another's. If your session is rejected, events wait in `authority_required` until you log in again.
- **Exactly one channel with `--wait`.** An answer printed to a live `peek send --wait` is not also sent through Ting.

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

## Enrollment

To receive peek events, your Silicon must be a Ting recipient for peek. `peek login` enrolls you. If that failed transiently, the login still succeeds with `"ting":{"subscribed":false,"error":{…}}`, and you can retry:

```sh
peek ting enroll --json    # {"subscribed":true,"subscription_id":"sub_…"}
```

peek **never** re-enrolls you automatically after a delivery fails with `recipient_not_registered`. Re-enrolling would reactivate a grant you may have revoked on purpose. Instead the event waits in `authority_required`, and `peek login status` shows `"ting":{"subscribed":false}` with `next:"peek ting enroll"`. `peek logout` removes the grant.

## Route events in your flow

Without a flow branch, peek events reach `intuit` through Stemcell's default template. Two verified snippets are in [Peek for Silicons](silicon.md):

- **Flow A** sends every event whose type starts with `peek.` to one ISI.
- **Flow B** routes each event to the ISI in `metadata.isi`, including session-mode addresses such as `worker.terminal:build-17`, and falls back to `intuit`. It never pastes ting data into generated expressions, so it is safe against CEL injection.

Test a branch locally by posting a fake batch to your Silicon:

```sh
curl --fail-with-body -H 'Content-Type: application/json' --data '{"tings":[{"id":"local-1","type":"peek.ask.answered","data":{"ask_id":"a1","answer":{"kind":"text","text":"yes"}},"metadata":{"isi":"intuit"}}]}' http://<handle>.<org>.localhost/events
```

## How peek sends (for builders)

| Step | Who | Credential |
|---|---|---|
| The event is recorded | peekd, when Peek.app reports an answer, dismissal, message, finished speech or dismissed show, or when an ask expires | none; it writes an outbox row with the exact request bytes |
| Delivery request | peekd: refresh your session if it expires within 120 s, then `POST /api/v1/deliveries` | your `oat_` access token |
| Proof and send | peek backend: one IAM on-behalf-of proof per attempt for `tings.send`, whose subject is your own token, then `POST /v1/tings` to Ting | peek's app secret signs the exchange |
| Receipt | Ting → your ting-daemon → Stemcell → your flow | – |

The backend builds the Ting body only from your verified identity and the request fields, hashes it into the proof, and sends exactly those bytes. `for` is always you: an event can only ever be delivered to the Silicon whose session sent it. The backend records a delivery receipt (event id, type, ting key, ting id, status); it does not store the event data. See [IAM and sessions](iam.md) and [Privacy](privacy.md).

Registering the types is an operator step, done once per data context (production and every testing environment) with byte-identical descriptions:

```sh
ting --org tos types register --type 'peek.ask.answered'     --description 'A Carbon answered a peek --ask (voice, keyboard or click).' --json
ting --org tos types register --type 'peek.ask.dismissed'    --description 'A Carbon dismissed a peek --ask without answering.' --json
ting --org tos types register --type 'peek.ask.expired'      --description 'A peek --ask reached its --expires-in deadline unanswered.' --json
ting --org tos types register --type 'peek.message.received' --description 'A Carbon spoke or typed to the Silicon from its peek with no pending ask.' --json
ting --org tos types register --type 'peek.speech.finished'  --description 'A peek --speak finished playing or was stopped by the Carbon.' --json
ting --org tos types register --type 'peek.show.dismissed'   --description 'A Carbon closed a peek --show before it retracted.' --json
ting --org tos types list --app peek --json
```
