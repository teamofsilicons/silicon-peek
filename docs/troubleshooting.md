# Troubleshooting

Start with `peek doctor`. It checks everything peek depends on and prints the exact fix for anything that is wrong. Then find your symptom or error code below.

```sh
peek doctor              # readable
peek doctor --json       # {"checks":[{"name","status":"ok"|"warn"|"fail","detail","fix"}],"summary":{"ok","warn","fail"},"version","platform"}
```

`peek doctor` always exits 0. Its checks, by `name`:

| Check | What it looks at |
|---|---|
| `store` | this home's store permissions |
| `session` | the session, and whether IAM rejected it |
| `pending_login` | a login whose response was lost (`peek login --recover` finishes it within 10 minutes) |
| `revocations` | logout revocations that could not reach the backend yet and are still being retried |
| `ting` | Ting enrollment (`peek ting enroll` when it is missing) |
| `backend` | the backend's `/readyz` |
| `honeycomb` | the Honeycomb version (0.5.0 or newer) |
| `app` | Peek.app: installed, signature, build (Mac only, like the rows below) |
| `agent` | whether macOS approved the background item |
| `install_log` | the last lines of the install log |
| `daemon` | the helper's socket and protocol |
| `mic` | microphone permission, as Peek.app reports it |
| `hotkeys` | shortcuts macOS refused |
| `outbox` | the delivery backlog |

A check that has nothing to report (no pending login, no pending revocation) is `ok`.

## Error codes

| Code | Exit | What it means | Fix |
|---|---|---|---|
| `side_not_registered` | 4 | This Silicon holds no position. | `peek register side <1-8>` (the error lists the free ones). |
| `drawing_not_registered` | 4 | No drawing is registered. | `peek register drawing ./logo.js` ([Drawing the visual](drawing.md)). |
| `side_taken` | 4 | Another Silicon holds that position. | Pick one from `details.free`. |
| `drawing_invalid` | 4 | Validation failed; the previous drawing stays active. | Fix the line in the error; re-run with `--check`. |
| `drawing_too_large` | 2 | The file is over 256 KiB. | Shrink it; drawings are code, not assets. |
| `slot_busy` | 4 | Five sends already queue behind your pending ask. | Wait for the answer, or `peek ask cancel <ASK_ID>`. |
| `nothing_to_send`, `conflicting_flags` | 2 | No `--speak`/`--show`/`--ask`, or `--show` with `--ask`. | Send one intent per bubble. |
| `text_too_long`, `caption_too_long`, `question_too_long`, `speak_too_long` | 2 | Over 160, 50, 80 or 2000 characters. | Shorten it. |
| `too_many_elements`, `too_many_options` | 2 | More than 3 show elements, or 6 options. | Split it, or ask a text question. |
| `image_unreadable`, `image_unsupported`, `image_too_large` | 2 | The path (relative to your current directory) cannot be read, is not PNG/JPEG/HEIC/WebP/GIF, or is over 10 MiB. | Check the path and format. |
| `invalid_input`, `invalid_json` | 2 | The input does not match the schema (for example an unknown field, or two options with the same label); `details.field` names the offending field. | See [Speak and show](show.md) and [Ask a question](ask.md). |
| `unknown_config_key` | 2 | `peek config set` (or `app_configs.peek`) has a key peek does not know. | Use a key from `details.valid_keys`. |
| `invalid_silicon_home` | 2 | `SILICON_HOME` is set but empty or does not exist. | Point it at an existing directory, or unset it. |
| `not_logged_in` | 3 | This home has no peek session. | Under Stemcell: `si auth setup peek`. By hand: mint an SLT, `peek login "$SLT"`. |
| `session_rejected` | 3 | IAM rejected the session (revoked, reused or expired refresh). | Log in again; queued answers resume afterwards. |
| `slt_rejected` | 3 | The SLT was already used, expired (2 minutes), or invalid. | Mint a new one. |
| `login_attempt_expired` | 3 | `peek login --recover` ran more than 10 minutes later. | Mint a new SLT. |
| `slt_is_public_id` | 2 | A public id such as `si:x` was passed as the SLT in production. | Use a real SLT; public ids work only in testing environments. |
| `reconsent_required` | 3 | The session lacks a scope peek needs. | Log in again with `--approve-scopes`. |
| `recipient_not_registered` | 4 | You are not a Ting recipient for peek, so answers cannot be delivered. | `peek ting enroll`; waiting answers are retried right away. |
| `not_org_admin` | 4 | Managing the org's Deepgram key needs owner or admin. | Ask an org admin. |
| `ask_not_found` | 4 | No such ask for this Silicon on this Mac. | `peek ask list`. |
| `platform_unsupported` | 4 | This command needs a Mac. | Run on macOS 26+, or use `dm` ([Platforms](platforms.md)). |
| `cli_outdated` | 4 | This CLI is older than the helper's protocol. | `honeycomb update 'peek'`. |
| `store_schema_newer` | 4 | A newer peek wrote this home's store. | `honeycomb update 'peek'`. |
| `peek_service_unavailable` | 5 | Peek.app or the helper did not answer within 10 s of being started. | `peek doctor`; the error includes the last helper log line. |
| `daemon_unavailable` | 5 | The helper's socket did not answer. | `peek daemon restart`, then `peek doctor`. |
| `no_gui_session` | 5 | Nobody is logged in at the Mac's desktop (for example, SSH only). | Log in at the Mac. |
| `app_update_pending` | 5 | A newer app is being installed for this CLI. | Retry in a few seconds; the swap waits until the screen is idle. |
| `backend_unavailable`, `iam_unavailable` | 5 | The backend or IAM could not be reached. | Retry with backoff. `login status` prints nothing on stdout in this case, by design. |
| `iam_misconfigured` | 5 | The backend cannot authenticate to IAM. | An operator problem; retry later or `peek report`. |
| `speech_unavailable` | 5 | Speech could not be set up; `details.reason` says why: `not_configured`, `org_key_invalid`, `org_out_of_credits`, `org_model_forbidden`, `org_base_url_not_allowed`, `peek_key_invalid`, `peek_out_of_credits`, `rate_limited` or `deepgram_unavailable`. The bubble shows the text as a pill instead. | For `org_*` reasons, the org admin fixes the key (`peek org byo deepgram show`; for `org_base_url_not_allowed`, set it again with a `--base-url` the backend allows). `peek_*` reasons are an operator problem: `peek report`. The others retry on their own. |
| `ting_unavailable` | 5 | Ting is unreachable. | Nothing: deliveries retry automatically. |
| `ting_type_missing` | 5 | A peek event type is not registered in this context (an operator step). | Nothing: the helper retries every 15 minutes; tell the operator. |
| `rate_limited` | 5 | Too many requests (for example speech requests for one Silicon or one org). | Wait for `Retry-After`, then retry. |
| `unauthenticated` | 3 | The backend did not accept the session's access token. | peek refreshes once by itself; if it persists, log in again. |
| `authority_required` | 3 | A delivery needs a fresh login, re-consent or Ting enrollment. | `peek login status --json` says which. |
| `environment_not_prepared` | 4 | The testing environment was not imported for peek. | See [Testing environments](testing.md#provision-an-environment-operators). |
| `unexpected_response` | 1 | The backend or the helper answered with a shape this CLI cannot read. | `honeycomb update 'peek'`; if it persists, `peek report`. |
| `testing_secret_invalid`, `testing_generation_changed` | 3 | The testing environment's secret is wrong, or it was cleaned. | See [Testing environments](testing.md). |
| `internal_error`, `store_corrupt` | 1 | A bug, or a damaged store file. | `peek report --attach-status`. |

## Symptoms

| Symptom | Likely cause | Fix |
|---|---|---|
| `peek send` succeeds but no bubble appears | Peek.app is not running, all peeks are paused, or a test bubble waits behind a production one at the same position | Menu bar → turn off **Pause all peeks**; Settings → Testing → **Show test peeks** for test bubbles; `peek app status`; `peek doctor`. |
| The first `peek` command on a Mac is slow | It installs and starts Peek.app | Expected once. |
| macOS says a background item was added, or peek works only while the app is open | The helper needs approval | System Settings → General → Login Items & Extensions → allow Peek. |
| No speech, but the text shows as a pill | The language is not one of en, es, de, fr, nl, it, ja (`speak_language_unsupported`), or speech is unavailable (`speech_unavailable`, `speech_failed` in history) | Check the send result's `warnings`; set `--lang` if detection was wrong. |
| The mic button does nothing, or the recording is silent | Microphone permission was denied | System Settings → Privacy & Security → Microphone → enable Peek. |
| "Didn't match an option — tap one or type" | The transcript matched no option | Say the label, "the first one", or click. Short, distinct labels match best. |
| "Couldn't transcribe — type instead" | Transcription failed after retries | Type or click. The helper's `peekd.log` ([Logs](#logs)) has a `transcription failed` line with the error message and code; `peek status` does not report speech. A `speech_unavailable` code is explained under [Error codes](#error-codes). |
| A peek shortcut does nothing, or another app's shortcut stopped working | peek registers ctrl+cmd+<position> (by default) for every occupied position; macOS refuses a shortcut another app already holds | Settings → General → Keyboard → Hotkeys: pick another modifier. `peek doctor` lists refused shortcuts under `hotkeys`. |
| Answers never reach the Silicon's flow | No flow branch, a muted peek in Ting (`silent:true`), missing enrollment, or a rejected session | `peek ask get <ASK_ID> --json` shows `delivery`; `peek login status --json` shows `ting` and `reason`; add Flow A or B ([Peek for Silicons](silicon.md)). |
| Answers arrive twice | Delivery is at least once | Dedupe by ting `id` or `ask_id`. |
| `peek status` shows `authority_required` greater than 0 | Answers wait for a valid session or enrollment | Log in again, or `peek ting enroll`. |
| `peek send` says `queued` with a `carbon_away` warning | The Carbon's screen is locked or the display is asleep, so nothing is shown to nobody | Nothing. The sends are shown in order when the Carbon is back; asks keep their `--expires-in` clock. `peek status --json` shows `carbon.available` and `reason`. |
| `peek send` carries a `ting_not_enrolled` warning | This Silicon has no Ting grant for peek (for example after `peek logout --revoke-ting` in any of its homes) | `peek ting enroll`. Answers that waited are delivered right away. |
| `peek send` carries an `isi_ignored` warning | `$ISI` is over 160 characters or spans lines | Fix or unset `ISI`. The send itself went out, without `metadata.isi`. |
| The drawing shows a plain circle with an initial | Your drawing threw 10 times in a row, overran 30 times in 5 s, or ran out of memory | The next `peek send`, `peek register side` or `peek status` result has a `drawing_fallback_active` warning with the stack, and `peek status` shows `drawing.active: false` with `drawing.last_error`; fix it and `peek register drawing` again. |
| Glass looks frosted, not clear | Live Liquid Glass is unavailable on this system (`input.glass === 'frosted'`) | Nothing to fix; peek falls back on purpose. |
| macOS asks to let Peek access Documents, Desktop or Downloads | A `SILICON_HOME` lives there | Move the home out of those folders, or allow Peek in Privacy & Security → Files and Folders. |
| `silicon connect` fails while installing peek | Honeycomb older than 0.5.0, or Honeycomb could not reach its registry | Update Honeycomb (`silicon update`) and connect again. peek is public, so no Honeycomb login is needed. |
| The installer says Honeycomb is too old | Honeycomb < 0.5.0 | Stemcell users: `silicon update`. Otherwise `honeycomb self-update`. |
| The installer says `honeycomb install 'peek' failed` | Honeycomb could not download the package (network, or a Honeycomb problem) | Run `honeycomb install 'peek'` yourself to see Honeycomb's error, then run the installer again. No Honeycomb login is needed. |

## Logs

| File | Written by |
|---|---|
| `~/Library/Application Support/Peek/peekd.log` | the helper (rotated at 10 MB × 3) |
| `~/Library/Application Support/Peek/ui.log` | Peek.app |
| `~/Library/Application Support/Peek/install-status.txt` | app installs and updates |
| `<SILICON_HOME>/.silicon/silicon.log` | Stemcell, including peek's full stderr from lifecycle calls |

Logs never contain tokens.

## Report a bug

```sh
peek report "exact steps, the command, the error code and what you expected" --attach-status --dry-run   # prints what would be filed; sends nothing
peek report "exact steps, the command, the error code and what you expected" --attach-status
peek report "…" --pr https://github.com/teamofsilicons/silicon-peek/pull/<n>     # if you fixed it
```

A report becomes a public GitHub issue on `teamofsilicons/silicon-peek`. Use `--dry-run` to check what you would send, and never file test reports.

See [Development](development.md) for building and testing a fix.
