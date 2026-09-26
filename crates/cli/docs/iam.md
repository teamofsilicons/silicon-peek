# IAM and sessions

peek signs Silicons in with IAM short-lived tokens only. It never asks for a password or a Silicon token, it stores each Silicon's session in that Silicon's own home, and its backend keeps no IAM tokens at all. This page explains the identities peek uses, the scopes it asks for and why, and how sessions are created, refreshed and revoked.

## Identities

| Kind | Format | How peek uses it |
|---|---|---|
| App | `peek` (owned by org `tos`) | `app_id` everywhere |
| Silicon | `si:<handle>` | owns a position; the recipient of its Ting events |
| Carbon | `c:<handle>` | not used for answers in v1 (see below) |
| Membership | `si:<handle>[<org>]` | `membership_id` in `peek login status` |
| Org | a bare handle such as `tos` | `X-Org-ID`, and `org_id` in Ting bodies |
| ISI | `name` or `name:session-id` | optional context recorded per send; **never** authority |

Everything peek does for a Silicon is keyed by the **verified** actor and org from its session: its position, drawing, asks and deliveries. Never by `SILICON_HOME`, a path, a flag or `ISI`.

The Carbon at the Mac does not log in to peek in v1. Answers carry only the asking Silicon's authority; the person who answered is the person at that Mac.

## Scopes, and why each exists

| Scope | Why peek needs it |
|---|---|
| `self.identity.read` | Ting accepts a proof only when the disclosed identity equals the sending actor. |
| `self.profile.read` | Your display name and initial on the bubble, in Settings, and on the fallback visual. |
| `self.membership.read` | Discloses your org role, so org owners and admins can manage the org's own Deepgram key. |
| `obo:ting:subscriptions.register` | Enroll you as a Ting recipient at login. |
| `obo:ting:tings.send` | Deliver answers, dismissals and messages to you. |
| `obo:ting:subscriptions.revoke` | Remove that enrollment when you log out. |

Scopes are fixed when you consent, and a refresh never adds scopes. peek's v1 scope set is final so that no Silicon is ever forced to log in again for a new scope. `--approve-scopes` (which Stemcell always passes) approves all six. If `self.identity.read`, `obo:ting:subscriptions.register` or `obo:ting:tings.send` is missing, login reports `reconsent_required:true` and deliveries wait in `authority_required`.

## Log in

1. **Mint an SLT** with the IAM CLI. It is single use and lives 2 minutes.
   ```sh
   iam --output json --org <OWNER_ORG> silicon-login --sid si:<handle> --stk <STK> --app-id peek --grant-org <SILICON_ORG> --approve-scopes   # what Stemcell runs
   iam -o json silicon-login --app-id peek --grant-org tos --approve-scopes     # a Silicon, by hand
   iam -o json login --app-id peek --grant-org tos --approve-scopes             # a Carbon's own CLI
   # → {"slt":"…","expires_in":120,…}
   ```
2. **Exchange it.** `peek login "$SLT"` (or `--token-file -`). peek writes a pending-login marker, then calls `POST /api/v1/auth/login` with body exactly `{"slt":"…"}` and an idempotency key derived from the SLT. The backend exchanges it with IAM using peek's app secret, checks the tokens belong to `peek`, picks your org (the `--org`/`SILICON_ORG` hint when you belong to it, otherwise the first alphabetically), and enrolls you with Ting.
3. **Store it.** peek writes the session into `$SILICON_HOME/.peek/session.json` under a lock, with `access_expires_at` counted from when the exchange **started**.
4. **Finish.** It revokes any previous session of this home (best effort), attaches the home to the helper, prints the `login status` shape, and, on a Mac, installs and starts Peek.app in the background.

A lost response is safe: IAM replays the same exchange for 10 minutes, and `peek login --recover` retries it with the same key. After that, mint a new SLT (`login_attempt_expired`).

An SLT that looks like a public id (`si:…` or `c:…`) is refused in production with `slt_is_public_id`; only testing environments accept public ids as SLTs.

## Where the session lives (client custody)

| File | Contents |
|---|---|
| `$SILICON_HOME/.peek/session.json` | your `oat_` access and `ort_` refresh tokens, actor, org, scopes, expiry, Ting enrollment, rejection state |
| `$SILICON_HOME/.peek/session.lock` | an empty file; every read-modify-write, refresh, login and logout holds its exclusive lock |
| `$SILICON_HOME/.peek/daemon-token` | 64 hex characters that prove to the helper that a request comes from this home |
| `$SILICON_HOME/.peek/config.json`, `testing.json` | configuration and saved testing environments |

The directory is `0700`, files are `0600`, symlinks are refused, and every write is atomic (temporary file, sync, rename, directory sync). `SILICON_HOME` unset means your real home (`~/.peek/`); set but empty or missing is an error. peek never touches `$SILICON_HOME/.silicon-iam`.

Why client custody: the Silicon's own home is where Stemcell and every TOS app keep Silicon state, and it means the backend never holds a pile of Silicon tokens. The backend receives your access token with each request and forgets it. The trust boundary is the macOS user account: any process of that user can read `session.json`, as with every TOS app store. The Keychain is not used, because it prompts under Stemcell's non-interactive context.

## Refresh

Access tokens last about 30 minutes. `peek` and the helper refresh with one shared procedure, so many processes on one home never race:

1. If the token is valid for longer than the margin and nothing is pending, use it.
2. Otherwise take `session.lock`, re-read, and if still needed, write a pending refresh key (`peek-refresh-` plus a hash of the refresh token) **before** any network call.
3. Call `POST /api/v1/auth/refresh` with that key and body exactly `{"refresh_token":"…"}`.
4. On success, replace both tokens and clear the pending key. On a transport error, 5xx or 429, keep the key and retry with the same key and body: the helper retries at 1, 2, 5, 10, 30, 60, 120 and 240 s; a CLI command retries after 1 s and 2 s, then leaves the pending key for the next run.
5. IAM replays a refresh for **10 minutes**. If recovery takes longer, or IAM says the refresh token was reused or revoked, the session is marked rejected, and `peek login status` says `reason:"rejected"` with the next command.

Margins: the CLI refreshes when fewer than 60 s remain, the helper 120 s before a delivery and 300 s ahead when the Carbon opens your bubble. An outage of IAM (`iam_misconfigured`, 503) is never treated as a rejection.

## Login status (the Stemcell contract)

`peek login status --json` verifies the session **live** against the backend (`GET /api/v1/auth/me`) and exits 0 whether or not you are authenticated. It prints nothing on stdout and exits 5 only when the backend or IAM cannot be reached, because "logged out" and "cannot tell" must never look the same. The exact shapes are in [CLI reference](cli.md). A `/me` answer of 401 forces one refresh and one retry before it reports `rejected`.

## Log out

`peek logout` writes a logged-out marker, asks the backend to revoke your Ting enrollment and your IAM refresh family, deletes the session, and cancels your undelivered events on this Mac. It exits 0 with `remote_revocation:"confirmed"` or `"pending"`; a pending revocation is retried by the next `peek` run or the helper. The backend's logout answers 204 even for unknown tokens, so it cannot be used to probe tokens.

## Many Silicons on one Mac

- **Each Silicon has its own `SILICON_HOME`**, and so its own session, IAM family and `daemon-token`. Stemcell refuses two connected Silicons on one home.
- **One helper per macOS user** (peekd) serves them all. It learns homes when their CLI attaches, reads each home's session under that home's lock, and never copies tokens into its own database.
- **Every CLI request to the helper is authenticated**: the peer must be the same macOS user, the home must be private and owned by that user, the request's `home_token` must equal the home's `daemon-token`, and the home must hold a valid session for the backend and context in use. Naming another Silicon's home does not let you act as it.
- Two homes holding the same `si:` in the same context share one position.

If a `SILICON_HOME` lives under `~/Documents`, `~/Desktop` or `~/Downloads`, macOS may ask to allow Peek to read it. peek reports this as `authority_required` with the hint to move the home or allow Peek in **Privacy & Security → Files and Folders**.

## How the backend checks a request

Every app route takes `Authorization: Bearer oat_…` and `X-Org-ID: <org>`. The backend introspects the token with IAM **on every request** (no cache) and requires: active, `client_id == "peek"`, `org_id` equal to `X-Org-ID`, a Silicon or Carbon actor with a disclosed public id, and the route's scopes (deliveries need `obo:ting:tings.send` and `self.identity.read`; writing the org's Deepgram key needs an `owner` or `admin` org role). Unknown, expired, revoked or wrong-org tokens get `401 unauthenticated`; a missing scope gets `403 reconsent_required`.

## Availability

peek is registered as a public app, but until its listing is approved only members of org `tos` can install it or log in. Other orgs get `403 private_application_organization_required` (exit 4). See [Peek for Silicons](silicon.md) for the interim path.

## Next

- [Testing environments](testing.md): the same flows against isolated test identities.
- [Privacy](privacy.md): what the backend stores and what it never does.
