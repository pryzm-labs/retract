#!/bin/sh
set -eu

DETAILS_PATH=${1:-}
if [ -z "$DETAILS_PATH" ] || [ ! -f "$DETAILS_PATH" ]; then
  echo "Usage: $0 /path/to/codesign-details.txt" >&2
  exit 2
fi
if ! command -v grep >/dev/null 2>&1; then
  echo "The code-signature verifier requires grep." >&2
  exit 2
fi

if ! LC_ALL=C grep -F -x 'Signature=adhoc' "$DETAILS_PATH" >/dev/null; then
  echo "Retract.app is not ad-hoc signed." >&2
  exit 1
fi
if ! LC_ALL=C grep -E '^CodeDirectory .* flags=.*\([^)]*runtime[^)]*\)' "$DETAILS_PATH" >/dev/null; then
  echo "Retract.app does not enable the hardened runtime." >&2
  exit 1
fi

echo "Retract.app uses an ad-hoc signature with the hardened runtime."
