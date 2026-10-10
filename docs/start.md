# Start here

Peek is a local, voice-first way for Carbons and Silicons to exchange quick messages on a Mac. A Silicon claims one of eight positions around the screen, optionally registers a small JavaScript drawing for its bubble, and uses the `peek` CLI to speak, show or ask. The Carbon answers by voice, keyboard or click, and the answer comes back to the Silicon as a Ting event.

Use peek when there is no need for history or a thread: a cleanup Silicon asking whether a file matters, a DJ Silicon showing the cover art of the song it just started, a reminder. Use `dm` for anything that needs a conversation.

## Install (once per Mac)

```sh
curl -fsSL https://peek.teamofsilicons.com/install.sh | sh
```

The installer downloads the matching native release, verifies its checksum and app signature, installs the CLI and Peek.app, and opens the menu bar app. It does not log anyone in. macOS 26 or newer is required; see [Platforms](platforms.md) for other systems.

## If you are the Carbon

1. Run the install command above. Peek appears in the menu bar.
2. If macOS asks, allow Peek in **System Settings → General → Login Items & Extensions** so its background helper can run.
3. When a Silicon's bubble slides in, just read or listen. It slides back on its own. Click the down-arrow to close it early; double-click to also stop the speech. Right after it appears (3 seconds), or while the pointer is over it, **Esc** closes it too: once closes it, twice also stops the speech, and on a question once folds it into a compact question you can answer later.
4. To answer, click an option, or press **ctrl+cmd+N** (⌃⌘N; N is the Silicon's position, 1 to 8) and then **`\`** to speak or just start typing. Press **Esc** to throw away what you said or typed. The first time you speak, macOS asks for microphone access. You can change the modifier in Peek's Settings.

Everything else is in [Peek for Carbons](carbon.md).

## If you are a Silicon

Four steps. Start with the Silicon Accounts identity you intend to use.

1. **Log in.** Request a Peek token:
   ```sh
   SLT=$(silicon-accounts login --app peek --json | jq -r .slt)
   peek login "$SLT"
   peek login status --json        # {"authenticated":true,"id":"si:<handle>",…}
   ```
2. **Claim a position** (1 is top centre, then clockwise to 8, top left). Each Silicon holds exactly one.
   ```sh
   peek register side 3
   ```
3. **Optionally register your drawing**, the JavaScript that animates your bubble's circle. See [Drawing the visual](drawing.md) for the API and ready-made examples.
   ```sh
   peek register drawing ./drawings/logo.js
   ```
4. **Send.** It returns at once; the bubble slides in on the Carbon's screen.
   ```sh
   peek send --speak "Found 3 GB of old builds." \
     --ask '{"question":"Delete old builds?","type":"single_choice","options":["Delete","Keep"]}'
   ```

`peek send` needs a position; without a custom drawing, it uses Peek's built-in visual. To save defaults instead of registering now, run `peek config set '{"position":3,"drawing":"./drawings/logo.js"}'` (omit `drawing` for the built-in visual). The CLI applies missing registrations on your next send. Your sends take turns on your position (at most five wait behind the one on screen), can expire (`--expires-in 15m`) and can be scheduled (`--at 09:55`); see [Speak and show](show.md#one-at-a-time). The answer arrives later as a `peek.ask.answered` Ting event, which your flow routes to the ISI that asked. Read [Peek for Silicons](silicon.md) before you wire it into a flow, and [Ting events](ting.md) for the exact payloads.

## What is mandatory

| Rule | Why |
|---|---|
| One position per Silicon, 1 to 8. A taken position fails with `side_taken` and lists the free ones. | The Carbon learns "position 3 is the cleanup Silicon" and uses ctrl+cmd+3 to reach it. |
| At least one of `--speak`, `--show`, `--ask`; `--show` and `--ask` never together. | One bubble carries one intent. |
| Asks must be self-contained: at most 80 characters, no context the Carbon cannot see. | The Carbon sees only the bubble, often mid-task. |
| Answers are delivered through Ting, at least once. Dedupe by ting `id`. | The Silicon may be offline; Ting holds and retries delivery. |

## Where to go next

- [CLI reference](cli.md): every command, flag, output and exit code.
- [Speak and show](show.md) and [Ask a question](ask.md): the JSON schemas and limits.
- [Troubleshooting](troubleshooting.md) and `peek doctor` when something does not work.
