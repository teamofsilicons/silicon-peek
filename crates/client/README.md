# silicon-peek-client

The shared Rust library of [Peek](https://peek.teamofsilicons.com). It is
linked into the `peek` CLI, the per-user daemon `peekd` and the backend
`peek-server`, so all three agree on every byte they exchange.

```toml
[dependencies]
silicon-peek-client = "0.1"                                     # stateless
silicon-peek-client = { version = "0.1", features = ["runtime"] } # + store, refresh, IPC
```

`#![forbid(unsafe_code)]`. The few OS calls it needs (`getuid`, `getpwuid_r`,
`getpeereid`) go through `nix` and tokio's safe wrappers.

## Stateless by default

| Module | What it holds |
|---|---|
| `error` | `Error` (code, message, hint, retryable, request_id, details), the full `ErrorCode` enum, `ExitCode` and the code → exit mapping (0 ok · 1 internal · 2 usage · 3 not authenticated · 4 refused · 5 unavailable). `Error::envelope()` is exactly what `--json` prints on stderr. |
| `json` | Strict JSON: duplicate keys are refused at every depth (`parse_value`, `parse_object`, `from_slice`). |
| `identity` | `ActorId` (`si:`/`c:`), `OrgId`, `Context` (`production` or a testing UUID), `ApiUrl` (HTTPS origin; HTTP only for loopback), `SlotKey` (`"<api_url>#<context>"`), `TestingSecret`, `SlotIndex` 1–8 and `Side`, `resolve_org` (`--org` → `SILICON_ORG` → slot). |
| `ids` | `AskId`/`SendId`/`MessageId`/`EventId` (`ask_`/`snd_`/`cmsg_`/`evt_` + UUIDv7 simple), `IdempotencyKey` (validated, plus the deterministic `login`/`refresh`/`revoke`/`delivery` derivations) and `ting_key`. |
| `schema` | `--show` (`Show::from_input`), `--ask` (`Ask::from_input`, `Ask::resolve_answer`), send options (`schema::send`), image sniffing and every limit in `schema::limits`. Counts are Unicode scalar values. |
| `ting` | The six `peek.*` payloads, `TingMetadata`, `DeliveryRequest` (the body peekd posts; `validate_for` is the server's check) and `ting_send_body` (the deterministic `/v1/tings` body). |
| `api` | Request/response types for every peek-server route, route and header names, the Honeycomb participant contract and the static `peek iam --json` document (`IamInfo`). |
| `http` | `Client`: a stateless peek-server client. |
| `ipc` | Protocol v1 envelopes (`Request`, `Reply`, `Event`, `Message`), the typed CLI ops (`ipc::cli`), UI ops and events (`ipc::ui`) and the framing codec (`ipc::frame`). |
| `config` | `config.json` and the strict merge behind `peek config set`. |
| `telemetry` | Opt-out environment, actor hashing (`sha256("<org>:<actor>")[..16]`) and the context allowlist. |

### HTTP client

```rust,no_run
use silicon_peek_client::{http::Client, identity::{ApiUrl, OrgId}, ids::IdempotencyKey, Secret};

# async fn run() -> silicon_peek_client::Result<()> {
let client = Client::builder(&ApiUrl::production()).component("peek-cli/0.1.0").build()?;
let slt = Secret::new("oac_…");
let session = client.login(&slt, None, &IdempotencyKey::login(slt.expose())).await?;
let me = client
    .with_session(session.access_token.clone(), session.org_id.clone())
    .me()
    .await?;
# let _ = me; Ok(()) }
```

Per request the client sends only the headers of the CLI/peekd → peek-server
hop: `Authorization` + `X-Org-ID` on bearer routes, `Idempotency-Key` on every
POST, `X-Testing-Environment-Key` on every `/api/` route in a testing context
(`with_testing`), `X-Testing-Environment-Generation` on every mutation except
login and refresh, `X-Peek-Telemetry: on|off` and `Peek-Client-Version`. It
never retries on its own. Non-2xx responses decode into `Error` from the
server's `{"error":{…}}` (falling back to `X-Request-ID` and `Retry-After`
headers); transport failures are `backend_unavailable`, retryable, with
`Origin::Transport`.

### IPC framing

One JSON object per line (≤ 1 MiB, duplicate keys refused). If it has
`"bin":[n1,…]`, exactly `n1+…` raw bytes follow the newline: the frame's blobs.
Each blob ≤ 10 MiB, ≤ 40 MiB per frame; per-op limits come from
`frame::blob_limit`. `Decoder` is a pure state machine (any split of bytes);
`FrameReader`/`write_frame` wrap it for `std::io`, `AsyncFrameReader`/
`write_frame_async` for tokio.

```rust
use silicon_peek_client::ipc::{Message, Request, cli::RegisterSide};
use silicon_peek_client::identity::SlotIndex;

let req = Request::new(&RegisterSide { index: SlotIndex::new(5)? }, None, vec![])?;
let frame = Message::Request(req).into_frame()?; // → {"v":1,"id":…,"op":"register.side","index":5}
# Ok::<(), silicon_peek_client::Error>(())
```

There is no live transcript type anywhere: speech-to-text arrives once, as
`ipc::ui::SttResult`.

## The `runtime` feature

| Item | Purpose |
|---|---|
| `runtime::store::Store` | `${SILICON_HOME:-<passwd home>}/.peek`: directory 0700, files 0600 created with `O_EXCL`, symlinks and foreign owners refused, atomic writes (temp + `sync_all` + `rename` + directory fsync), `session.lock` (`flock`), `session.json`/`config.json`/`testing.json` with a schema guard (`store_schema_newer`), `daemon-token`, and the `config home` pointer. `$SILICON_HOME/.silicon-iam` is never touched. |
| `runtime::fresh_session(store, client, context, margin)` | The one refresh path (BLUEPRINT §2.5): lock-free fast path; exclusive lock and re-read; `pending_refresh_key = "peek-refresh-" + hex(blake3(refresh_token))` persisted before any I/O; exact body; expiry measured from `refresh_started_at`; retries with the same key at 1, 2, 5, 10, 30, 60, 120, 240 s within 10 minutes; `401`/`409 idempotency_response_expired` mark the slot `rejected` (terminal); `503 iam_misconfigured` is an outage. Margins: `CLI_MARGIN` 60 s, `DELIVERY_MARGIN` 120 s, `PREWARM_MARGIN` 300 s. `fresh_session_with` takes a `RefreshPolicy`; `force_refresh` handles a 401 from `/me`. |
| `runtime::login` | `login` (pending_login, in-process retries 1/3/9 s with the same key, commit, `daemon-token`), `recover_login` (≤ 10 minutes), `logout` (tombstone, revoke with the bearer, delete the slot; never claims a revocation it does not have) and `revoke_pending`. |
| `runtime::authenticate_home` | peekd's three checks on a CLI `auth` block: private directory, constant-time `daemon-token`, unrejected slot. Identity comes from the slot. `auth_block` builds the block a CLI sends. |
| `runtime::daemon` | `socket_path()` (`/var/tmp/silicon-peek-<uid>/peekd.sock` or `PEEK_DAEMON_SOCKET`), `DaemonConnection` (peer-uid check, `hello`, `call` with timeouts, `next_event` for `send --wait`), `verify_peer` and `ensure_socket_dir` for peekd. |

```rust,no_run
use std::time::Duration;
use silicon_peek_client::{http::Client, identity::{ApiUrl, Context}, runtime::{self, Store}};

# async fn run() -> silicon_peek_client::Result<()> {
let store = Store::from_env()?;
let client = Client::new(&ApiUrl::production())?;
let slot = runtime::fresh_session(&store, &client, Context::Production, runtime::CLI_MARGIN).await?;
let authed = client.with_session(slot.access_token.clone(), slot.org_id.clone());
# let _ = authed; Ok(()) }
```

## Tests

```sh
CARGO_TARGET_DIR=../../target cargo test -p silicon-peek-client --features runtime
```

Unit tests cover every validator boundary, serde round trips and strict
parsing, framing (split reads, blobs, oversize, truncation) and store
permissions. Integration tests run the HTTP client, `fresh_session` (success,
5xx retried with the same key, 401 terminal, concurrent refresh → one
rotation), login/logout and home authentication against `wiremock` and temp
homes, and the framing over real Unix socket pairs. No test touches the real
home directory or any remote service.
