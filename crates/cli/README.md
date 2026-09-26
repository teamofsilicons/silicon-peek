# silicon-peek-cli

`peek` is the command-line interface of [Peek](https://peek.teamofsilicons.com): quick, voice-first
exchanges between Carbons and Silicons on a Mac. A Silicon claims one of eight positions around the
screen, registers a small JavaScript drawing for its bubble, then speaks a sentence, shows up to
three text or image elements, or asks one question. The Carbon answers by voice, keyboard or click,
and the answer comes back to the asking Silicon as a Ting event.

Install it with Honeycomb (the CLI and Peek.app travel together):

```sh
curl -fsSL https://peek.teamofsilicons.com/install.sh | sh     # or: honeycomb install 'peek'
```

## Start

```sh
peek iam --json                                        # app id `peek`, owning org `tos`
iam silicon-login --app-id peek --grant-org <org> --approve-scopes   # mint an SLT
peek login '<SLT>'
peek register side 3
peek register drawing ./logo.js
peek send --speak "Build finished" --show '{"elements":[{"type":"text","text":"✓ build"}]}'
peek send --ask '{"question":"Delete old.zip?","type":"single_choice","options":["Keep","Delete"]}'
```

Answers arrive as Ting events of type `peek.ask.answered`; `peek ask get <ASK_ID>` shows them
locally.

## A tree of documentation

- `peek --help`, `peek <command> --help`: what each command is for, how it fits with its
  neighbours, its flags, examples and the next steps.
- `peek commands --json`: the whole command tree as data, generated from the same grammar.
- `peek docs [TOPIC] [--search TEXT] [--all]`: the manuals, bundled into the binary (offline).

With `--json` every command prints exactly one JSON value on stdout; errors go to stderr as
`{"error":{"code","message","hint","retryable","request_id","details"}}`. Exit codes: 0 ok,
1 internal, 2 invalid input, 3 not authenticated, 4 refused, 5 unavailable. peek never prompts.

## Platforms

macOS 26+ runs everything (Peek.app draws the bubbles). On Linux and Windows the same binary
implements the IAM app contract (`iam`, `login`, `login status`, `logout`, `config`) plus `docs`,
`commands`, `report`, `update` and `doctor`; commands that need the Mac exit 4 with
`platform_unsupported`.

## Links

- Docs: <https://peek.teamofsilicons.com/docs>
- Source: <https://github.com/teamofsilicons/silicon-peek>
- Rust client: <https://crates.io/crates/silicon-peek-client>
- Bugs: `peek report --help`

`docs/` in this crate is a generated copy of the repository's `docs/`
(`python3 scripts/sync-cli-docs.py`); edit the originals.
