#!/bin/sh
set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SOURCE_DIR="$PROJECT_ROOT/vendor/tdlib-source"
BUILD_DIR="$SOURCE_DIR/build-retract"
DIST_DIR="$PROJECT_ROOT/vendor/tdlib-dist"
PINNED_COMMIT="e0943d068ce90b5010f1aea946e6901e25b43bf6"
EXPECTED_OPENSSL_VERSION="3.6.3"
EXPECTED_ARCHIVE_SHA="aab5736f737319a13bcb871aa2b8a7a90a33e28ec9708fee53dbd387a24b98e4"
EXPECTED_COMPRESSED_SHA="e93e2134e9fb57f7d019a8802f8518b9c0339806224af17df21fa965785afb95"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "The reviewed TDLib archive is built on Apple-silicon macOS." >&2
  exit 1
fi
if [ -z "${OPENSSL_ROOT_DIR:-}" ]; then
  echo "Set OPENSSL_ROOT_DIR to the installed OpenSSL 3.6.3 provider built by Cargo's pinned openssl-src dependency." >&2
  exit 1
fi
OPENSSL_HEADER="$OPENSSL_ROOT_DIR/include/openssl/opensslv.h"
if [ ! -f "$OPENSSL_HEADER" ] || ! grep -q "OpenSSL $EXPECTED_OPENSSL_VERSION " "$OPENSSL_HEADER"; then
  echo "OPENSSL_ROOT_DIR must contain the reviewed OpenSSL $EXPECTED_OPENSSL_VERSION headers." >&2
  exit 1
fi

for tool in cmake git gzip libtool shasum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "Missing required build tool: $tool" >&2; exit 1; }
done
if [ ! -d "$SOURCE_DIR/.git" ]; then
  git clone --filter=blob:none --no-checkout https://github.com/tdlib/td.git "$SOURCE_DIR"
fi
if ! git -C "$SOURCE_DIR" cat-file -e "$PINNED_COMMIT^{commit}" 2>/dev/null; then
  git -C "$SOURCE_DIR" fetch --depth 1 origin "$PINNED_COMMIT"
fi
git -C "$SOURCE_DIR" checkout --detach --quiet "$PINNED_COMMIT"
test "$(git -C "$SOURCE_DIR" rev-parse HEAD)" = "$PINNED_COMMIT"

PREFIX_FLAGS="-ffile-prefix-map=$PROJECT_ROOT=/usr/src/retract -fdebug-prefix-map=$PROJECT_ROOT=/usr/src/retract"
cmake -S "$SOURCE_DIR" -B "$BUILD_DIR" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES=arm64 \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=12.0 \
  -DOPENSSL_ROOT_DIR="$OPENSSL_ROOT_DIR" \
  -DOPENSSL_INCLUDE_DIR="$OPENSSL_ROOT_DIR/include" \
  -DOPENSSL_SSL_LIBRARY="$OPENSSL_ROOT_DIR/lib/libssl.a" \
  -DOPENSSL_CRYPTO_LIBRARY="$OPENSSL_ROOT_DIR/lib/libcrypto.a" \
  -DOPENSSL_USE_STATIC_LIBS=TRUE \
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DCMAKE_C_FLAGS="$PREFIX_FLAGS" \
  -DCMAKE_CXX_FLAGS="$PREFIX_FLAGS"
cmake --build "$BUILD_DIR" --config Release --target tdjson_static --parallel "${TDLIB_BUILD_JOBS:-4}"

WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/retract-tdlib-static.XXXXXX")
trap 'rm -rf "$WORK_DIR"' EXIT HUP INT TERM
libtool -static -o "$WORK_DIR/libtdjson_retract.a" \
  "$BUILD_DIR/libtdjson_static.a" \
  "$BUILD_DIR/libtdjson_private.a" \
  "$BUILD_DIR/libtdclient.a" \
  "$BUILD_DIR/libtdcore.a" \
  "$BUILD_DIR/libtdapi.a" \
  "$BUILD_DIR/tddb/libtddb.a" \
  "$BUILD_DIR/sqlite/libtdsqlite.a" \
  "$BUILD_DIR/tde2e/libtde2e.a" \
  "$BUILD_DIR/libtdmtproto.a" \
  "$BUILD_DIR/tdnet/libtdnet.a" \
  "$BUILD_DIR/tdactor/libtdactor.a" \
  "$BUILD_DIR/tdutils/libtdutils.a"
gzip -n -9 -c "$WORK_DIR/libtdjson_retract.a" > "$WORK_DIR/libtdjson_static.a.gz"

ARCHIVE_SHA=$(shasum -a 256 "$WORK_DIR/libtdjson_retract.a" | cut -d ' ' -f 1)
COMPRESSED_SHA=$(shasum -a 256 "$WORK_DIR/libtdjson_static.a.gz" | cut -d ' ' -f 1)
if [ "$ARCHIVE_SHA" != "$EXPECTED_ARCHIVE_SHA" ] || [ "$COMPRESSED_SHA" != "$EXPECTED_COMPRESSED_SHA" ]; then
  echo "The local toolchain did not reproduce Retract's reviewed TDLib archive." >&2
  echo "archive=$ARCHIVE_SHA compressed=$COMPRESSED_SHA" >&2
  exit 1
fi
mkdir -p "$DIST_DIR"
cp "$WORK_DIR/libtdjson_static.a.gz" "$DIST_DIR/libtdjson_static.a.gz"
cp "$SOURCE_DIR/LICENSE_1_0.txt" "$DIST_DIR/TDLib-LICENSE_1_0.txt"
echo "Reproduced the reviewed TDLib static archive."
