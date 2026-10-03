# IAM 5 migration candidate

This candidate follows the [IAM 5 migration guide](https://docs.iam.teamofsilicons.com/migrating-to-iam-5/) and vendors the reviewed SDK at `f1e9c4768029aacabe337ca41be52e05023d1631`. It requires the coordinated IAM 5 and Ting reusable-token receiver rollout. Local implementation does not establish that those production services have switched.

Ordinary application login selects exactly one actor and organization. Peek refuses legacy multi-organization, unscoped, mismatched-organization and OBO-scoped login credentials. The application secret stays in peek-server; ordinary access and refresh credentials remain in the caller's local session store. Login never enrolls a Ting recipient or starts endpoint consent.

## Explicit Ting permission

The selected Peek account starts the feature using its ordinary bearer, `X-Org-ID`, and current testing headers when applicable:

| Operation | Request |
| --- | --- |
| Start | `POST /api/v1/ting/authorization`, `{}`, `Idempotency-Key` |
| Status | `GET /api/v1/ting/authorizations/{request_id}` |
| Complete | `POST /api/v1/ting/authorizations/{request_id}/complete`, `{"code":"…"}`, `Idempotency-Key` |

Each response contains a local `request_id`, IAM's `authorization` detail, `completed`, and safe `roots` summaries. `authorization.id` is the corresponding IAM authorization ID. The manual flow opens IAM's returned HTTPS `authorization_url`; only the represented user approves there and pastes the one-use code back. No callback is registered by this flow. Code completion validates the initiating actor, organization, local request, IAM request and current testing world. Request IDs from another account or organization are unavailable.

The feature requests `ting` roots `subscriptions.register`, `subscriptions.revoke`, and `tings.send`, already declared in `application.json.example`. The initial request and code exchange retain stable upstream idempotency keys. An uncertain code response can be recovered when IAM reports the authorization as exchanged. A repeated completion with different code is refused. Decline and graph-change responses leave ordinary login and queued actions intact. A graph change requires a fresh review.

Each approved root's access/refresh pair is encrypted with `PEEK_ENCRYPTION_KEY`. Associated data binds the ciphertext to the world, clean generation, initiating account, organization and endpoint. SQLite FULL durability commits the pending refresh identity before the upstream request, then atomically stores the replacement pair. A database lease serializes the account's exchanges across server processes; uncertain rotations reuse the saved key and refresh token. Logout does not erase provider credentials. IAM membership-removal webhooks and testing clean/purge remove the relevant saved authority.

## Provider calls and retries

Peek sends the reusable root access token to Ting as `Authorization: Bearer oba_…`. Ting verifies its exact registered endpoint, method and path on every operation. Peek never calls the retired proof exchange and never forwards a refresh token. Testing provider credentials come only from IAM's token response and must resolve to the same isolated environment; production never accepts a testing token.

Ting's approved provider account and organization determine the recipient and outbound body. They may differ from the initiating Peek account. The three roots must agree on that provider context. Before sending, Peek records each operation's original payload hash and provider destination. A later authorization for another provider account cannot redirect a pending delivery; restore the original provider permission or create a new action. One rejected access token triggers a dedicated-family refresh and one retry with identical operation bytes. Rejected or revoked feature permission returns `403 reconsent_required` with `details.feature: "ting"`, without invalidating ordinary Peek login.

## Persistence and release

Schema 2 adds `obo_requests`, `obo_roots`, `obo_operations`, and `obo_locks`. Existing drawings, delivery receipts, enrollment records and ordinary client sessions are preserved. Existing enrollment does not confer IAM 5 authority. The native helper must finish the explicit permission flow before it can resume deliveries.

Keep this candidate private until IAM and Ting's matching receiver are available, then exercise isolated production/testing flows for both Carbon and Silicon accounts. Publish backend, client, CLI and native changes together through the normal Honeycomb review workflow. Physical native installation, external approvals, live consent and provider delivery remain separate validation steps.

## Local validation (2026-10-03)

Backend: `cargo test -p silicon-peek --no-fail-fast` passes 131 tests; the existing live OpenAI test remains opt-in and ignored. The nine explicit feature tests exercise separate permission, owner isolation, encrypted storage, decline, graph changes, lost code responses, token rotation after restart, changed destination refusal, cross-process serialization and testing-clean removal. `cargo clippy -p silicon-peek --all-targets -- -D warnings` passes. Client/CLI/daemon validation and native tests are recorded in their accompanying commits. No deployed IAM/Ting consent or physical native installation was performed by this local migration.
