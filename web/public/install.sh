#!/bin/sh
# Install peek: Honeycomb (if missing) → the peek CLI → Peek.app. No IAM login happens here.
install_peek() {
  set -eu
  PATH=/usr/bin:/bin:/usr/sbin:/sbin:${PATH:-}
  [ "$(uname -s)" = Darwin ] || { echo "Peek.app needs macOS 26+. On Linux/Windows install only the CLI: honeycomb install 'peek'" >&2; exit 1; }
  os=$(sw_vers -productVersion); [ "${os%%.*}" -ge 26 ] || { echo "Peek.app needs macOS 26 or newer; this Mac runs $os." >&2; exit 1; }
  case "$(uname -m)" in arm64|x86_64) ;; *) echo "Unsupported CPU: $(uname -m)" >&2; exit 1 ;; esac
  HC=$(command -v honeycomb 2>/dev/null || true)
  [ -n "$HC" ] || [ ! -x "$HOME/.honeycomb/dir/system/bin/honeycomb" ] || HC="$HOME/.honeycomb/dir/system/bin/honeycomb"
  if [ -z "$HC" ]; then
    printf '[1/4] Installing Honeycomb…\n'
    /bin/bash -c "$(curl -fL --proto '=https' --connect-timeout 20 --max-time 120 https://raw.githubusercontent.com/teamofsilicons/silicon-honeycomb/main/install.sh)"
    HC="$HOME/.honeycomb/dir/system/bin/honeycomb"
  else printf '[1/4] Honeycomb found at %s\n' "$HC"; fi
  hv=$("$HC" --version 2>/dev/null | awk '{print $NF}')       # UNCERTAIN: exact --version format; require >= 0.5.0
  case "$hv" in 0.[0-4].*) echo "Honeycomb $hv is too old for peek (need >= 0.5.0). Stemcell users: silicon update. Otherwise: honeycomb self-update." >&2; exit 1 ;; esac
  printf '[2/4] Installing the peek CLI…\n'
  out=$("$HC" install 'peek' --json) || { echo "honeycomb install 'peek' failed. Until peek is public, sign in as a tos member first: honeycomb login" >&2; exit 1; }
  PEEK=$(printf '%s' "$out" | sed -n 's/.*"help_command":"\([^"]*\)\/peek --help".*/\1\/peek/p')
  [ -x "$PEEK" ] || PEEK=$(command -v peek)
  printf '[3/4] Installing Peek.app into ~/Applications…\n'
  "$PEEK" app install --json >/dev/null
  printf '[4/4] Done. Peek is running in your menu bar.\n\n'
  printf 'Next:\n  Carbon: allow the microphone when asked; press ctrl+cmd+<1-8> (⌃⌘1…⌃⌘8) to open a Silicon'"'"'s peek (change the modifier in Settings → General → Keyboard → Hotkeys).\n'
  printf '  Silicon: add `peek` to silicon.apps in silicon.yaml, or run: iam silicon-login --app-id peek --grant-org <org> --approve-scopes, then peek login <SLT>\n'
  printf '  Docs: https://peek.teamofsilicons.com/docs\n'
}
install_peek "$@"
