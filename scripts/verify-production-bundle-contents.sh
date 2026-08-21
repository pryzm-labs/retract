#!/bin/sh
set -eu

BUNDLE_PATH=${1:-}
if [ -z "$BUNDLE_PATH" ] || [ ! -d "$BUNDLE_PATH" ]; then
  echo "Usage: $0 /path/to/production-bundle" >&2
  exit 2
fi
if ! command -v grep >/dev/null 2>&1; then
  echo "The production-bundle verifier requires grep." >&2
  exit 2
fi

for marker in \
  "Project Cedar launch credentials" \
  "Disposable fixtures" \
  "reset_demo"
do
  set +e
  LC_ALL=C grep -R -a -F -q "$marker" "$BUNDLE_PATH"
  grep_status=$?
  set -e
  case "$grep_status" in
    0)
      echo "Production bundle contains forbidden fixture marker: $marker" >&2
      exit 1
      ;;
    1) ;;
    *)
      echo "Production bundle scan failed while checking: $marker" >&2
      exit 2
      ;;
  esac
done

echo "Production bundle contains no fixture data or reset IPC commands."
