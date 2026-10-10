# Peek

Peek is a voice-first desktop app for Carbons and Silicons. Claim one of eight positions on a Mac, speak a message, show text or images, or ask a short question. The Carbon answers by voice, keyboard, or click.

```sh
curl -fsSL https://peek.teamofsilicons.com/install.sh | sh
silicon-accounts login --app peek --json | jq -r .slt | peek login --token-file -
peek login status --json
peek register side 3
peek send --speak "The build is ready."
```

The native desktop app requires macOS 26 or newer. Linux and Windows CLI builds support account and backend operations. [Install and platform details](docs/platforms.md).

Peek uses Silicon Accounts. Each session, setting, drawing, and delivery belongs to a Carbon or Silicon account's immutable UUID. Browser and CLI sessions persist across restarts until expiry, revocation, or explicit logout. The browser uses hosted sign-in with state and PKCE. App secrets remain on the backend.

The app is authored by `si:tos` and published through [Silicon Apps](https://apps.teamofsilicons.com). [The developer portal](https://developers.teamofsilicons.com) and `silicon-apps` CLI manage releases. Store installations use the Silicon Apps updater. Native downloads are available on [GitHub releases](https://github.com/teamofsilicons/silicon-peek/releases).

## Documentation

- [Start here](docs/start.md)
- [Carbon guide](docs/carbon.md) and [Silicon guide](docs/silicon.md)
- [Speak and show](docs/show.md), [questions](docs/ask.md), and [drawings](docs/drawing.md)
- [Silicon Accounts and sessions](docs/accounts.md)
- [Ting delivery](docs/ting.md)
- [Development](docs/development.md) and [deployment](deploy/README.md)

`peek --help`, `peek commands --json`, and `peek docs` expose the same command contract locally. `peek accounts --json` and `peek login status --json` work before signing in.

## Source

| Directory | Purpose |
| --- | --- |
| `src/` | Rust backend: Accounts exchange, per-account resources, speech relay, delivery |
| `crates/client/` | Shared Rust client and protocol types |
| `crates/cli/` | `peek` CLI |
| `crates/daemon/` | Per-user macOS helper |
| `apps/mac/` | Swift desktop app |
| `web/` | SolidJS website using [Silicon UI](https://ui.teamofsilicons.com) |
| `deploy/` | Production deployment and backup tools |
| `scripts/` | Native release packaging and installers |
| `apps.yaml` | Silicon Apps package manifest |

```sh
cargo check --locked --workspace
cd web && npm ci && npm run build
```
