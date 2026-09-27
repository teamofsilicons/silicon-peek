# Peek documentation

Peek puts a small glass bubble on a Carbon's Mac screen so a Silicon can speak a sentence, show up to three things, or ask one quick question, without opening a conversation. It is built for moments nobody will come back to: "is this file safe to delete?", "now playing …", "stand-up in five minutes".

There are two kinds of pages here. **Instructive** pages tell you exactly what to run and what is mandatory. **Informative** pages explain why peek works the way it does, so you can make good judgement calls when you build on it. Start with the instructive pages; follow their links when you need the reasons.

The same pages ship inside the CLI. `peek docs <topic>` prints any of them offline, and `peek docs --search <text>` searches all of them.

## Install

One command installs Honeycomb (if missing), the `peek` CLI and Peek.app. It does not log anyone in. With Honeycomb already installed, `honeycomb install 'peek'` also installs and starts Peek.app on a Mac.

```sh
curl -fsSL https://peek.teamofsilicons.com/install.sh | sh
```

Peek.app needs macOS 26 or newer. On Linux and Windows only the CLI is installed (`honeycomb install 'peek'`), and only the IAM commands work there. See [Platforms](platforms.md).

## Instructive: use peek

| Topic | Read it when | CLI |
|---|---|---|
| [Start here](start.md) | You are new. Four steps for Carbons, four for Silicons. | `peek docs start` |
| [Peek for Carbons](carbon.md) | You are the human at the Mac: shortcuts, voice, typing, settings. | `peek docs carbon` |
| [Peek for Silicons](silicon.md) | You are a Silicon (or you write one): `silicon.yaml`, `tools.md`, flows. | `peek docs silicon` |
| [CLI reference](cli.md) | You need a command's exact flags, output or exit code. | `peek docs cli` |
| [Speak and show](show.md) | You want to say something or show text and images. | `peek docs show` |
| [Ask a question](ask.md) | You need an answer: text, choice, slider or range. | `peek docs ask` |
| [Drawing the visual](drawing.md) | You are writing the JavaScript drawing for your bubble. | `peek docs drawing` |
| [Ting events](ting.md) | You route answers into your flow. | `peek docs ting` |

## Informative: how and why

| Topic | What it explains | CLI |
|---|---|---|
| [IAM and sessions](iam.md) | Scopes and why each exists, login, token custody, refresh. | `peek docs iam` |
| [Testing environments](testing.md) | Running peek against an isolated IAM world with the same code path as production. | `peek docs testing` |
| [Telemetry](telemetry.md) | What is recorded, what never is, and every way to turn it off. | `peek docs telemetry` |
| [Privacy](privacy.md) | Where voice, text and drawings go, and what the backend keeps. | `peek docs privacy` |
| [Platforms](platforms.md) | macOS versus Linux and Windows, and the exact errors. | `peek docs platforms` |
| [Development](development.md) | Architecture, building from source, the IPC protocol, contributing. | `peek docs development` |
| [Versioning and compatibility](versioning.md) | API v1, IPC protocol 1, the compatibility matrix, deprecation. | `peek docs versioning` |
| [Troubleshooting](troubleshooting.md) | A symptom-to-fix table and `peek doctor`. | `peek docs troubleshooting` |

## Links

- Website: https://peek.teamofsilicons.com
- Source: https://github.com/teamofsilicons/silicon-peek
- Rust client library: https://crates.io/crates/silicon-peek-client
- Bugs and fixes: `peek report --help`
