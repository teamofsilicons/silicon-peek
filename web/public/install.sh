#!/bin/sh
# Install the signed native macOS release. Silicon Apps manages its supported store targets.
install_peek() {
  set -eu
  os=$(uname -s)
  if [ "$os" != Darwin ]; then
    apps=$(command -v silicon-apps 2>/dev/null || true)
    [ -n "$apps" ] || apps="${SILICON_HOME:-$HOME}/.apps/bin/silicon-apps"
    [ -x "$apps" ] || { echo 'Install Silicon Apps from https://apps.teamofsilicons.com, then run: silicon-apps install peek' >&2; exit 1; }
    exec "$apps" install peek
  fi
  os_version=$(sw_vers -productVersion)
  [ "${os_version%%.*}" -ge 26 ] || { echo 'Peek.app requires macOS 26 or newer.' >&2; exit 1; }
  case "$(uname -m)" in
    arm64) triple=aarch64-apple-darwin ;;
    x86_64) triple=x86_64-apple-darwin ;;
    *) echo 'Unsupported CPU architecture.' >&2; exit 1 ;;
  esac
  repo=https://github.com/teamofsilicons/silicon-peek
  version=${PEEK_VERSION:-}
  if [ -z "$version" ]; then
    latest=$(curl -fsSLI --proto '=https' -o /dev/null -w '%{url_effective}' "$repo/releases/latest")
    version=${latest##*/}
  fi
  case "$version" in ''|.|..|*[!a-zA-Z0-9._-]*) echo 'Invalid release version.' >&2; exit 1 ;; esac
  temp=$(mktemp -d)
  trap 'rm -rf "$temp"' EXIT INT TERM
  archive="peek-$version-$triple.tar.gz"
  url="$repo/releases/download/$version"
  curl -fsSL --proto '=https' "$url/$archive" -o "$temp/$archive"
  curl -fsSL --proto '=https' "$url/SHA256SUMS" -o "$temp/SHA256SUMS"
  expected=$(awk -v name="$archive" '$2 == name || $2 == "*"name { print $1 }' "$temp/SHA256SUMS")
  [ ${#expected} -eq 64 ] || { echo 'Missing release checksum.' >&2; exit 1; }
  actual=$(shasum -a 256 "$temp/$archive" | awk '{print $1}')
  [ "$actual" = "$expected" ] || { echo 'Release checksum mismatch.' >&2; exit 1; }
  destination="$HOME/.local/share/peek/releases/$version/$triple"
  mkdir -p "$destination" "$HOME/.local/bin"
  tar -xzf "$temp/$archive" -C "$destination"
  codesign --verify --strict "$destination/bin/peek"
  ln -sfn "$destination/bin/peek" "$HOME/.local/bin/peek"
  /bin/sh "$destination/install-app.sh"
  "$HOME/.local/bin/peek" app install --json >/dev/null
  printf 'Installed Peek %s.\n' "$version"
  case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) printf 'Add ~/.local/bin to PATH, or use ~/.local/bin/peek.\n' ;; esac
  printf 'Sign in with Silicon Accounts, then run: peek login <SLT>\n'
  printf 'Docs: https://peek.teamofsilicons.com/docs\n'
}
install_peek "$@"
