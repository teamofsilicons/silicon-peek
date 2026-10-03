# IAM and sessions

Peek exchanges IAM short-lived tokens on its backend. It never asks for an IAM password, Silicon STK or direct account token. Every ordinary session selects exactly one Carbon or Silicon account and one organization. The backend verifies every bearer against IAM and requires that same organization and testing environment.

| Identity | Example |
| --- | --- |
| Application | `peek`, owned by `tos` |
| Carbon | `c:alice` |
| Silicon | `si:cleanup` |
| Membership | `si:cleanup[tos]` |
| Organization | `tos` |

All stored work is bound to the verified account, organization and data plane. A local home, profile or command flag cannot grant access to another organization. Keep independent sessions for separate contexts; a context change must not change a pending action's destination.

Ordinary login asks for `self.identity.read`, `self.profile.read` and `self.membership.read`. Identity supports account binding, profile supplies display names, and disclosed membership controls organization settings. Legacy `obo:*` login scopes do not convey feature authority under IAM 5. Multi-organization and unscoped sessions require a fresh IAM sign-in.

`peek login <SLT>` exchanges the code with an idempotency key and stores the ordinary session locally. The pending login and rotating refresh metadata are saved before network I/O, so a lost response can be retried with the original key. A selected organization must match the IAM session; Peek never chooses the first organization from a legacy grant list. Refresh cannot change the original actor, organization or testing world.

`peek login status --json` verifies the account live. An IAM outage is reported separately from a rejected or logged-out session. The CLI and native helper refresh only the relevant saved context. See the [CLI reference](cli.md) for profile and permission commands.

## Ting permission

Login remains usable without Ting. When delivery or recipient enrollment needs Ting, explicitly request the feature's endpoint permissions, review the account and organization in IAM, and complete the manual code flow in Peek. Declining permission preserves the queued action; it does not sign you out. The selected Ting provider account may differ from the Peek account, and Peek uses that approved context for provider resource access.

The backend keeps each root's encrypted OBO access/refresh pair separately from ordinary login credentials. Ting verifies the reusable access token on every request. A pending operation keeps its original provider destination, payload and key through refresh and retry. Reauthorizing another provider account cannot silently move it.

`peek ting enroll` explicitly enrolls the approved provider recipient. Ordinary logout revokes only the selected ordinary session and preserves durable provider consent. `peek logout --revoke-ting` additionally requests removal of the Ting recipient subscription. IAM's grant-management page owns explicit OBO grant revocation; an expired, changed or revoked grant requires a new feature review. Peek never re-enrolls a revoked subscription from the delivery path.

## Testing

Testing credentials select an isolated world. Each mutation carries the current clean generation, and all saved feature authority includes that generation. A failed testing lookup never falls back to production. Clean/purge and signed membership-removal webhooks remove the relevant saved authority and data.

The [IAM 5 migration contract](IAM5_MIGRATION.md) documents the API, encrypted persistence, provider transport, retries and coordinated release requirements. Human-owned `understanding/` notes remain requirements history rather than implementation claims.
