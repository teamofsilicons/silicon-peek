# Platforms

Bubbles need a Mac: Peek.app is a native macOS 26+ app with Liquid Glass. The `peek` CLI, however, is the same Rust binary on every platform Honeycomb supports, so a Silicon on Linux or Windows can list peek in `silicon.apps`, connect, and log in without errors. Only the commands that need a bubble refuse to run there, with an exact error.

## Support matrix

| Platform | Honeycomb target | What works |
|---|---|---|
| macOS 26 or newer, Apple silicon | `macos-aarch64` | everything |
| macOS 26 or newer, Intel | `macos-x86_64` | everything |
| Linux x86_64 (including WSL2) | `linux-x86_64` | IAM and support commands |
| Linux arm64 | `linux-aarch64` | IAM and support commands |
| Windows x86_64 | `windows-x86_64` | IAM and support commands |
| Windows arm64 | `windows-aarch64` | IAM and support commands |

`peek iam --json` reports this as `"platforms":{"full":["macos-aarch64","macos-x86_64"],"iam_only":[…]}`.

The macOS CLI itself runs on older macOS releases, but Peek.app (and so every bubble) needs macOS 26.0 or newer. On an older Mac, the Mac-bound commands fail when the app cannot be installed or started. Linux builds are static (musl), so they run on any distribution; Windows builds are statically linked.

## Commands on Linux and Windows

These work fully everywhere: `iam`, `login`, `login status`, `logout`, `config …`, `ting enroll`, `org byo deepgram …`, `docs`, `commands`, `report`, `update`, `doctor`, `--help`, `--version`.

Every Mac-bound command (`send`, `register side`, `register drawing`, `unregister`, `ask …`, `history`, `status`, `app …` except `app status`, `daemon …`) exits **4** with:

```json
{"error":{"code":"platform_unsupported","message":"peek send shows a bubble on a Mac through Peek.app; this peek runs on linux-x86_64.",
 "hint":"Run this Silicon on macOS 26+ with Peek.app installed, or use dm for text conversations.","retryable":false,
 "details":{"platform":"linux-x86_64","supported_platforms":["macos-aarch64","macos-x86_64"],"docs_url":"https://peek.teamofsilicons.com/docs/platforms"}}}
```

`peek app status --json` answers `{"supported":false,"platform":"linux-x86_64"}` with exit 0, so scripts can check before they try.

Why log in at all on Linux? Stemcell installs and authenticates every app in `silicon.apps` on connect, and a failure aborts the connect. A peek that could not log in on Linux would break every such Silicon, even ones that never send a bubble there.

## Installing the CLI only

```sh
honeycomb install 'peek'
```

The website installer (`install.sh`) is macOS-only: on other systems it stops and prints this command. On a Mac the same command also installs and starts Peek.app, through the install script of the package's macOS targets; the Linux and Windows targets have none.

## macOS details

| Item | Value |
|---|---|
| Minimum macOS | 26.0 (the Liquid Glass APIs) |
| App location | `~/Applications/Peek.app` (your real home; no admin rights needed) |
| Bundle ids | `ai.tos.peek` (app), `ai.tos.peek.daemon` (the helper's signing identifier), `ai.tos.peek.dev` (development builds) |
| Background helper | `peekd`, inside the app at `Contents/Helpers/peekd`, registered as a login item agent |
| Permissions | Microphone (asked the first time you answer by voice); Screen Recording only if you opt in to screen backdrop sampling; no Accessibility or Input Monitoring permission (global shortcuts do not need it, and Esc is claimed the same way, only for 3 s after a bubble appears or while the pointer is over one) |
| Displays | the main screen by default, or the screen under the pointer |

**Liquid Glass.** On systems where peek can render real glass inside its always-inactive panels, drawings get `input.glass === 'live'`. Otherwise peek uses the public frosted material and tells drawings `input.glass === 'frosted'`; everything still works, it just looks more frosted. See [Drawing the visual](drawing.md).

**One app, many Silicons.** Every Silicon home gets its own copy of the CLI from Honeycomb, but there is one Peek.app and one helper per macOS user. The newest build any home offers wins, and the app never downgrades. See [Versioning and compatibility](versioning.md).

**GUI session required.** Bubbles need a logged-in desktop session. Over plain SSH with nobody logged in at the Mac, Mac-bound commands fail with `no_gui_session` (exit 5).
