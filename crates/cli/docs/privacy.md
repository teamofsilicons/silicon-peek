# Privacy

peek keeps bubbles, history and caches on the Mac. Speak text and voice instructions go directly to Deepgram for ElevenLabs synthesis. Completed voice recordings go through the Peek backend to OpenAI to be transcribed, answers go through the Peek backend to Ting so the asking Silicon receives them, and a copy of each drawing is kept on the backend so a Silicon's face follows it to a new Mac. This page lists every flow.

## What leaves the Mac

| Data | Goes to | When | Why |
|---|---|---|---|
| The Carbon's **voice** | OpenAI (`api.openai.com`), through the Peek backend | only after the Carbon records a voice answer or message (mic button or `\`) and stops | to turn it into text; the transcript becomes the answer |
| A Silicon's **`--speak` text, voice ID and voice instructions** | directly to Deepgram Voice Agent (`agent.deepgram.com`) for ElevenLabs synthesis | when the bubble speaks (unless it is in the local speech cache) | to synthesize the voice |
| **Answers, dismissals and messages** (question, answer or message text, option labels, transcript for voice answers, timestamps, position) | the peek backend, then Ting, then the asking Silicon | when the Carbon answers, dismisses or writes | this is the point of an ask |
| The **drawing** (the Silicon's JavaScript file) | the peek backend | on `peek register drawing` | restore the Silicon's visual on another Mac |
| **Telemetry** without content | the peek backend, then Space Station | continuously, unless opted out | diagnostics; see [Telemetry](telemetry.md) |

What never leaves the Mac: show elements (text, images, captions), option images, your send history, the recordings once transcribed, the wallpaper or screen samples, and the Carbon's typing before Return.

## Speech providers

- **Recording happens only while the Carbon answers.** The microphone is opened when the Carbon presses the mic button or `\`, and closed when they stop or press Esc. macOS shows its microphone indicator while it is open.
- **Nothing is uploaded until the Carbon stops.** peek does not stream audio while the Carbon talks. A cancelled recording (Esc) or a silent one (never above −50 dBFS) is never uploaded.
- **Transcribe after recording.** The completed recording (16 kHz mono WAV, at most 120 s) goes to OpenAI's `gpt-transcribe` through Peek's authenticated backend. Transcription failures may retry. There is no live transcript: Peek displays only the final result. Option labels and language preferences may accompany the recording as recognition hints.
- **ElevenLabs TTS streams directly to the Mac.** The helper sends speech text, the voice ID and delivery cues to Deepgram Voice Agent for ElevenLabs v4 synthesis, then receives raw PCM directly. The Peek backend grants access but receives no TTS text or audio. No microphone recording is sent on this TTS route. The helper requests Deepgram's model-improvement opt-out and disables session history. Provider processing also depends on the operator's account and agreements; see [Deepgram's privacy policy](https://deepgram.com/privacy) and [ElevenLabs' privacy policy](https://elevenlabs.io/privacy-policy). These settings do not establish ElevenLabs' retention or training policy.
- **Provider API keys stay on the server.** The helper receives a temporary Deepgram token, valid for opening a connection for 30 seconds by default, and keeps it only in memory. Clients receive no long-lived Deepgram or OpenAI API key. The OpenAI relay never stores or logs recordings or transcripts. Only request metadata is recorded.
- **OpenAI data handling.** OpenAI says API content is not used to train models unless the customer opts in. Its endpoint table lists no abuse-monitoring or application-state retention for `/v1/audio/transcriptions`. See [OpenAI's data controls](https://developers.openai.com/api/docs/guides/your-data), checked September 29, 2026, for current conditions and project controls. This is separate from the speech-synthesis providers' policies and is not a Peek setting.
- **Legacy Deepgram keys.** Previously saved org keys remain encrypted for compatibility and can be inspected or deleted with `peek org byo deepgram`. They do not control current speech. TTS uses the server-owned Deepgram account; OpenAI handles transcription.
- **Local files.** The WAV is kept at `~/Library/Application Support/Peek/recordings/` only until the answer is delivered (or its delivery expires), then deleted. Synthesized speech is cached in `cache/tts/` (at most 200 MB, 30 days) so repeated phrases with the same voice, instructions and language are not sent again.

The transcript is sent to the Silicon directly; the Carbon does not review it first. Answer by keyboard or click if you would rather not use your voice.

## What stays on the Mac

In `~/Library/Application Support/Peek/` (private to your macOS user):

| Path | Contents |
|---|---|
| `peekd.sqlite` | positions, drawings in use, send history (payloads), queued and scheduled sends until they are shown or cancelled, asks and their answers and transcripts, the delivery outbox. **No tokens.** |
| `drawings/…` | the active drawing of each Silicon, and the previous one |
| `cache/images/`, `cache/tts/` | copies of shown images (a scheduled send's images are copied when it is scheduled and kept until it is sent or cancelled), synthesized speech |
| `recordings/` | voice answers waiting for delivery |
| `settings.json`, logs | Peek.app settings; `peekd.log` and `ui.log` (rotated at 10 MB × 3) |

Each Silicon's session tokens live in its own `$SILICON_HOME/.peek/` (mode 0600), never in Peek's folder. Delete `~/Library/Application Support/Peek/` and `~/Library/Caches/Peek/` to remove all local history and caches; `peek app uninstall` removes the app.

**Backdrop sampling.** To keep text readable, peek samples the colour behind each bubble from your wallpaper, at a tiny size, on the Mac. Sampling the real screen is opt-in (Settings → General → Backdrop → Screen contents, which needs Screen Recording permission); it captures a 16 × 16 pixel area under each visible bubble, twice a second, excluding peek's own windows. Samples are used for shading and never stored or sent.

## What the peek backend stores

The backend (`https://backend.peek.teamofsilicons.com`) keeps:

| Table | Contents |
|---|---|
| `drawings` | each Silicon's current drawing (JavaScript, at most 256 KiB) |
| `ting_enrollments` | which Silicons are enrolled as Ting recipients, and their subscription id |
| `deliveries` | delivery receipts: event id, type, Ting key, Ting id, status, timestamps. **Not** the event data. |
| `byo_keys` | a legacy org Deepgram key, encrypted (AES-256-GCM), never returned or used for speech |
| `reports` | bug reports sent with `peek report` |
| `idempotency`, `webhook_events`, testing-environment bindings | bookkeeping so retries are safe |

It stores **no IAM tokens** (each Silicon keeps its own), **no audio**, **no speak text or voice instructions**, **no transcripts** and **no show content**. Answer and message text passes through it on the way to Ting and is not kept. TTS content and audio travel directly between the Mac and the speech provider. Completed recordings pass through the backend to OpenAI without being kept or logged. Test environments live in a separate database. Backups are encrypted and expire after 7 days.

Deletion: `peek unregister` deletes the Silicon's drawing on the Mac and on the backend. `peek logout` revokes its session and its Ting enrollment. When IAM reports that a Silicon was removed from an org, the backend deletes its drawing and enrollment.

## What Ting stores

Answers are delivered as Ting events, so Ting holds them under its own retention while it delivers them (and keeps undelivered events for the Silicon for a limited time). A Silicon that mutes peek in Ting still has events accepted and stored, but never delivered.

## Identity

The Carbon at the Mac does not sign in to peek. Answers carry only the asking Silicon's identity. Telemetry records hashed actor ids, never raw ones.
