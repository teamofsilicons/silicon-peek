# CLI reference

Peek exposes its full current contract through `peek --help`, `peek <command> --help`, and `peek commands --json`. Commands support structured JSON output with `--json`.

## Accounts and persistent sign-in

```sh
peek accounts --json
silicon-accounts login --app peek --json | jq -r .slt | peek login --token-file -
peek login status --json
peek logout
```

The discovery command is offline and reports `app_id: peek`. A signed-out `login status --json` succeeds with `authenticated: false`. Signed-in status identifies the Carbon or Silicon account. Network failures remain errors and do not erase saved credentials.

Each profile retains its own account session across commands and restarts. A request uses the account UUID verified by Silicon Accounts. See [Silicon Accounts and sessions](accounts.md).

## Register a position and visual

```sh
peek register side 3
peek register drawing ./visual.js
```

Positions run clockwise from 1 to 8. One account occupies one position. [Drawing the visual](drawing.md) documents the JavaScript canvas interface.

## Speak, show, and ask

```sh
peek send --speak "The build is ready."
peek send --ask '{"question":"Deploy this build?","type":"single_choice","options":["Deploy","Wait"]}'
```

A send needs at least one of `--speak`, `--show`, or `--ask`. A single send cannot combine `--show` and `--ask`. Questions must make sense on their own. The CLI validates the request before showing it.

Sends can queue, expire, and be scheduled. Use `peek send --help` for exact timing flags. [Speak and show](show.md) and [Ask a question](ask.md) contain the complete content schemas and interaction rules.

## Account settings

```sh
peek config show
peek config set '{"position":3}'
peek account --help
```

Configuration is private to the selected home and profile. Backend resources are private to the authenticated account. Changing an account or API URL does not transfer a pending action's identity.

## Ting

```sh
peek ting --help
```

Enable deliveries explicitly. The backend uses Silicon Accounts App verification and User verification for the original account. If Ting is unavailable or still requires the retired identity model, pending work remains available for retry. See [Ting events](ting.md).

## Desktop app

```sh
peek app status --json
peek app install
peek doctor
```

Desktop bubbles require macOS 26 or later. The native installer verifies the app signature and starts Peek from the menu bar. Account discovery and documentation work on the other native CLI targets.

## Installation and updates

```sh
silicon-apps install peek
silicon-apps update peek
```

These commands apply to targets supported by the store's live package workers. For macOS, the native installer and GitHub release downloads are available while those workers are being brought online. See [Platforms](platforms.md).

## Documentation and reports

```sh
peek docs
peek docs accounts
peek commands --json
peek report --help
```

Exit codes distinguish usage errors, authentication failures, refused operations, and unavailable dependencies. Errors carry a code, message, and recovery hint. Unknown commands fail rather than silently doing another operation. [Troubleshooting](troubleshooting.md) covers common failures.
