#!/bin/sh
set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$PROJECT_ROOT"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "Unsigned Retract packages must be built on Apple-silicon macOS." >&2
  exit 1
fi

for tool in codesign ditto file node npm otool rg shasum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing required release tool: $tool" >&2
    exit 1
  fi
done

RELEASE_TMP=$(mktemp -d "${TMPDIR:-/tmp}/retract-release.XXXXXX")
trap 'rm -rf "$RELEASE_TMP"' EXIT HUP INT TERM

APP_PATH="$PROJECT_ROOT/src-tauri/target/release/bundle/macos/Retract.app"
APP_BINARY_REL="Contents/MacOS/retract"
TDLIB_REL="Contents/Resources/lib/libtdjson.dylib"
VERSION=$(node -p 'require("./package.json").version')
ARCHIVE_NAME="Retract-v${VERSION}-macos-arm64.app.zip"
ARCHIVE_PATH="$RELEASE_TMP/$ARCHIVE_NAME"
CHECKSUM_PATH="$RELEASE_TMP/$ARCHIVE_NAME.sha256"
MANIFEST_PATH="$RELEASE_TMP/$ARCHIVE_NAME.manifest.json"
RELEASE_DIR="$PROJECT_ROOT/artifacts/release"

verify_app() {
  verify_path=$1
  verify_details="$RELEASE_TMP/codesign-details.txt"
  verify_deps="$RELEASE_TMP/tdlib-dependencies.txt"

  sh scripts/verify-app-contents.sh "$verify_path"

  codesign --verify --deep --strict "$verify_path"
  codesign -dv --verbose=4 "$verify_path" >"$verify_details" 2>&1
  if ! rg --line-regexp 'Signature=adhoc' "$verify_details" >/dev/null; then
    echo "Retract.app is not ad-hoc signed." >&2
    exit 1
  fi
  if ! rg '^CodeDirectory .* flags=.*\(.*runtime.*\)' "$verify_details" >/dev/null; then
    echo "Retract.app does not enable the hardened runtime." >&2
    exit 1
  fi

  binary_description=$(file "$verify_path/$APP_BINARY_REL")
  case "$binary_description" in
    *"Mach-O 64-bit executable arm64"*) ;;
    *)
      echo "Retract executable is not arm64-only: $binary_description" >&2
      exit 1
      ;;
  esac
  case "$binary_description" in
    *universal*)
      echo "Retract executable unexpectedly contains multiple architectures." >&2
      exit 1
      ;;
  esac

  expected_tdlib_sha=$(sed -n '2s/^sha256=\([^ ]*\).*/\1/p' vendor/tdlib-dist/build-stamp.txt)
  actual_tdlib_sha=$(shasum -a 256 "$verify_path/$TDLIB_REL" | cut -d ' ' -f 1)
  if [ -z "$expected_tdlib_sha" ] || [ "$actual_tdlib_sha" != "$expected_tdlib_sha" ]; then
    echo "Bundled TDLib does not match its reviewed build stamp." >&2
    exit 1
  fi

  otool -L "$verify_path/$TDLIB_REL" >"$verify_deps"
  sed -n '2,$s/^[[:space:]]*\([^ ]*\).*/\1/p' "$verify_deps" | while IFS= read -r dependency; do
    case "$dependency" in
      @rpath/libtdjson.dylib|/usr/lib/*|/System/Library/*) ;;
      *)
        echo "TDLib has a non-system runtime dependency: $dependency" >&2
        exit 1
        ;;
    esac
  done
}

npm run tauri build -- --bundles app
npm run verify:production-bundle -- --existing
verify_app "$APP_PATH"

node scripts/release-metadata.mjs "$MANIFEST_PATH"
ditto -c -k --sequesterRsrc --keepParent "$APP_PATH" "$ARCHIVE_PATH"
archive_sha=$(shasum -a 256 "$ARCHIVE_PATH" | cut -d ' ' -f 1)
printf '%s  %s\n' "$archive_sha" "$ARCHIVE_NAME" >"$CHECKSUM_PATH"

EXTRACT_DIR="$RELEASE_TMP/expanded"
mkdir -p "$EXTRACT_DIR"
ditto -x -k "$ARCHIVE_PATH" "$EXTRACT_DIR"
verify_app "$EXTRACT_DIR/Retract.app"

mkdir -p "$RELEASE_DIR"
cp "$ARCHIVE_PATH" "$CHECKSUM_PATH" "$MANIFEST_PATH" "$RELEASE_DIR/"

echo "Unsigned Retract preview artifacts:"
echo "  $RELEASE_DIR/$ARCHIVE_NAME"
echo "  $RELEASE_DIR/$ARCHIVE_NAME.sha256"
echo "  $RELEASE_DIR/$ARCHIVE_NAME.manifest.json"
