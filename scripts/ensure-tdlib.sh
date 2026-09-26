#!/bin/sh
set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DIST_DIR="$PROJECT_ROOT/vendor/tdlib-dist"
ARCHIVE="$DIST_DIR/libtdjson_static.a.gz"
STAMP_FILE="$DIST_DIR/build-stamp.txt"
TDLIB_VERSION="1.8.64"
EXPECTED_ARCH="arm64"
EXPECTED_COMPRESSED_SHA="e93e2134e9fb57f7d019a8802f8518b9c0339806224af17df21fa965785afb95"
EXPECTED_ARCHIVE_SHA="aab5736f737319a13bcb871aa2b8a7a90a33e28ec9708fee53dbd387a24b98e4"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "$EXPECTED_ARCH" ]; then
  echo "The included TDLib engine currently targets Apple-silicon macOS." >&2
  exit 1
fi

for tool in gzip shasum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing required TDLib verification tool: $tool" >&2
    exit 1
  fi
done

if [ ! -f "$ARCHIVE" ] || [ ! -f "$STAMP_FILE" ] || [ ! -f "$DIST_DIR/TDLib-LICENSE_1_0.txt" ]; then
  echo "The reviewed TDLib static artifact is missing. Restore vendor/tdlib-dist from the repository checkout." >&2
  exit 1
fi

ACTUAL_COMPRESSED_SHA=$(shasum -a 256 "$ARCHIVE" | cut -d ' ' -f 1)
ACTUAL_ARCHIVE_SHA=$(gzip -dc "$ARCHIVE" | shasum -a 256 | cut -d ' ' -f 1)
if [ "$ACTUAL_COMPRESSED_SHA" != "$EXPECTED_COMPRESSED_SHA" ] || [ "$ACTUAL_ARCHIVE_SHA" != "$EXPECTED_ARCHIVE_SHA" ]; then
  echo "The included TDLib static artifact does not match Retract's reviewed digests." >&2
  echo "Restore vendor/tdlib-dist/libtdjson_static.a.gz from the reviewed repository checkout." >&2
  exit 1
fi

if ! grep -q "^archive_sha256=$EXPECTED_ARCHIVE_SHA file=libtdjson_retract.a$" "$STAMP_FILE" \
  || ! grep -q "^compressed_sha256=$EXPECTED_COMPRESSED_SHA file=libtdjson_static.a.gz$" "$STAMP_FILE" \
  || ! grep -q '^openssl=3.6.3 provider=sqlcipher-vendored-openssl$' "$STAMP_FILE"; then
  echo "The TDLib provenance stamp does not match the reviewed static artifact." >&2
  exit 1
fi

echo "TDLib $TDLIB_VERSION is ready for Retract ($EXPECTED_ARCH, static SHA-256 verified)."
