# peek-server

The backend authenticates Carbons and Silicons through Silicon Accounts, relays speech and notifications, stores account-owned drawings and provider settings, accepts reports, and relays telemetry. App secrets and provider credentials remain here.

## Identity and persistence

Every bearer request is introspected against Silicon Accounts. Ownership follows the immutable account UUID, while public handles are display data. Browser sign-in uses state and PKCE; the backend exchanges the code. CLI sign-in exchanges a single-use token.

Refresh replay is stored durably with encryption. Concurrent and repeated requests recover the saved rotation. Transient errors preserve sign-in; expiry or revocation requires a new login. Logout revokes the session family and invalidates its replay entries.

Migration `0003_silicon_accounts.sql` preserves legacy rows while changing ownership columns to account UUIDs. Old credentials cannot silently claim those rows. Back up the database before upgrading and sign in again through Silicon Accounts.

## Routes

| Method and path | Purpose |
|---|---|
| `GET /healthz`, `GET /readyz` | Liveness and configured dependencies |
| `GET /api/v1/accounts` | App discovery |
| `POST /api/v1/auth/login` | Exchange an SLT |
| `POST /api/v1/auth/exchange` | Exchange a browser code with PKCE |
| `POST /api/v1/auth/refresh` | Rotate or recover refresh |
| `POST /api/v1/auth/logout` | Revoke a session |
| `GET /api/v1/auth/me` | Current account |
| `POST /api/v1/ting/recipient` | Enroll notification delivery |
| `POST /api/v1/deliveries` | Deliver a queued event |
| `POST /api/v1/speech/token` | Temporary speech credentials |
| `POST /api/v1/speech/listen` | Transcribe recorded speech |
| `GET/PUT/DELETE /api/v1/drawings/current` | Current account drawing |
| `GET/PUT/DELETE /api/v1/accounts/{account}/byo/deepgram` | Account-owned provider settings |
| `POST /api/v1/reports` | File a report |
| `POST /api/web/telemetry` | Frontend telemetry relay |
| `POST /webhooks/accounts` | Signed account updates |

Ting uses App verification and User verification. An incompatible upstream deployment returns a dependency error and pending deliveries remain retryable.

## Develop and deploy

Copy `.env.example` to `.env` and configure the Accounts app ID, app secret and encryption key. Run `cargo check --workspace --locked` and `cargo run -p silicon-peek --bin peek-server`. Provider and telemetry settings are listed in `.env.example`; deployment and rollback are documented in `deploy/README.md`.
