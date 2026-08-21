#!/bin/sh
set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SOURCE_DIR="$PROJECT_ROOT/vendor/tdlib-source"
BUILD_DIR="$SOURCE_DIR/build-retract"
DIST_DIR="$PROJECT_ROOT/vendor/tdlib-dist"
OUTPUT_LIBRARY="$DIST_DIR/libtdjson.dylib"
STAMP_FILE="$DIST_DIR/build-stamp.txt"
PINNED_COMMIT="e0943d068ce90b5010f1aea946e6901e25b43bf6"
TDLIB_VERSION="1.8.64"
DEPLOYMENT_TARGET="12.0"
BUILD_ARCH=$(uname -m)
EXPECTED_STAMP="tdlib=$TDLIB_VERSION commit=$PINNED_COMMIT arch=$BUILD_ARCH macos=$DEPLOYMENT_TARGET"

if [ -f "$OUTPUT_LIBRARY" ] && [ -f "$DIST_DIR/TDLib-LICENSE_1_0.txt" ] && [ -f "$STAMP_FILE" ] && [ "$(sed -n '1p' "$STAMP_FILE")" = "$EXPECTED_STAMP" ] && command -v shasum >/dev/null 2>&1; then
  STAMPED_SHA=$(sed -n '2s/^sha256=\([^ ]*\).*/\1/p' "$STAMP_FILE")
  ACTUAL_SHA=$(shasum -a 256 "$OUTPUT_LIBRARY" | cut -d ' ' -f 1)
  if [ -n "$STAMPED_SHA" ] && [ "$ACTUAL_SHA" = "$STAMPED_SHA" ]; then
    echo "TDLib $TDLIB_VERSION is ready for Retract ($BUILD_ARCH, SHA-256 verified)."
    exit 0
  fi
  echo "The bundled TDLib digest changed; rebuilding it from the pinned source revision."
fi

for tool in git cmake gperf install_name_tool shasum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing required TDLib build tool: $tool" >&2
    echo "On macOS, install Xcode Command Line Tools plus cmake, gperf, and openssl@3." >&2
    exit 1
  fi
done

if [ -n "${OPENSSL_ROOT_DIR:-}" ]; then
  OPENSSL_ROOT="$OPENSSL_ROOT_DIR"
elif [ -d /opt/homebrew/opt/openssl@3 ]; then
  OPENSSL_ROOT=/opt/homebrew/opt/openssl@3
elif [ -d /usr/local/opt/openssl@3 ]; then
  OPENSSL_ROOT=/usr/local/opt/openssl@3
else
  echo "OpenSSL 3 was not found. Install openssl@3 with Homebrew or set OPENSSL_ROOT_DIR." >&2
  exit 1
fi

if [ ! -d "$SOURCE_DIR/.git" ]; then
  echo "Downloading official TDLib source at the Retract-pinned revision..."
  git clone --filter=blob:none --no-checkout https://github.com/tdlib/td.git "$SOURCE_DIR"
fi

if ! git -C "$SOURCE_DIR" cat-file -e "$PINNED_COMMIT^{commit}" 2>/dev/null; then
  echo "Fetching pinned TDLib revision $PINNED_COMMIT..."
  git -C "$SOURCE_DIR" fetch --depth 1 origin "$PINNED_COMMIT"
fi

git -C "$SOURCE_DIR" checkout --detach --quiet "$PINNED_COMMIT"
ACTUAL_COMMIT=$(git -C "$SOURCE_DIR" rev-parse HEAD)
if [ "$ACTUAL_COMMIT" != "$PINNED_COMMIT" ]; then
  echo "TDLib source verification failed: expected $PINNED_COMMIT, found $ACTUAL_COMMIT" >&2
  exit 1
fi

mkdir -p "$BUILD_DIR" "$DIST_DIR"
echo "Configuring TDLib $TDLIB_VERSION for $BUILD_ARCH..."
cmake -S "$SOURCE_DIR" -B "$BUILD_DIR" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES="$BUILD_ARCH" \
  -DCMAKE_OSX_DEPLOYMENT_TARGET="$DEPLOYMENT_TARGET" \
  -DOPENSSL_ROOT_DIR="$OPENSSL_ROOT" \
  -DOPENSSL_USE_STATIC_LIBS=TRUE \
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON

echo "Building TDLib. The first build can take several minutes..."
cmake --build "$BUILD_DIR" --config Release --target tdjson --parallel "${TDLIB_BUILD_JOBS:-4}"

BUILT_LIBRARY="$BUILD_DIR/libtdjson.dylib"
if [ ! -f "$BUILT_LIBRARY" ]; then
  echo "TDLib build completed without producing $BUILT_LIBRARY" >&2
  exit 1
fi

cp -fL "$BUILT_LIBRARY" "$OUTPUT_LIBRARY"
install_name_tool -id @rpath/libtdjson.dylib "$OUTPUT_LIBRARY"
cp -f "$SOURCE_DIR/LICENSE_1_0.txt" "$DIST_DIR/TDLib-LICENSE_1_0.txt"

{
  echo "$EXPECTED_STAMP"
  echo "sha256=$(shasum -a 256 "$OUTPUT_LIBRARY" | cut -d ' ' -f 1) file=libtdjson.dylib"
} > "$STAMP_FILE"

echo "Bundled TDLib is ready at $OUTPUT_LIBRARY"
