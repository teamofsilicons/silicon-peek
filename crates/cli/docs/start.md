# Start here

Peek is a local, voice-first way for Carbons and Silicons to exchange quick messages on a Mac. A Silicon claims one of eight positions around the screen, registers a small JavaScript drawing for its bubble, and then uses the `peek` CLI to speak, show or ask. The Carbon answers by voice, keyboard or click, and the answer comes back to the Silicon as a Ting event.

Use peek when there is no need for history or a thread: a cleanup Silicon asking whether a file matters, a DJ Silicon showing the cover art of the song it just started, a reminder. Use `dm` for anything that needs a conversation.

## Install (once per Mac)

```sh
curl -fsSL https://peek.teamofsilicons.com/install.sh | sh
```

What it does, in order:

1. Installs Honeycomb if `honeycomb` is not on your `PATH` (and not at `~/.honeycomb/dir/system/bin/honeycomb`). Honeycomb 0.5.0 or newer is required.
2. Runs `honeycomb install 'peek'`, which installs the `peek` CLI.
3. Runs `peek app install`, which puts Peek.app in `~/Applications`, verifies its signature and starts it. Peek lives in the menu bar; it has no Dock icon.
4. Prints the next steps.

It does **not** log anyone in. Requirements: macOS 26 or newer on Apple silicon or Intel. On Linux and Windows the script stops and tells you to run `honeycomb install 'peek'` for the CLI only; see [Platforms](platforms.md).

## If you are the Carbon

1. Run the install command above. Peek appears in the menu bar.
2. If macOS asks, allow Peek in **System Settings → General → Login Items & Extensions** so its background helper can run.
3. When a Silicon's bubble slides in, just read or listen. It slides back on its own. Click the down-arrow to close it early; double-click to also stop the speech.
4. To answer, click an option, or press **ctrl+cmd+N** (⌃⌘N; N is the Silicon's position, 1 to 8) and then **`\`** to speak or just start typing. Press **Esc** to cancel. The first time you speak, macOS asks for microphone access. You can change the modifier in Peek's Settings.

Everything else is in [Peek for Carbons](carbon.md).

## If you are a Silicon

Four steps. The first is done for you if you run under Silicon Stemcell with `peek` in `silicon.apps`.

1. **Log in.** Stemcell does it on `silicon connect`. By hand:
   ```sh
   SLT=$(iam -o json silicon-login --app-id peek --grant-org <org> --approve-scopes | jq -r .slt)
   peek login "$SLT"
   peek login status --json        # {"authenticated":true,"id":"si:<handle>",…}
   ```
2. **Claim a position** (1 is top centre, then clockwise to 8, top left). Each Silicon holds exactly one.
   ```sh
   peek register side 3
   ```
3. **Register your drawing**, the JavaScript that animates your bubble's circle. See [Drawing the visual](drawing.md) for the API and ready-made examples.
   ```sh
   peek register drawing ./drawings/logo.js
   ```
4. **Send.** It returns at once; the bubble slides in on the Carbon's screen.
   ```sh
   peek send --speak "Found 3 GB of old builds." \
     --ask '{"question":"Delete old builds?","type":"single_choice","options":["Delete","Keep"]}'
   ```

`peek send` refuses to run until steps 2 and 3 are done, and the error names the exact command to run. The answer arrives later as a `peek.ask.answered` Ting event, which your flow routes to the ISI that asked. Read [Peek for Silicons](silicon.md) before you wire it into a flow, and [Ting events](ting.md) for the exact payloads.

## What is mandatory

| Rule | Why |
|---|---|
| One position per Silicon, 1 to 8. A taken position fails with `side_taken` and lists the free ones. | The Carbon learns "position 3 is the cleanup Silicon" and uses ctrl+cmd+3 to reach it. |
| A registered drawing before any `peek send`. | The bubble's circle is the Silicon's face; peek never shows an anonymous bubble. |
| At least one of `--speak`, `--show`, `--ask`; `--show` and `--ask` never together. | One bubble carries one intent. |
| Asks must be self-contained: at most 80 characters, no context the Carbon cannot see. | The Carbon sees only the bubble, often mid-task. |
| Answers are delivered through Ting, at least once. Dedupe by ting `id`. | The Silicon may be offline; Ting holds and retries delivery. |

## Where to go next

- [CLI reference](cli.md): every command, flag, output and exit code.
- [Speak and show](show.md) and [Ask a question](ask.md): the JSON schemas and limits.
- [Troubleshooting](troubleshooting.md) and `peek doctor` when something does not work.
