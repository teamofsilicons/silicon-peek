#!/bin/sh
# Starts a local peek stack for development: fake IAM + fake Ting, the real
# peek-server (temp SQLite, PEEK_PUBLIC_ORIGIN http://127.0.0.1:<port>, the real
# Deepgram key from ~/.peek-operator/deepgram-api-key when present) and the real
# peekd in an isolated run (PEEK_SUPPORT_DIR, PEEK_CACHES_DIR, PEEK_DAEMON_SOCKET,
# PEEK_NO_SERVICES=1). Prints the environment for the CLI and for Peek.app.
#
#   scripts/e2e/run-local.sh [--dir DIR] [--no-build] [--no-deepgram] [--ui-executable PATH]
#   . "$DIR/env.sh"; peek login any-slt; peek register side 3; peek send --speak 'Hello'
#   scripts/e2e/stop.sh [DIR]
#
# Everything lives under DIR (default: a new mktemp directory); nothing touches
# the real ~/Library, ~/Applications, launchd or ~/.peek.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec python3 "$here/local_stack.py" start "$@"
