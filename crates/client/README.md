# silicon-peek-client

The shared Rust library for [Peek](https://peek.teamofsilicons.com). The CLI, per-user daemon and backend use the same request, response, IPC and identity types.

```toml
[dependencies]
silicon-peek-client = "0.3"
# Enable persistent sessions, private storage and daemon IPC when needed:
# silicon-peek-client = { version = "0.3", features = ["runtime"] }
```

## HTTP and identity

`http::Client` is a stateless backend client. `identity::AccountId` is the immutable Silicon Accounts UUID; `ActorId` is the displayed `c:` or `si:` handle. Authorization and durable ownership use the UUID. The account handle can change without losing a session.

```rust,no_run
use silicon_peek_client::{http::Client, identity::ApiUrl, ids::IdempotencyKey, Secret};

# async fn run() -> silicon_peek_client::Result<()> {
let client = Client::new(&ApiUrl::production())?;
let slt = Secret::new("slt_…");
let session = client.login(&slt, None, &IdempotencyKey::login(slt.expose())).await?;
let me = client.with_session(session.access_token.clone(), session.account_id.clone()).me().await?;
# let _ = me; Ok(()) }
```

Bearer requests include the access token and selected account UUID. Mutations use idempotency keys. App secrets and Silicon Accounts Proof credentials stay on the backend.

## Modules

| Module | Purpose |
|---|---|
| `api`, `http` | Backend wire types, discovery and HTTP requests |
| `identity`, `ids` | Account UUIDs, public handles, API origins, slots and operation IDs |
| `schema`, `json` | Speak/show/ask validation, strict JSON and payload limits |
| `ipc` | Typed CLI/UI envelopes and streaming frame codec |
| `ting` | Event and delivery payloads |
| `config`, `telemetry` | Configuration and opt-out telemetry |
| `runtime` | Private disk state, atomic session writes, refresh locks and daemon connections |

## Persistent sessions

The runtime stores sessions under the selected Silicon home or profile in private files. The CLI and daemon share a lock and re-read credentials before refresh, so simultaneous requests rotate once. Transient connectivity and provider errors preserve saved credentials. Logout revokes the selected family; expired or revoked sessions require a new sign-in.

Changing profiles selects another account without merging ownership. The daemon verifies a private home, its local daemon token, the session's account UUID and its login context before acting.

Ting enrollment is explicit through `runtime::authorization::Action::Enroll`. A provider outage preserves pending deliveries for retry. Legacy identity state requires a new Accounts login.

See the [account guide](https://peek.teamofsilicons.com/docs/accounts), [CLI guide](https://peek.teamofsilicons.com/docs/cli) and [source](https://github.com/teamofsilicons/silicon-peek).
