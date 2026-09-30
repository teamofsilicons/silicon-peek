# Peek for Carbons

This page is for the human at the Mac. Peek shows bubbles from your Silicons at the edges of your screen, speaks for them, and lets you answer with your voice, your keyboard or a click. Nothing here needs a terminal after the install.

## Install

```sh
curl -fsSL https://peek.teamofsilicons.com/install.sh | sh
```

You need macOS 26 or newer. The script installs Honeycomb if it is missing, then runs `honeycomb install 'peek'`, which installs the `peek` CLI and, on a Mac, puts Peek.app into `~/Applications` (after checking its Developer ID signature) and starts it. Peek has no Dock icon; look for it in the menu bar. If you already have Honeycomb, `honeycomb install 'peek'` alone does the same.

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
- **A `+N` badge** beside the down-arrow when N more bubbles from the same Silicon are waiting their turn. It counts up and down as they arrive and leave.
- **A `^` button** beside the down-arrow on a question you folded away with Esc; it opens the question again.

Peek samples what is behind each bubble (your desktop picture by default) and picks white or black shading so the text stays readable on light and dark backgrounds.

### Reading and touching a bubble

- **What you can press looks pressable.** Options, buttons, sliders, the text field, the mic, keyboard and down-arrow buttons, and text you can expand sit slightly lifted, with brighter glass and a fine edge, and the pointer becomes a hand over them. Plain labels (a year, a caption that fits, a notice) stay flat. Hover makes the difference obvious.
- **Everything you can press is springy.** On hover it lifts and grows a little with a bounce; pressing squashes it, and it springs back when you let go. Plain labels never move.
- **Cut-off text shows in full on hover.** Any text that ends in "…" (a pill, a caption, an option label, the question) shows its complete text in a glass overlay beside it as soon as you hover, with no delay.
- **Long text opens in place.** Click a long text that does not fit and it opens right where it is, in a small glass popup that bounces open and shows all of it. Click it again, click anywhere else or press Esc to close it.
- **Long text grows away from the edge.** At every one of the eight positions the narrow text column grows toward the middle of the screen, never off it, and leans with the arc only as much as stays easy to read.

When the Silicon speaks, with or without something to show, the bubble slides back 1.5 seconds after the speech ends. A show without speech stays for a few seconds, longer for more text. An ask stays until you answer or dismiss it. If the speech cannot be played (for example, the speech provider is unavailable), the words appear as a pill instead.

A bubble is ready before it moves: Peek prepares the glass, the colours behind it and the Silicon's first drawing frame out of sight, then slides it in, so nothing changes colour or shading after it lands.

**One at a time.** Each Silicon's bubbles take turns at its position. A new one never pushes away the one you are reading; it waits until that one is done (its time ran out, you answered or closed it), then slides in. A Silicon can have at most five waiting, and the `+N` badge shows how many. Silicons can also schedule a bubble for a set time; it appears then, just like any other.

While your Mac is locked or its display is asleep, Peek shows nothing and says nothing. Bubbles wait and appear, in order, when you are back, and a bubble that was scheduled while the Mac slept appears once it wakes. Anything that ran out of time meanwhile is simply dropped, and its Silicon is told whether you ever saw it.

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

The first time you use the mic, macOS asks for microphone permission. Peek records only while you are answering. After you stop, your recording is sent through Peek's backend to OpenAI `gpt-transcribe` to turn it into text (the backend never stores or logs it), and deleted from your Mac once the answer is delivered. See [Privacy](privacy.md).

## Talk to a Silicon first

You do not have to wait for a question. Press ⌃⌘N for a Silicon's position: an empty bubble opens with the hint `\ to talk · type to write · esc to close`. Press `\` to speak or start typing, and press Return. The Silicon receives your words as a `peek.message.received` event, the same way it receives answers. Use this for "remind me about this at 5" or "stop the music". The empty bubble slides back after 12 seconds if you do nothing.

⌃⌘N on a bubble that is already showing brings it forward and pauses its countdown. If a bubble is waiting for that position (because peeks are paused, or it is a hidden test bubble), ⌃⌘N shows it.

## Dismiss and cancel

| You do | On a bubble that speaks or shows | On a question |
|---|---|---|
| Click the down-arrow | It slides away. Speech keeps playing. | It is dismissed and slides away; the Silicon is told. |
| Double-click the down-arrow | It slides away and the speech stops. | It is dismissed and the speech stops; the Silicon is told. |
| Press Esc | It slides away. Speech keeps playing. | It folds into a **compact question**: the question stays, the answer controls hide, and you can still answer it later. |
| Press Esc twice quickly (within 0.4 s) | It slides away and the speech stops. | It is dismissed and the speech stops; the Silicon is told. |
| Press Esc on a compact question | – | "Esc again to dismiss" appears for 2 seconds. Press Esc again to dismiss it; otherwise it stays compact. |
| Click `^` on a compact question | – | The full question comes back. Clicking the question, the mic or keyboard button, or pressing ⌃⌘N does the same. |
| Press Esc while speaking or typing an answer | – | What you said or typed is thrown away (nothing is uploaded) and the question folds into a compact question. |

The down-arrow always closes; it never just folds a question away. A dismissed question reaches its Silicon as `peek.ask.dismissed`; you never have to answer.

### When Esc goes to a bubble

Peek never watches your keyboard, so Esc reaches a bubble only at these moments (none of them needs Input Monitoring or Accessibility permission):

- **For 3 seconds after a bubble appears.** Each bubble gets its own 3 seconds, including the next one from the queue. During that time the app you are using does not get the Esc.
- **While the pointer is over a bubble.**
- **While a bubble has the keyboard,** after ⌃⌘N or while you type an answer.
- **Right after an Esc to that bubble,** for the 0.4 seconds in which a second Esc counts as a double, and while "Esc again to dismiss" shows.

At any other time Esc belongs to the app you are using, as usual. When several bubbles are on screen, Esc goes to the one under the pointer, otherwise to the newest. An open long-text popup closes first. If another app already claims the bare Esc key everywhere, Peek cannot take it: use the down-arrow, and `peek doctor` lists the conflict under `hotkeys`.

## The menu bar and Settings

The menu bar item lists the eight positions with their Silicons and shortcuts, and offers **Pause all peeks** (bubbles wait in the helper until you resume; nothing is lost, and each Silicon can still line up at most five), **Simulation…**, **Settings…** (⌘,) and **Quit Peek** (⌘Q).

Settings has five tabs:

| Tab | Setting | Choices | Default |
|---|---|---|---|
| General | Mode | Normal (arc) or Compact (a line: a tiny visual on the left, the content on the right) | Normal |
| General | Display | The menu-bar screen, or the screen under the pointer when the bubble arrives | Menu-bar screen |
| General | Backdrop | Desktop picture, or Screen contents (samples the real screen under each bubble; needs Screen Recording permission) | Desktop picture |
| General | Hotkeys | ⌘, ⌃⌘, ⌥⌘, ⇧⌘, ⌃⌥⌘ or ⌃⌥, each followed by 1 … 8 | ⌃⌘1 … ⌃⌘8 |
| General | Share usage and diagnostics | On or off. Content is never included. See [Telemetry](telemetry.md). | On |
| General | Keep Silicons' peek CLI up to date | The helper's hourly fallback to Honeycomb's updater | On |
| Voice | Speaking voice | ElevenLabs voice; your Silicon can set a voice and delivery instructions through the CLI | George (`JBFqnCBsd6RMkjVDRZzb`) |
| Voice | Listening language | Automatic (your macOS languages), a fixed language, or any BCP 47 tag | Automatic |
| Testing | Show test peeks | Whether bubbles from testing environments appear; also lists the environments in use | On |
| Startup | Launch at login, background helper | Status only, with a hint and **Open Login Items…** when macOS needs your approval | – |
| Diagnostics | – | Versions (app, helper, macOS, QuickJS, glass mode), the helper's socket, microphone permission, `settings.json` and the end of the helper's log | – |

**Simulation** (menu bar → Simulation…, or General → Open Simulation…) lets you try every combination (position, speak, show, each ask type, mode, appearance, backdrop) with built-in samples. It uses no network, no IAM, no Ting, no Deepgram, no ElevenLabs and no OpenAI, and its bubbles are labelled `SIMULATION`. A voice answer in Simulation is recorded but never transcribed.

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
