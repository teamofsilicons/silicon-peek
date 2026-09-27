# Privacy

peek is a local app. Bubbles, history, recordings and caches stay on the Mac. What leaves the Mac, leaves for one reason each: speech goes to Deepgram to be spoken or transcribed (directly, or relayed by the peek backend), answers go through the peek backend to Ting so the asking Silicon receives them, and a copy of each drawing is kept on the backend so a Silicon's face follows it to a new Mac. This page lists every flow.

## What leaves the Mac

| Data | Goes to | When | Why |
|---|---|---|---|
| The Carbon's **voice** | Deepgram (`api.deepgram.com`, or the org's own Deepgram endpoint), directly or relayed by the peek backend (see below) | only after the Carbon records a voice answer or message (mic button or `\`) and stops | to turn it into text; the transcript becomes the answer |
| A Silicon's **`--speak` text** | Deepgram, directly or relayed by the peek backend | when the bubble speaks (unless it is in the local speech cache) | to synthesize the voice |
| **Answers, dismissals and messages** (question, answer or message text, option labels, transcript for voice answers, timestamps, position) | the peek backend, then Ting, then the asking Silicon | when the Carbon answers, dismisses or writes | this is the point of an ask |
| The **drawing** (the Silicon's JavaScript file) | the peek backend | on `peek register drawing` | restore the Silicon's visual on another Mac |
| **Telemetry** without content | the peek backend, then Space Station | continuously, unless opted out | diagnostics; see [Telemetry](telemetry.md) |

What never leaves the Mac: show elements (text, images, captions), option images, your send history, the recordings once transcribed, the wallpaper or screen samples, and the Carbon's typing before Return.

## Voice and Deepgram

- **Recording happens only while the Carbon answers.** The microphone is opened when the Carbon presses the mic button or `\`, and closed when they stop or press Esc. macOS shows its microphone indicator while it is open.
- **Nothing is uploaded until the Carbon stops.** peek does not stream audio while the Carbon talks. A cancelled recording (Esc) or a silent one (never above −50 dBFS) is never uploaded.
- **Upload once, transcribe once.** The recording (16 kHz mono WAV, at most 120 s) is sent once to Deepgram's pre-recorded transcription (Nova-3). There is no live transcript anywhere, not even inside peek.
- **Two ways to reach Deepgram.** Before it speaks or transcribes, the helper (peekd) asks the peek backend for a speech token (and reuses the answer until it expires). The answer says which way to go:
  - **Direct mode.** The backend mints a Deepgram token that lives at most 60 seconds, and peekd calls Deepgram itself. The backend sees that a token was requested, never the audio or the text.
  - **Relay (proxy) mode.** Used when the Deepgram key cannot mint tokens (Deepgram answers 401 or 403 to the grant). peekd then sends the speak text, or the finished recording, to the peek backend over HTTPS with the Silicon's own session, and the backend forwards it to Deepgram with its key and streams the answer straight back. **The backend never stores or logs the audio, the text or the transcript**; it records only request metadata (the route, status, duration and Deepgram's request id), and it applies per-Silicon and per-org rate limits.
  - The same rules apply to an org's own key: whichever key serves the request decides the mode.
- **Model training opt-out.** Every Deepgram request carries Deepgram's model-improvement opt-out (`mip_opt_out=true`), in both modes, so Deepgram keeps the audio and text only as long as needed to process the request. It is not a setting: the backend always adds it, and requests are tagged `peek` plus the environment. Deepgram's own policies govern its processing: https://deepgram.com/privacy.
- **Your org's own Deepgram key.** An org admin can make every request for that org use the org's Deepgram account, and a regional endpoint such as `api.eu.deepgram.com` for data residency (`peek org byo deepgram set`). peek never falls back to its own key silently.
- **Local files.** The WAV is kept at `~/Library/Application Support/Peek/recordings/` only until the answer is delivered (or its delivery expires), then deleted. Synthesized speech is cached in `cache/tts/` (at most 200 MB, 30 days) so repeated phrases are not sent again.

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
| `byo_keys` | an org's own Deepgram key, encrypted (AES-256-GCM), never returned |
| `reports` | bug reports sent with `peek report` |
| `idempotency`, `webhook_events`, testing-environment bindings | bookkeeping so retries are safe |

It stores **no IAM tokens** (each Silicon keeps its own), **no audio**, **no speak text**, **no transcripts** and **no show content**. Answer and message text passes through it on the way to Ting and is not kept; in relay mode, audio and speak text pass through it on the way to Deepgram and are not kept or logged either. Test environments live in a separate database. Backups are encrypted and expire after 7 days.

Deletion: `peek unregister` deletes the Silicon's drawing on the Mac and on the backend. `peek logout` revokes its session and its Ting enrollment. When IAM reports that a Silicon was removed from an org, the backend deletes its drawing and enrollment.

## What Ting stores

Answers are delivered as Ting events, so Ting holds them under its own retention while it delivers them (and keeps undelivered events for the Silicon for a limited time). A Silicon that mutes peek in Ting still has events accepted and stored, but never delivered.

## Identity

The Carbon at the Mac does not sign in to peek. Answers carry only the asking Silicon's identity. Telemetry records hashed actor ids, never raw ones.
