#!/bin/sh
# Stops the local peek stack started by run-local.sh (the most recent one, or DIR).
#   scripts/e2e/stop.sh [DIR]
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec python3 "$here/local_stack.py" stop "$@"
