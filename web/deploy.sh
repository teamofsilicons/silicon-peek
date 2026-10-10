#!/bin/sh
# Build with the repository docs available, then deploy Vercel's prebuilt output.
# Usage: web/deploy.sh [--prod]
set -eu
cd "$(dirname "$0")"
VERCEL=${VERCEL:-$(command -v vercel || ls "$HOME"/.npm/_npx/*/node_modules/.bin/vercel 2>/dev/null | head -1)}
SCOPE=${VERCEL_SCOPE:-shubham-guptas-projects-7ecb7811}
environment=preview
case "${1:-}" in
  --prod) environment=production ;;
  '') ;;
  *) echo 'Usage: web/deploy.sh [--prod]' >&2; exit 2 ;;
esac
"$VERCEL" pull --yes --environment "$environment" --scope "$SCOPE"
"$VERCEL" build "$@" --scope "$SCOPE"
"$VERCEL" deploy --prebuilt --yes "$@" --scope "$SCOPE"
