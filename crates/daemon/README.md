# silicon-peek-daemon (`peekd`)

The per-user Peek daemon. One `peekd` runs per macOS account. It ships inside
`Peek.app/Contents/Helpers/peekd`, and Peek.app registers it as the launchd
agent `ai.tos.peek.daemon`. It builds on macOS only; other platforms get a
`compile_error!`.

```
peekd run [--launchd | --parent-ui | --headless]
```

peekd owns:

- the slot registry;
- the send queue (always queue, strict FIFO: one bubble on screen and at most
  five waiting per Silicon; `--replace` takes over), expiry of every send,
  one-time scheduled sends (`--in`/`--at`, fired on wall-clock time, caught up
  after sleep or downtime), asks and history;
- the delivery outbox;
- every Deepgram call: Aura-2 TTS, streamed to the UI, and Nova-3 STT, once
  per recording;
- per-home session refresh, run under each home's `session.lock`;
- the telemetry relay;
- applying locally bundled Peek.app updates; Silicon Apps owns network updates.

Peek.app holds no tokens and does no networking. The CLI talks only to peekd,
and to peek-server for login.

## Socket and roles

`/var/tmp/silicon-peek-<uid>/peekd.sock`

- The directory is 0700 and the socket 0600.
- A single instance holds `peekd.lock` with `flock(LOCK_EX|LOCK_NB)`; a second
  peekd exits with `daemon_running` (exit 4).
- Each end checks the other's uid with `getpeereid`.
- IPC protocol v1 comes from `silicon-peek-client::ipc`.

**`role: "cli"`**

- One request per connection. `send --wait` is the exception: the connection
  stays open until `ask.result`.
- Every op except `hello` and `daemon.status` carries the home's `auth`
  block. peekd checks, in order:
  1. the home is a private directory;
  2. its `daemon-token`;
  3. an unrejected slot for `api_url#context`.
- Identity comes from that slot, never from request fields.

**`role: "ui"`**

- One long-lived link.
- The peer's executable, found through `LOCAL_PEERPID` → `proc_pidpath`, must
  be the `Contents/MacOS/Peek` of the bundle peekd runs from.

## State

`~/Library/Application Support/Peek/` (dir 0700; `~` is the getpwuid home):

| Path | What |
|---|---|
| `peekd.sqlite` | homes, slots, drawings, sends, asks, scheduled sends, outbox, telemetry outbox (account-scoped schema). All times are unix ms. No tokens. |
| `settings.json` | UI and daemon settings, mirrored from `settings.changed` |
| `drawings/<context>/<account>/<sha256>.js` | the active drawing, plus the previous one for rollback |
| `cache/images/`, `cache/tts/` | image copies and the TTS cache (LRU, 200 MB / 30 days) |
| `recordings/` | voice recordings, kept until the answer is delivered |
| `offers/`, `rejected.json`, `install.lock`, `update-applied.json` | local app update offers |
| `homes.json` | registered CLI homes |
| `peekd.log` | rotated at 10 MB × 3 |

## Delivery

- Every event the Silicon should hear about becomes an outbox row: answered,
  dismissed, expired, a Carbon message, and opt-in speech-finished or
  show-dismissed.
- The row's request bytes are fixed when it is created and POSTed unchanged to
  `/api/v1/deliveries` with `Idempotency-Key: peek-delivery-<event_id>`.
- Backoff: 1 s, 2 s, 5 s, 15 s, 30 s, 1 m, 2 m, 5 m, 10 m, then every 15 m,
  until the home's `delivery_max_age_hours`.
- `recipient_not_registered`, `reconsent_required` and a rejected session park
  the row in `authority_required`. A new `peek login` sends `attach`, which
  drains it.
- peekd never re-registers a Ting recipient.

## Development

```sh
cargo fmt -p silicon-peek-daemon --check
cargo check -p silicon-peek-daemon --locked
```

`PEEK_DAEMON_SOCKET`, `PEEK_SUPPORT_DIR`, `PEEK_APPLICATIONS_DIR`, `PEEK_UI_EXECUTABLE` and `PEEK_API_URL` select local development paths. `PEEKD_LOG` controls the log filter.
