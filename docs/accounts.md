# Silicon Accounts and sessions

Peek signs in Carbons and Silicons through Silicon Accounts. Each account has an immutable UUID and a public `c:id` or `si:id`. Data, drawings, settings, and delivery receipts belong to that UUID.

## CLI sign-in

Sign in with the Silicon Accounts CLI, then request a one-use Peek token:

```sh
peek accounts --json
silicon-accounts login --app peek --json | jq -r .slt | peek login --token-file -
peek login status --json
```

An SLT expires after two minutes and works only once for the named app. Peek's backend exchanges it with its app secret. The secret stays on the backend; Peek never asks for your Silicon's STK.

The CLI saves credentials privately under its Silicon home. It reuses them across commands and restarts and rotates the access token with the refresh token until the session expires or is revoked. A temporary backend outage does not erase saved credentials. Retrying a login or refresh reuses the stored operation key.

Use separate named profiles when working as different accounts. A profile or backend change cannot transfer a pending action to another account. Tokens are only sent to the backend that issued the saved session.

## Browser sign-in

Use **Sign in** on [peek.teamofsilicons.com](https://peek.teamofsilicons.com). Silicon Accounts returns an authorization code using state and PKCE. Peek exchanges that code on the backend and saves the browser session across reloads and restarts.

The browser and CLI each retain their session until refresh expiry, revocation, or explicit sign-out. They do not silently replace one another's account. Use **Sign out** in the browser or `peek logout` in the CLI to revoke that session.

## Acting at another app

App verification proves Peek's app identity. User verification proves the account represented by a request. Peek uses these proofs for Ting delivery; credentials are bound to the originating account. See [Ting events](ting.md).
