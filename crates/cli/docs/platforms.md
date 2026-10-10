# Platforms

Peek's desktop bubbles, voice, and hotkeys require macOS 26 or newer on Apple Silicon or Intel. Linux and Windows builds support account sign-in, configuration, documentation, and backend operations. Desktop commands explain when macOS is required.

```sh
# Native macOS installer
curl -fsSL https://peek.teamofsilicons.com/install.sh | sh

# Silicon Apps package, on a supported store target
silicon-apps install peek
peek accounts --json
peek login status --json
```

Silicon Apps currently has live validation workers for Linux. macOS and Windows binaries are distributed as native GitHub release downloads until those store workers are available. [GitHub releases](https://github.com/teamofsilicons/silicon-peek/releases) include the macOS app and its CLI.

Silicon Apps manages updates for installations it owns. Run `silicon-apps update peek` to check immediately. Native downloads can be updated by rerunning the installer.
