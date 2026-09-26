# Speak and show

`peek send --speak` says a sentence in your voice; `peek send --show` puts up to three text or image elements on your bubble's information arc. Use them together for a glanceable update: the DJ Silicon says "Now playing Low Tide" while the cover art, title and year sit on the arc for as long as the speech lasts.

```sh
peek send --speak "Now playing Low Tide by The Silicons." \
  --show '{"elements":[
    {"type":"text","text":"2022"},
    {"type":"image","path":"./covers/low-tide.jpg","caption":"Low Tide by The Silicons"}
  ]}'
```

Both return immediately with `{"send_id":"snd_…","status":"showing",…}`. You need a registered position and drawing first ([Start here](start.md)).

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

The Carbon can close it earlier with the down-arrow: one click slides it away and lets the speech finish; a double click also stops the speech.

A new `--show` replaces one that is still visible. While one of your asks is on screen, new sends queue behind it (at most 5, then `slot_busy`).

## Know when it was seen

Speech and show events are opt-in, because every event costs a Ting delivery and a catch-all flow branch forwards each one as a message:

```sh
peek send --speak "Stand-up in five minutes." --notify speech_finished
peek send --show @status.json --notify show_dismissed
peek config set '{"notify":["speech_finished"]}'     # default for every send
```

| Event | Sent when | Data |
|---|---|---|
| `peek.speech.finished` | the speech finished or the Carbon stopped it | `stopped_by_user`, `played_ms`, `total_ms` |
| `peek.show.dismissed` | the Carbon closed a show before it retracted on its own | `gesture`, `visible_ms` |

Full payloads are in [Ting events](ting.md).

## Examples

**Reminder.**

```sh
peek send --speak "Your call with Priya starts in five minutes." \
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
