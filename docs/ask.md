# Ask a question

`peek send --ask` puts one question on the Carbon's screen and returns immediately. The Carbon answers by click, keyboard or voice, and the answer comes back to you as a `peek.ask.answered` Ting event (or on stdout with `--wait`). Use it for small, self-contained decisions nobody will revisit.

```sh
peek send --json --speak "I found a 2 GB zip from 2023 in Downloads." \
  --ask '{"question":"Delete ~/Downloads/old.zip?","type":"single_choice",
          "options":[{"id":"keep","label":"Keep"},{"id":"delete","label":"Delete","image":"./trash.png"}]}'
# {"send_id":"snd_0192…","ask_id":"ask_0192…","slot":3,"status":"showing","speech":{"status":"pending",…},"warnings":[]}
```

Keep the `ask_id`. It is in the answer, and `peek ask get <ASK_ID>` shows its state at any time.

## Write a good question

- **Self-contained.** The Carbon sees only your bubble, often in the middle of something else. "Delete ~/Downloads/old.zip?" works; "Should I do it?" does not.
- **Short.** The question is at most 80 characters and is always text. It curves along its own arc above the answers.
- **Speak the context, ask the decision.** Use `--speak` for the sentence of background, and keep the question itself tight.
- **Give options ids** when your code routes on them; labels are for humans and may change.
- **Expect voice.** Carbons may say "keep it" or "the second one". Distinct, short labels match best; two options with the same label are rejected.

## Schema

`--show` and `--ask` are mutually exclusive. `--speak` works with both. Pass the JSON inline, as `@file.json` (relative to the current directory), or `-` for stdin. Unknown fields are rejected with `invalid_input`; lengths count Unicode scalar values.

```jsonc
{"question":"…",            // 1..80 characters, always text           (question_too_long)
 "type":"text"|"single_choice"|"multiple_choice"|"slider"|"range",
 …type-specific fields…}
```

| Type | Fields |
|---|---|
| `text` | `placeholder`?: 0–60 characters. `max_length`?: 1–2000, default 500. |
| `single_choice` | `options`: 2–6 items (`too_many_options` above 6). Each item is `{"id"?, "label", "image"?}` or a bare `"Label"` string. |
| `multiple_choice` | `options` as above, plus `min`?: 0 or more (default 1) and `max`?: at most the number of options (default all). |
| `slider` | `min`, `max` (greater than `min`), `step`?: greater than 0 (default `(max-min)/100`), `default`?: a number (default `min`), `unit`?: 0–8 characters. |
| `range` | as `slider`, but `default`?: `[from, to]` (default `[min, max]`). |

Option items:

| Field | Rule |
|---|---|
| `id` | optional, `^[a-z0-9_-]{1,32}$`, unique within the ask. Defaults to `"1"`, `"2"`, … in order. |
| `label` | 1–40 characters, unique within the ask (`invalid_input`). |
| `image` | optional path relative to the current directory; the same formats and 10 MiB limit as show images ([Speak and show](show.md)). |

## Examples by type

```sh
# text
peek send --ask '{"question":"What should I name the new branch?","type":"text","placeholder":"e.g. fix-login","max_length":60}'

# single choice, shorthand options (ids become "1" and "2")
peek send --ask '{"question":"Deploy to production now?","type":"single_choice","options":["Deploy","Wait"]}'

# multiple choice
peek send --ask '{"question":"Which reports should I include?","type":"multiple_choice",
  "options":[{"id":"sales","label":"Sales"},{"id":"ops","label":"Ops"},{"id":"hiring","label":"Hiring"}],"min":1}'

# slider
peek send --speak "It's getting loud in here." --ask '{"question":"Music volume?","type":"slider","min":0,"max":100,"step":5,"default":40,"unit":"%"}'

# range
peek send --ask '{"question":"Which hours can I run the backup?","type":"range","min":0,"max":24,"step":1,"default":[1,5],"unit":"h"}'
```

## How the Carbon answers

| Path | How | `via` |
|---|---|---|
| Click | Click an option; drag a slider or range handle along the arc (it follows the pointer smoothly and springs to the nearest step when released; steps are marked with dots), then ✓. | `click` |
| Keyboard | Press ctrl+cmd+N (your position; the Carbon may have chosen another modifier), then type into the curved field right under the question; Return sends. For choices, the arrow keys, Space and Return also work, and a typed label, number or ordinal is matched on the Mac. Or click the keyboard button. | `keyboard` |
| Voice | Click the mic button, or press ctrl+cmd+N and then `\`. Stop with a click, `\` or Return. | `voice` |

Voice answers are recorded on the Mac, then transcribed **once**, after the Carbon stops, by Deepgram Nova-3. There is no live transcript: your drawing sees only the microphone level while the Carbon talks.

- **Text asks.** The bubble slides away as soon as the Carbon stops. The transcript is the answer, delivered in the background.
- **Choice, slider and range asks.** The bubble shows the `transcribing` phase (up to 8 s) while peek matches what was said. It normalizes the transcript and tries exact and fuzzy label matches, ordinals ("the second one"), numerals ("forty two" → 42), and, for multiple choice, "and"-lists ("sales and hiring"). Your option labels are sent to Deepgram as key terms to improve recognition; slider and range asks turn numerals on.
  - **Matched:** that is the answer, and the bubble slides out.
  - **Unmatched or empty:** the bubble stays open with "Didn't match an option — tap one or type". Nothing is sent.
- **Slow transcription.** After 8 seconds without a result the question comes back with "Still transcribing — tap an option or type"; a late match still answers it.
- **Silence.** If the recording never rose above −50 dBFS, nothing is uploaded and the bubble stays open with "Didn't hear anything — try again or type".
- **Failure.** Transcription is retried twice in the background. If it still fails, the question comes back with "Couldn't transcribe — type instead". **An empty answer is never sent.**
- A recording is at most 120 seconds.

When `via` is `voice`, the answer event also carries the final `transcript`, so you can see exactly what was said.

## What you receive

The answer arrives as a `peek.ask.answered` Ting event. Its `data.answer` has one of these shapes:

```jsonc
{"kind":"text","text":"fix-login-redirect"}
{"kind":"single_choice","option_id":"keep","label":"Keep"}
{"kind":"multiple_choice","option_ids":["sales","hiring"],"labels":["Sales","Hiring"]}
{"kind":"slider","value":45}
{"kind":"range","from":1,"to":5}
```

The full event, with `ask_id`, `question`, `via`, `transcript` and timestamps, is in [Ting events](ting.md), together with the flow snippets that route it to the ISI that asked.

## Lifecycle

| State | How it gets there | You receive |
|---|---|---|
| `pending` | the ask is on screen (full or compact) or waiting in your queue (behind your earlier sends, or while the Carbon is away) | – |
| `answered` | the Carbon answered | `peek.ask.answered` (or the `--wait` output) |
| `dismissed` | the Carbon closed it without answering (Esc twice, or the down-arrow once or twice) | `peek.ask.dismissed` with the `gesture` |
| `expired` | it had `--expires-in` or `--expires-at` and the time ran out, on screen or while it waited | `peek.ask.expired`, with `shown` saying whether the Carbon ever saw it |
| `cancelled` | you ran `peek cancel`, `peek ask cancel <ASK_ID>`, `peek queue clear`, `peek unregister` or `peek logout` | nothing: it was your own action |
| `replaced` | a later `peek send --replace` of yours took over the bubble | nothing: it was your own action |

```sh
peek ask get ask_0192… --json
peek ask list --state pending --json
peek ask cancel ask_0192…
```

An ask without a deadline waits until it is answered, dismissed or cancelled. Give it one with `--expires-in 15m` (10 s to 7 days, counted from the send) or `--expires-at 18:00` (a date-time, in the Mac's time zone unless it has an offset or you pass `--tz`).

Your sends take turns: an ask waits behind whatever of yours is on screen, and while it is pending, your later sends wait behind it. At most 5 wait at a time (then `queue_full`, exit 4), so the Carbon never faces a stack of your questions. `peek queue` shows the line; see [Speak and show](show.md#one-at-a-time).

While the Carbon's screen is locked or the display is asleep, nothing is shown: the send returns `status:"queued"` with a `carbon_away` warning and appears when the Carbon is back. Its deadline keeps running from the send, so an ask that expires while it waits is never shown, and you receive `peek.ask.expired` with `"shown":false`.

### How the Carbon puts an ask aside

The Carbon can fold your question away without answering it. One Esc (for 3 seconds after it appears, or while the pointer is over it) turns it into a **compact ask**: the question stays on screen, the answer controls hide, and it can still be answered. The Carbon opens it again with the `^` button next to the down-arrow, by clicking the question, or with ctrl+cmd+N. You receive nothing for this: the ask is still `pending`.

It becomes `dismissed`, and you receive `peek.ask.dismissed`, when the Carbon:

| Does | `gesture` |
|---|---|
| presses Esc twice within 0.4 s, or Esc on a compact ask and again while "Esc again to dismiss" shows (2 s) | `esc` |
| clicks the down-arrow once (on the full or the compact ask; it never just folds it) | `down_arrow` |
| double-clicks the down-arrow (the speech stops too) | `down_arrow_double` |

A dismissal by Esc also stops the ask's speech.

### Scheduled asks

`--in` and `--at` work for asks too. The ask is stored until it comes due, then waits in your queue like any other send; `peek.schedule.due` tells you when it fired and `peek.send.shown` when it appeared. A scheduled ask cannot use `--wait` (the answer arrives as `peek.ask.answered`) and takes `--expires-at`, not `--expires-in`. The `ask_id` is in the `peek send` result already, but `peek ask get` finds it only once it has fired; until then it is in `peek schedule list`.

```sh
peek send --at 09:55 --expires-at 10:30 --speak "Stand-up starts in five minutes." \
  --ask '{"question":"Join stand-up today?","type":"single_choice","options":["Join","Skip"]}'
```

## Wait for the answer inline

```sh
peek send --json --ask '{"question":"Ship it?","type":"single_choice","options":["Ship","Hold"]}' --wait=60
```

- Answered in time: one JSON value with `ask_id`, `send_id`, `state:"answered"`, `answer`, `via`, `transcript` and `answered_at`. **No Ting event is sent for it**: each outcome reaches you through exactly one channel.
- Dismissed, expired, cancelled or replaced while waiting: `{"ask_id","send_id","state":"dismissed"|"expired"|"cancelled"|"replaced"}`. This too replaces the Ting event: no `peek.ask.dismissed` or `peek.ask.expired` follows. `replaced` means a later `peek send --replace` of yours took over the bubble.
- Timeout (default 120 s, at most 600): `{"ask_id","send_id","state":"pending","delivery":"ting"}`, exit 0, and the answer goes through Ting later.
- Write the number with `=` (`--wait=60`); a bare `--wait` waits 120 s.

Agent tool calls often time out after about two minutes. Prefer Ting delivery in ISIs, and use `--wait` for scripts.

## Errors

| Code | Exit | Cause |
|---|---|---|
| `question_too_long` | 2 | more than 80 characters |
| `too_many_options` | 2 | more than 6 options |
| `invalid_input` | 2 | a missing field, fewer than 2 options, a duplicate id or label, `max ≤ min`, an unknown field; `details.field` names it |
| `conflicting_flags` | 2 | `--ask` together with `--show`, `--duration` with `--ask`, `--wait` with `--in` or `--at`, or `--expires-in` with `--expires-at` |
| `image_unreadable`, `image_unsupported`, `image_too_large` | 2 | an option image |
| `queue_full` | 4 | five of your sends already wait behind the one on screen; `peek queue`, then `peek cancel <SEND_ID>` or `peek queue clear` (older peek versions call it `slot_busy`) |
| `ask_not_found` | 4 | `peek ask get` or `cancel` with an unknown id |
| `side_not_registered` | 4 | run `peek register side <1-8>` or configure `position`; custom drawings are optional |
