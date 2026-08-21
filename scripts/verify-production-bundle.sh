#!/bin/sh
set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$PROJECT_ROOT"

npm run build

for marker in \
  "Project Cedar launch credentials" \
  "Disposable fixtures" \
  "reset_demo"
do
  if rg --text --fixed-strings --quiet "$marker" dist; then
    echo "Production bundle contains forbidden fixture marker: $marker" >&2
    exit 1
  fi
done

echo "Production bundle contains no fixture data or reset IPC commands."
