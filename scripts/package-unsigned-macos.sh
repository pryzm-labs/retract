#!/bin/sh
set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$PROJECT_ROOT"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "Unsigned Retract packages must be built on Apple-silicon macOS." >&2
  exit 1
fi

for tool in codesign ditto file grep nm node npm otool shasum strings; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing required release tool: $tool" >&2
    exit 1
  fi
done

RELEASE_TMP=$(mktemp -d "${TMPDIR:-/tmp}/retract-release.XXXXXX")
trap 'rm -rf "$RELEASE_TMP"' EXIT HUP INT TERM

# Native dependencies such as vendored OpenSSL record their Cargo output prefix
# in the compiled binary. Build the distributable in a private, disposable
# target directory outside the developer's home so an unsigned package cannot
# reveal the builder's account name or checkout location.
CARGO_TARGET_DIR="$RELEASE_TMP/target"
export CARGO_TARGET_DIR

APP_PATH="$CARGO_TARGET_DIR/release/bundle/macos/Retract.app"
APP_BINARY_REL="Contents/MacOS/retract"
VERSION=$(node -p 'require("./package.json").version')
ARCHIVE_NAME="Retract-v${VERSION}-macos-arm64.app.zip"
ARCHIVE_PATH="$RELEASE_TMP/$ARCHIVE_NAME"
CHECKSUM_PATH="$RELEASE_TMP/$ARCHIVE_NAME.sha256"
MANIFEST_PATH="$RELEASE_TMP/$ARCHIVE_NAME.manifest.json"
RELEASE_DIR="$PROJECT_ROOT/artifacts/release"

verify_app() {
  verify_path=$1
  verify_details="$RELEASE_TMP/codesign-details.txt"

  sh scripts/verify-app-contents.sh "$verify_path"

  codesign --verify --deep --strict "$verify_path"
  codesign -dv --verbose=4 "$verify_path" >"$verify_details" 2>&1
  sh scripts/verify-codesign-details.sh "$verify_details"

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

  if otool -L "$verify_path/$APP_BINARY_REL" | grep -q 'libtdjson'; then
    echo "Retract must not load TDLib dynamically." >&2
    exit 1
  fi
  if ! nm -gU "$verify_path/$APP_BINARY_REL" | grep -q ' _td_json_client_create$'; then
    echo "Retract executable does not contain the statically linked TDLib API." >&2
    exit 1
  fi
  if strings "$verify_path/$APP_BINARY_REL" | grep -E -q '/Users/[^/]+/|/home/[^/]+/'; then
    echo "Retract executable exposes a developer home-directory path." >&2
    exit 1
  fi

  verify_native_dependencies "$verify_path/$APP_BINARY_REL" "$RELEASE_TMP/app-dependencies.txt"
}

verify_native_dependencies() {
  native_path=$1
  dependency_report=$2
  otool -L "$native_path" >"$dependency_report"
  sed -n '2,$s/^[[:space:]]*\([^ ]*\).*/\1/p' "$dependency_report" | while IFS= read -r dependency; do
    case "$dependency" in
      /usr/lib/*|/System/Library/*) ;;
      *)
        echo "Native artifact has a non-reviewed runtime dependency: $dependency" >&2
        exit 1
        ;;
    esac
  done
}

# Rust dependencies can embed source locations in panic and tracing strings
# even in an optimized binary. Remap the complete developer home before Cargo
# fingerprints or compiles anything so release artifacts never reveal the
# builder's account name or local checkout layout.
REMAP_HOME="--remap-path-prefix=$HOME=/usr/src/retract-home"
if [ -n "${RUSTFLAGS:-}" ]; then
  RUSTFLAGS="$RUSTFLAGS $REMAP_HOME"
else
  RUSTFLAGS=$REMAP_HOME
fi
export RUSTFLAGS

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
