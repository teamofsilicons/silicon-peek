# Peek for Carbons

This page is for the human at the Mac. Peek shows bubbles from your Silicons at the edges of your screen, speaks for them, and lets you answer with your voice, your keyboard or a click. Nothing here needs a terminal after the install.

## Install

```sh
curl -fsSL https://peek.teamofsilicons.com/install.sh | sh
```

You need macOS 26 or newer. The script installs Honeycomb if it is missing, the `peek` CLI, and Peek.app into `~/Applications`, then starts it. Peek has no Dock icon; look for it in the menu bar.

The first time Peek starts, macOS may say a background item was added, or ask you to approve it. Open **System Settings → General → Login Items & Extensions** and allow Peek. Peek's helper (`peekd`) runs in the background so Silicons can reach you even when no bubble is open. If you do not approve it, Peek still works while the app is running, and shows a one-time notice.

## The eight positions

Each Silicon owns exactly one position. Position 1 is top centre; the rest follow clockwise.

| Position | Where | Shortcut (default) |
|---|---|---|
| 1 | top centre | ctrl+cmd+1 (⌃⌘1) |
| 2 | top right | ctrl+cmd+2 |
| 3 | right | ctrl+cmd+3 |
| 4 | bottom right | ctrl+cmd+4 |
| 5 | bottom centre | ctrl+cmd+5 |
| 6 | bottom left | ctrl+cmd+6 |
| 7 | left | ctrl+cmd+7 |
| 8 | top left | ctrl+cmd+8 |

A shortcut exists only while a Silicon holds that position. The default modifier is ctrl+cmd, because plain cmd+1…8 already switches tabs in most browsers and editors. You can change it in **Settings → General → Keyboard → Hotkeys**: cmd, ctrl+cmd, opt+cmd, shift+cmd, ctrl+opt+cmd or ctrl+opt. The rest of this page writes the shortcut as **⌃⌘N** (N is the Silicon's position); read it with your own modifier if you changed it.

## What a bubble shows

A bubble slides in from the edge of the screen with a small bounce, the way macOS panels do. It has no background, frame or backdrop of its own: only the pieces below float directly over whatever is already on your screen. It has:

- **The visual**: a circle the Silicon draws itself, in JavaScript. It reacts to the speech, your voice, the pointer and clicks. Clicking it does nothing except let the drawing animate.
- **The information arc**: text pills and bordered images laid out along a curve that faces the centre of your screen, up to three items. Longer text wraps into a narrow, taller column instead of one wide line, so the bubble stays compact.
- **The question arc**: when the Silicon asks something, the question curves above the answer controls.
- **Mic and keyboard buttons**, next to the visual on the side facing the screen edge, when an answer or message is possible.
- **A down-arrow** to close the bubble.

Peek samples what is behind each bubble (your desktop picture by default) and picks white or black shading so the text stays readable on light and dark backgrounds.

### Reading and touching a bubble

- **What you can press looks pressable.** Options, buttons, sliders, the text field, the mic, keyboard and down-arrow buttons, and text you can expand sit slightly lifted, with brighter glass and a fine edge, and the pointer becomes a hand over them. Plain labels (a year, a caption that fits, a notice) stay flat. Hover makes the difference obvious.
- **Everything you can press is springy.** On hover it lifts and grows a little with a bounce; pressing squashes it, and it springs back when you let go. Plain labels never move.
- **Cut-off text shows in full on hover.** Any text that ends in "…" (a pill, a caption, an option label, the question) shows its complete text in a glass overlay beside it as soon as you hover, with no delay.
- **Long text opens in place.** Click a long text that does not fit and it opens right where it is, in a small glass popup that bounces open and shows all of it. Click it again, click anywhere else or press Esc to close it.
- **Long text grows away from the edge.** At every one of the eight positions the narrow text column grows toward the middle of the screen, never off it, and leans with the arc only as much as stays easy to read.

When the Silicon speaks and shows something, the bubble slides back 1.5 seconds after the speech ends. A show without speech stays for a few seconds, longer for more text. An ask stays until you answer or dismiss it. If the speech cannot be played (for example, its language has no voice), the words appear as a pill instead.

## Answer a question

| Ask type | How to answer |
|---|---|
| Text | Click the keyboard button (or press ⌃⌘N and start typing), type, press Return. The text field curves along the arc right under the question. Or answer by voice. |
| Single choice | Click an option. Or say it: "keep it", "the second one". |
| Multiple choice | Click each option, then confirm with ✓ (or Return). Or say "alpha and charlie". |
| Slider, range | Drag the handle(s) along the arc, then confirm with ✓. Or say the number(s). |

**Sliders move smoothly.** While you drag, a handle follows your pointer continuously instead of jumping from step to step, and the value above it shows the step it will land on. When you let go it springs to the nearest step. If the slider has a sensible number of steps, each one is marked with a small dot along the track.

**With the keyboard.** After ⌃⌘N the bubble takes the keyboard: the arrow keys move the highlight between options or nudge a slider, Space toggles a multiple-choice option, and Return answers. Typing a label, a number, "first" or "2" also works; if it matches no option the bubble says "Didn't match an option — tap one or type".

**By voice.** Click the mic button, or press ⌃⌘N and then `\`. The bubble shows a live waveform of what it hears. Stop with a click, `\` or Return.

- For a **text** question, the bubble slides away as soon as you stop. Your words are transcribed once and sent as the answer.
- For a **choice, slider or range** question, the bubble says it is transcribing for a moment, then picks the option you named. If nothing matches, it stays open and says "Didn't match an option — tap one or type". Nothing is sent until an option matches. If transcription takes longer than 8 seconds, the question comes back with "Still transcribing — tap an option or type"; a late match still answers it.
- If the recording was silent, nothing is uploaded, and the bubble stays open with "Didn't hear anything — try again or type".
- If transcription fails, the question comes back with "Couldn't transcribe — type instead". Peek never sends an empty answer.

The first time you use the mic, macOS asks for microphone permission. Peek records only while you are answering. Your recording is sent once to Deepgram to turn it into text (directly, or relayed by peek's backend, which never stores or logs it), and deleted from your Mac once the answer is delivered. See [Privacy](privacy.md).

## Talk to a Silicon first

You do not have to wait for a question. Press ⌃⌘N for a Silicon's position: an empty bubble opens with the hint `\ to talk · type to write · esc to close`. Press `\` to speak or start typing, and press Return. The Silicon receives your words as a `peek.message.received` event, the same way it receives answers. Use this for "remind me about this at 5" or "stop the music". The empty bubble slides back after 12 seconds if you do nothing.

⌃⌘N on a bubble that is already showing brings it forward and pauses its countdown. If a bubble is waiting for that position (because peeks are paused, or it is a hidden test bubble), ⌃⌘N shows it.

## Dismiss and cancel

| You do | What happens |
|---|---|
| Click the down-arrow once | The bubble slides away. Speech keeps playing. |
| Double-click the down-arrow | The bubble slides away and the speech stops. |
| Press Esc while speaking or typing an answer | Your input is cancelled and the bubble slides back. Nothing is uploaded. |
| Dismiss a question (Esc or the down-arrow) | The Silicon is told you dismissed it (`peek.ask.dismissed`). |

While a question is waiting, the same Silicon's later bubbles queue behind it, so questions never pile up on screen.

## The menu bar and Settings

The menu bar item lists the eight positions with their Silicons and shortcuts, and offers **Pause all peeks** (bubbles wait in the helper until you resume; nothing is lost), **Simulation…**, **Settings…** (⌘,) and **Quit Peek** (⌘Q).

Settings has five tabs:

| Tab | Setting | Choices | Default |
|---|---|---|---|
| General | Mode | Normal (arc) or Compact (a line: a tiny visual on the left, the content on the right) | Normal |
| General | Display | The menu-bar screen, or the screen under the pointer when the bubble arrives | Menu-bar screen |
| General | Backdrop | Desktop picture, or Screen contents (samples the real screen under each bubble; needs Screen Recording permission) | Desktop picture |
| General | Hotkeys | ⌘, ⌃⌘, ⌥⌘, ⇧⌘, ⌃⌥⌘ or ⌃⌥, each followed by 1 … 8 | ⌃⌘1 … ⌃⌘8 |
| General | Share usage and diagnostics | On or off. Content is never included. See [Telemetry](telemetry.md). | On |
| General | Keep Silicons' peek CLI up to date | The helper's hourly fallback to Honeycomb's updater | On |
| Voice | Speaking voice | The default Aura-2 voice per language | per language |
| Voice | Listening language | Automatic (your macOS languages), a fixed language, or any BCP 47 tag | Automatic |
| Testing | Show test peeks | Whether bubbles from testing environments appear; also lists the environments in use | On |
| Startup | Launch at login, background helper | Status only, with a hint and **Open Login Items…** when macOS needs your approval | – |
| Diagnostics | – | Versions (app, helper, macOS, QuickJS, glass mode), the helper's socket, microphone permission, `settings.json` and the end of the helper's log | – |

**Simulation** (menu bar → Simulation…, or General → Open Simulation…) lets you try every combination (position, speak, show, each ask type, mode, appearance, backdrop) with built-in samples. It uses no network, no IAM, no Ting and no Deepgram, and its bubbles are labelled `SIMULATION`. A voice answer in Simulation is recorded but never transcribed.

**Test bubbles** come from a Silicon running against a testing environment. They carry a `TEST · <environment name>` pill and a dashed ring, and show "Sent to test silicon" after you answer. A real bubble always takes priority over a test bubble at the same position.

## Ask your Silicon to use peek

If your Silicon does not use peek yet, paste this to it (or to whoever maintains it):

```text
Add peek to silicon.apps in your silicon.yaml, reconnect, then run
`peek register side <a free position 1-8>` and `peek register drawing ./logo.js`.
Use `peek send --speak "…" --ask '{…}'` for quick yes/no questions and
`peek send --speak "…" --show '{…}'` for glanceable updates.
Answers arrive as Ting events of type peek.ask.answered. Docs: peek docs silicon
```

## Updates and uninstall

Peek updates itself. The CLI is updated by Honeycomb; the app and its helper update from the newest build any Silicon has installed, never downgrade, and wait until no bubble, recording or question is on screen.

To remove Peek.app, its login item and its helper:

```sh
peek app uninstall
```

`honeycomb uninstall 'peek'` removes only the CLI; it does not remove the app. Local history and caches live in `~/Library/Application Support/Peek/` and `~/Library/Caches/Peek/`; delete those folders to remove them.

## Next

- [Privacy](privacy.md): what leaves your Mac.
- [Troubleshooting](troubleshooting.md): no bubble, no sound, the mic, shortcuts.
