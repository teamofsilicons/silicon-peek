# Speak and show

`peek send --speak` says a sentence in your voice; `peek send --show` puts up to three text or image elements on your bubble's information arc. Use them together for a glanceable update: the DJ Silicon says "Now playing Low Tide" while the cover art, title and year sit on the arc for as long as the speech lasts.

```sh
peek send --speak "Now playing Low Tide by The Silicons." \
  --show '{"elements":[
    {"type":"text","text":"2022"},
    {"type":"image","path":"./covers/low-tide.jpg","caption":"Low Tide by The Silicons"}
  ]}'
```

Both return immediately with `{"send_id":"snd_…","status":"showing",…}` (`queued` when an earlier send of yours is still on screen). You need a registered position or a configured `position`; a custom drawing is optional ([Start here](start.md)).

## Speak

| Rule | Value |
|---|---|
| Length | 1–2000 characters (Unicode scalar values), including inline tags. Longer fails with `speak_too_long`, exit 2. |
| Engine | ElevenLabs v4 (`eleven_v4`) through Deepgram Voice Agent, streamed directly to the Mac as 24 kHz mono PCM ([Privacy](privacy.md)). Playback starts as audio arrives. |
| Language | `--lang` selects a language hint; otherwise Peek uses reliable local detection and then config `language` as a fallback. Peek sends the normalized primary language to the provider (`pt-BR` becomes `pt`). A language hint does not select an accent or translate the text. |
| Voice | `--voice`, then config `voice`, then the Carbon's per-language setting, then George (`JBFqnCBsd6RMkjVDRZzb`). Use an ElevenLabs voice ID available through Peek's provider account. |
| Delivery | `--voice-instructions`, then config `voice_instructions`, 1–2000 characters. Peek translates these directions into audio cues; adherence is best effort. |

### Customize a Silicon's voice

A Silicon can save the Carbon's preferences from the CLI, then adjust individual messages. George is the default male voice, suitable as the starting voice for a DJ:

```sh
# Persist this Silicon's voice and delivery defaults.
peek config set '{"voice":"JBFqnCBsd6RMkjVDRZzb","voice_instructions":"Warm, conversational male DJ; relaxed pace and gentle emphasis."}'

# Override those instructions for this message only.
peek send --speak 'The build passed. [short pause] All tests are green! [chuckles]' \
  --voice JBFqnCBsd6RMkjVDRZzb --voice-instructions 'Delighted, with a brisk delivery.'

# Clear saved instructions.
peek config unset voice_instructions
```

Choose `--voice` for the speaker; use `--voice-instructions` for the performance. Describe the sound you want in a short phrase: warm, curious, restrained, whispering, or slowly and clearly. Peek converts the instructions into a leading square-bracket audio cue. Plain directions written as ordinary words in `--speak` may be spoken aloud. Audio cues are model prompts, so a particular emotion, pace or pronunciation is not guaranteed.

For a consistent accent, start with a voice whose accent fits the intended language. An accent cue can request a variation, but does not create a new voice. ElevenLabs v4 aims for native pronunciation in the target language when it differs from the reference voice's language, so a voice's original accent does not necessarily carry into another language. See [ElevenLabs v4's accent and voice controls](https://elevenlabs.io/docs/overview/capabilities/text-to-speech/eleven-v4).

### Voices

These 17 voices have been checked to produce audio through Peek's Deepgram-to-ElevenLabs v4 route. The group helps choose a starting voice; it does not restrict the languages that the voice can speak. A successful synthesis check is not a guarantee of accent or pronunciation quality. Voice availability can change with the provider account.

| Voice | Voice ID (`--voice`) | Starting group |
|---|---|---|
| **George (default, male)** | `JBFqnCBsd6RMkjVDRZzb` | Multilingual |
| Piper | `DtsPFCrhbCbbJkwZsb3d` | Multilingual |
| Mark | `UgBBYS2sOqTuMpoF3BR0` | Multilingual |
| Jessica | `cgSgspJ2msm6clMCkdW9` | English |
| Daniel | `onwK4e9ZLuTAKqWW03F9` | English |
| Brian | `jBlmi27XRORxjPquUeCh` | Spanish |
| Aria Bloom | `TC0Zp7WVFzhA8zpTlRqV` | Spanish |
| Jennifer | `D6MRWCKoavI2xUJXmaCb` | Dutch |
| Jerry | `gdTrLNuwWUaxC0z5n1j7` | Dutch |
| Alexandre Boutin | `IPgYtHTNLjC7Bq7IPHrm` | French |
| Mademoiselle French | `F1toM6PcP54s45kOOAyV` | French |
| Ben | `aTTiK3YzK3dXETpuDE2h` | German |
| Doreen Pelz | `mDRP1h6KfUD1XAUJxqr0` | German |
| Aida | `CiwzbDpaN3pQXjTgx3ML` | Italian |
| Archer - Conversational | `Fahco4VZzobUeiPqni1S` | Italian |
| Hinata | `j210dv0vWm7fCknyQpbA` | Japanese |
| Morioki | `8EkOjt4xTPGMclNlh1pk` | Japanese |

An existing voice ID outside this list may work when available to the provider account; Peek does not create or clone voices. The voice ID allows 1–128 ASCII letters, digits, underscores and hyphens. Previously configured Google or Aura voice names must be replaced with an ElevenLabs ID. Peek does not silently substitute another voice.

### Accents and pronunciation

Put the accent cue before the words it should influence:

```sh
peek send --speak '[strong Indian accent] Anuv Jain.'
```

The older Peek shorthand `<indian accent>Anuv Jain</indian accent>` is no longer accepted. Replace it with square-bracket cues. ElevenLabs documents accent cues as experimental. Choose a suitable voice and listen to the result when pronunciation matters.

For any cue that should have a start and an end, put `[x]...[/x]` around the affected words. This applies to delivery, emotion, tone, pace, accents, or other scoped instructions. Paired cues have worked well in user testing; whispering and accents are examples:

```sh
peek send --speak '[whisper] Just between us. [/whisper] Now, back to the music.'
peek send --speak 'Now playing [strong Indian accent] Anuv Jain [/strong Indian accent]. Enjoy the track.'
```

Use the same cue text in the opening and closing brackets, with `/` before the closing cue. Peek passes both through to ElevenLabs; their scope is interpreted by the model rather than enforced by Peek.

For a difficult name, ElevenLabs v4 also documents pronunciation written as `/IPA/` directly in the text. Results depend on the voice and phrase; use an accurate phonetic transcription and test it. See [ElevenLabs' pronunciation guidance](https://elevenlabs.io/docs/overview/capabilities/text-to-speech/best-practices#ipa-with-eleven-v4).

### Vocal expressions and pauses

Place a square-bracket cue where the delivery should change in `--speak`. The literal tag spellings below come from ElevenLabs' [audio-tag help](https://elevenlabs.io/docs/help-center/product/core-capabilities/text-to-speech/how-do-audio-tags-work-with-eleven-v3-and-v4) and [v4 prompting guide](https://elevenlabs.io/docs/overview/capabilities/text-to-speech/best-practices#prompting-eleven-v4), checked October 1, 2026. This is a selection of documented cues, not an exhaustive syntax or a fixed set of guaranteed effects.

| Use | Literal cues |
|---|---|
| Emotion | `[curious]`, `[excited]`, `[sarcastic]`, `[crying]`, `[mischievously]` |
| Delivery | `[whispers]`, `[whispering]`, `[shouts]`, `[shouting]` |
| Reactions | `[laughs]`, `[laughs harder]`, `[starts laughing]`, `[wheezing]`, `[chuckles]`, `[clears throat]`, `[sighs]`, `[exhales]`, `[snorts]` |
| Pauses and breath | `[short pause]`, `[long pause]`, `[exhales sharply]`, `[inhales deeply]` |
| Experimental accent | `[strong X accent]` — replace `X` with the accent |
| Other experimental cues | `[sings]`, `[woo]`, `[fart]` |
| Sound effects | `[gunshot]`, `[applause]`, `[clapping]`, `[explosion]`, `[swallows]`, `[gulps]` |

Be specific about voice delivery: a vague cue can be interpreted as a sound effect. Short phrases inside brackets can describe delivery beyond these examples. Punctuation and ellipses shape rhythm; capitalization can emphasize words. Test the chosen voice rather than expecting exact pause durations or identical performances. ElevenLabs v4 does not support SSML, including `<break>` tags, and does not have the older models' numeric Speed or Style controls. Peek currently uses one voice per send.

```sh
peek send --speak '[sighs] That took a while... [short pause] but it is DONE.' \
  --voice-instructions 'Begin tired, then brighten at the good news.'
```

For long narration, Peek queues speech at sentence boundaries so playback can start sooner. It preserves your text and repeats `--voice-instructions` for each piece. Inline accent or emotion cues may not carry across pieces, so their delivery remains best effort. A long passage without sentence breaks, or a long audio cue, can still delay the first audio.

What `speech.status` in the send result means:

| Status | Meaning |
|---|---|
| `pending` | Speech is being fetched and will play. |
| `cached` | The same voice, instructions, language and text were spoken recently; playback uses the local cache without a network call. |
| `skipped` | There was nothing to speak. |
| `unsupported_language` | A compatibility status from older helpers; current speech does not use the old seven-language gate. |

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

`--at` and `--expires-at` read times in the Mac's time zone unless the value has an offset (`18:00+05:30`, `…Z`) or you pass `--tz Asia/Kolkata`. The full rules, limits and outputs are in the [CLI reference](cli.md#speak-show-and-ask).

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
- [Privacy](privacy.md): speak text and voice instructions are sent directly to Deepgram for ElevenLabs synthesis; show content never leaves the Mac.
