# Speak and show

`peek send --speak` says a sentence in your voice; `peek send --show` puts up to three text or image elements on your bubble's information arc. Use them together for a glanceable update: the DJ Silicon says "Now playing Low Tide" while the cover art, title and year sit on the arc for as long as the speech lasts.

```sh
peek send --speak "Now playing Low Tide by The Silicons." \
  --show '{"elements":[
    {"type":"text","text":"2022"},
    {"type":"image","path":"./covers/low-tide.jpg","caption":"Low Tide by The Silicons"}
  ]}'
```

Both return immediately with `{"send_id":"snd_…","status":"showing",…}` (`queued` when an earlier send of yours is still on screen). You need a registered position and drawing first ([Start here](start.md)).

## Speak

| Rule | Value |
|---|---|
| Length | 1–2000 characters (Unicode scalar values). Longer fails with `speak_too_long`, exit 2. 2000 is Deepgram Aura's per-request limit. |
| Engine | Deepgram Aura-2, streamed as 24 kHz mono PCM, either directly from Deepgram or relayed by the peek backend ([Privacy](privacy.md)). Playback starts after about 100 ms of audio has arrived. |
| Languages | English, Spanish, German, French, Dutch, Italian, Japanese. |
| Language choice | `--lang` (any BCP 47 tag; its primary subtag is used, so `es-MX` means Spanish), else detection from the text, else config `language` when detection is ambiguous. |
| Voice choice | `--voice`, else config `voice`, else the default for the language (below). |

Default voices:

| Language | Voice |
|---|---|
| en | `aura-2-thalia-en` |
| es | `aura-2-celeste-es` (`aura-2-selena-es` when the text mixes English and Spanish) |
| de | `aura-2-viktoria-de` |
| fr | `aura-2-agathe-fr` |
| nl | `aura-2-rhea-nl` |
| it | `aura-2-livia-it` |
| ja | `aura-2-izanami-ja` |

Any Aura-2 voice name of the form `aura-2-<name>-<lang>` works with `--voice`, as long as its language is one of the seven.

What `speech.status` in the send result means:

| Status | Meaning |
|---|---|
| `pending` | Speech is being fetched and will play. |
| `cached` | The same voice and text were spoken recently; it plays from the local cache without a network call. |
| `skipped` | There was nothing to speak. |
| `unsupported_language` | The text is not in one of the seven languages. Nothing is spoken; the text is shown as a pill instead, and the result carries the warning `speak_language_unsupported`. peek never reads a language with a voice that cannot speak it. |

If speech cannot be fetched (after two quick retries, all before anything has played), the send still succeeds: the text is shown as a pill when there is no `--show`, and your history records the warning `speech_failed`, or `speech_unavailable` when the backend has no usable speech key ([Troubleshooting](troubleshooting.md) lists the reasons). Speech is never retried once playback has started, so the Carbon never hears a sentence twice.

## Show

```jsonc
{"elements":[                                             // 1..3 elements
  {"type":"text","text":"Now playing"},                   // 1..160 characters
  {"type":"image","path":"./covers/co2.jpg","caption":"CO2 by Prateek Kuhad"}   // caption optional, 0..50 characters
]}
```

| Field | Rule | Error (exit 2) |
|---|---|---|
| `elements` | 1 to 3 items | `too_many_elements` |
| text `text` | 1–160 characters | `text_too_long` |
| image `path` | relative to the current directory; PNG, JPEG, HEIC, WebP, or GIF (first frame); at most 10 MiB | `image_unreadable`, `image_unsupported`, `image_too_large` |
| image `caption` | 0–50 characters; omit it or use `null` for none | `caption_too_long` |
| anything else | unknown fields are rejected | `invalid_input` |

Pass the JSON inline, with `@file.json` (relative to the current directory), or `-` for stdin:

```sh
peek send --show @now-playing.json
jq -n --arg t "$TITLE" '{elements:[{type:"text",text:$t}]}' | peek send --show -
```

### How it is laid out

- Elements sit along the information arc, which curves around your visual and faces the centre of the screen. They are centred as a group, like `justify-content: center`, with padding between them.
- Each element takes at most one third of the arc's width. Longer text wraps into a narrow, taller column that grows toward the centre of the screen; whatever still does not fit ends in "…". The Carbon sees the full text by hovering over it, or by clicking it, which opens it in a small popup in place. Still put the important words first: a glance may be all it gets.
- Text becomes a pill; images are drawn with a border, and a caption becomes a pill under the image. Pills rotate to follow the curve.
- Pill shading is white or black, chosen from what is behind the bubble so it stays readable.
- In compact mode the arc becomes a straight line: a tiny visual on the left, the elements on the right.

### Images

The CLI reads each image's bytes and hands them to Peek.app, which copies them into its own cache at once (your file may be temporary), decodes them at no more than 512 px, and computes a dominant colour and a 3–5 colour palette. Your drawing receives the images as opaque handles with those colours, never as paths; see [Drawing the visual](drawing.md). Images stay on the Mac.

## How long it stays

| Send | The bubble slides back |
|---|---|
| `--speak` only | 1.5 s after the speech ends |
| `--speak` and `--show` | 1.5 s after the speech ends, or `--duration` seconds after it ends when given |
| `--show` only | after `--duration` seconds, default `clamp(3 + 0.06 × visible characters, 4, 15)` |

The text-length default applies only to a show without speech; it never adds to the time after speech.

Nothing is shown to nobody. While the Carbon's screen is locked or the display is asleep, the send is queued (`status:"queued"` with a `carbon_away` warning) and shown in order when the Carbon is back. Speech starts only when the bubble is actually on screen, and the clocks above start then too.

The Carbon can close it earlier:

| The Carbon | What happens |
|---|---|
| clicks the down-arrow | the bubble slides away; the speech finishes |
| double-clicks the down-arrow | the bubble slides away and the speech stops |
| presses Esc once | the bubble slides away; the speech finishes |
| presses Esc twice within 0.4 s | the bubble slides away and the speech stops |

Esc reaches a bubble for 3 seconds after it appears, and at any time while the pointer is over it ([Peek for Carbons](carbon.md#dismiss-and-cancel)). Each close is `peek.show.dismissed` with the `gesture` when you opted in: `down_arrow`, `down_arrow_double`, `esc` or `esc_double`.

## One at a time

Your sends take turns on your position. A new send never pushes the visible one away: it waits until the current bubble is done (its time ran out, or the Carbon closed it), then slides in. At most 5 wait behind the one on screen; a sixth fails with `queue_full` (exit 4). The Carbon sees how many are waiting as a small `+N` next to the bubble's down-arrow.

```sh
peek queue                      # what is on screen and what waits, with ids
peek cancel snd_0192…           # withdraw one; no Ting event
peek queue clear                # drop everything that waits (--all also the one on screen)
```

Three options change the order of things:

- **`--replace`** shows this send right now, closing whatever of yours is current (even an ask, which is cancelled without a Ting event). Use it for corrections.
  ```sh
  peek send --speak "Correction: the build failed after all." --replace
  ```
- **`--expires-in` / `--expires-at`** drop a send that is no longer worth showing. If it is still waiting when the deadline passes it is never shown; if it is on screen it slides away and its speech stops. Either way you receive `peek.send.expired`, which says whether it was ever shown.
  ```sh
  peek send --show '{"elements":[{"type":"text","text":"Build 1843 passed"}]}' --expires-in 10m
  peek send --speak "Lunch is here." --expires-at 13:30
  ```
- **`--in` / `--at`** schedule a send for later, once. It is checked now, stored on the Mac and put in your queue at its time; if the Mac was asleep, it is shown when the Mac wakes, unless its `--expires-at` passed. You receive `peek.schedule.due` when it comes due and `peek.send.shown` when it appears.
  ```sh
  peek send --speak "Stand-up in five minutes." --at 09:55
  peek send --show @digest.json --in 2h --expires-at 2026-09-27T20:00
  peek schedule list
  ```

`--at` and `--expires-at` read times in the Mac's time zone unless the value has an offset (`18:00+05:30`, `…Z`) or you pass `--tz Asia/Kolkata`. The full rules, limits and outputs are in the [CLI reference](cli.md#queue-expiry-and-scheduling).

## Know when it was seen

Speech and show events are opt-in, because every event costs a Ting delivery and a catch-all flow branch forwards each one as a message:

```sh
peek send --speak "Stand-up in five minutes." --notify speech_finished
peek send --show @status.json --notify show_dismissed
peek send --show @status.json --notify shown          # tell me when it appears
peek config set '{"notify":["speech_finished"]}'     # default for every send
```

| Event | Sent when | Data |
|---|---|---|
| `peek.speech.finished` | the speech finished or the Carbon stopped it | `stopped_by_user`, `played_ms`, `total_ms` |
| `peek.show.dismissed` | the Carbon closed a show before it retracted on its own | `gesture`, `visible_ms` |
| `peek.send.shown` | the bubble appeared on screen (always for scheduled sends) | `shown_at`, `scheduled`, `schedule_id` |

These are always sent, without opting in, because they tell you something you asked for did not happen as planned:

| Event | Sent when | Data |
|---|---|---|
| `peek.send.expired` | a send with `--expires-in` or `--expires-at` ran out before it finished | `shown` (was it ever on screen), `shown_at`, `expired_at`, `scheduled` |
| `peek.schedule.due` | a scheduled send (`--in`, `--at`) came due | `outcome`: `shown`, `queued`, `expired` or `replaced` |

Full payloads are in [Ting events](ting.md).

## Examples

**Reminder, at the right time.** Scheduled for 10:25 in the Mac's time zone, and dropped if the Carbon is not back by 10:35:

```sh
peek send --at 10:25 --expires-at 10:35 --speak "Your call with Priya starts in five minutes." \
  --show '{"elements":[{"type":"text","text":"Call with Priya"},{"type":"text","text":"10:30"}]}'
```

**Build finished, silently.**

```sh
peek send --show '{"elements":[{"type":"text","text":"Build 1843 passed · 4m 12s"}]}' --duration 6
```

**Now playing, with cover art.** The arc shows the year, the cover with its caption, and your drawing can tint itself from the cover's colours:

```sh
peek send --speak "Next up, Low Tide." --show '{"elements":[
  {"type":"text","text":"2022"},
  {"type":"image","path":"./covers/low-tide.jpg","caption":"Low Tide by The Silicons"}]}'
```

## Next

- [Ask a question](ask.md) when you need an answer back.
- [Privacy](privacy.md): speak text is sent to Deepgram to be spoken; show content never leaves the Mac.
