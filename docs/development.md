# Development

Peek combines a Rust backend, Rust client library and CLI, a per-user macOS daemon, a Swift desktop app, and a SolidJS website. Each Carbon or Silicon account is identified by its Silicon Accounts UUID.

## Build

```sh
cargo fmt --all -- --check
cargo check --locked --workspace
cargo build --release -p silicon-peek-cli -p silicon-peek-daemon
cd web && npm ci && npm run build
```

Linux and Windows omit the macOS-only `silicon-peek-daemon`. The backend compiles for Linux ARM64 with `cargo zigbuild --release --locked --target aarch64-unknown-linux-musl -p silicon-peek`.

## Run the backend

Copy `.env.example` and configure `PEEK_ACCOUNTS_APP_SECRET` using the app created in Silicon Apps. Keep the encryption key stable across deployments. Set the database path to a local writable file when developing. Run `cargo run -p silicon-peek`.

Secrets stay on the backend. The browser and CLI exchange one-use login codes, store their sessions, and refresh them safely. Account UUIDs partition persisted resources; public IDs are display names, not data ownership keys.

## Desktop app

Generate `apps/mac/Peek.xcodeproj` with XcodeGen and build the Peek scheme with Xcode. The bundled `peekd` helper comes from `dist/peekd`. `scripts/build-mac-release.sh` builds the universal helper and app, verifies signatures, and packages the native payload.

## Release

`apps.yaml` is the Silicon Apps package manifest. `scripts/build-release.sh` builds and validates all payloads. The packager checks executable formats, app signatures, archive paths, and matching versions. Publishing and deployment are documented in [the operator runbook](https://github.com/teamofsilicons/silicon-peek/blob/main/deploy/README.md).

The frontend vendors foundation and button styles from [Silicon UI](https://ui.teamofsilicons.com), adapted to SolidJS. App updates come from Silicon Apps rather than a second network updater.
