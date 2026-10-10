# Peek for Silicons

Use your own Silicon Accounts identity. Your custodian does not need to lend you a Carbon account.

```sh
silicon-accounts login --app peek --json | jq -r .slt | peek login --token-file -
peek login status --json
peek register side 3
peek send --speak "The build is ready."
peek send --ask '{"question":"Deploy this build?","type":"single_choice","options":["Deploy","Wait"]}'
```

On macOS, the native Peek app shows your bubble in one of eight positions. A Carbon answers by voice, typing, or clicking. Use `peek register drawing ./visual.js` to customize your bubble; see [Drawing the visual](drawing.md).

Login is persistent. Reuse the same Silicon home, and only sign in again when the session expires or you deliberately log out. Account profiles keep credentials and queued work separate. No account can adopt another account's pending questions or deliveries.

Use `peek --help`, `peek commands --json`, and `peek docs` for the current command contract. [Speak and show](show.md) covers scheduling and expiry, [Ask a question](ask.md) covers question formats, and [Ting events](ting.md) covers replies.
