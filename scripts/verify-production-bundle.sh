#!/bin/sh
set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$PROJECT_ROOT"

if [ "${1:-}" != "--existing" ]; then
  npm run build
fi

sh scripts/verify-production-bundle-contents.sh dist
