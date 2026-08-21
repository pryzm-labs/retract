#!/bin/sh
set -eu

APP_PATH=${1:-}
if [ -z "$APP_PATH" ] || [ ! -d "$APP_PATH" ]; then
  echo "Usage: $0 /path/to/Retract.app" >&2
  exit 2
fi

symbolic_link=$(find "$APP_PATH" -type l -print -quit)
if [ -n "$symbolic_link" ]; then
  echo "Symbolic links are not permitted in Retract.app: $symbolic_link" >&2
  exit 1
fi

find "$APP_PATH" -mindepth 1 -print | while IFS= read -r absolute_path; do
  relative_path=${absolute_path#"$APP_PATH"/}
  case "$relative_path" in
    Contents|Contents/MacOS|Contents/Resources|Contents/Resources/lib|Contents/Resources/licenses|Contents/_CodeSignature)
      if [ ! -d "$absolute_path" ]; then
        echo "Expected an app-bundle directory: $relative_path" >&2
        exit 1
      fi
      ;;
    Contents/Info.plist|Contents/MacOS/retract|Contents/Resources/icon.icns|Contents/Resources/lib/libtdjson.dylib|Contents/Resources/licenses/TDLib-LICENSE_1_0.txt|Contents/Resources/licenses/TDLib-build-stamp.txt|Contents/_CodeSignature/CodeResources)
      if [ ! -f "$absolute_path" ]; then
        echo "Expected a regular app-bundle file: $relative_path" >&2
        exit 1
      fi
      ;;
    *)
      echo "Unexpected app-bundle path: $relative_path" >&2
      exit 1
      ;;
  esac
done

for required_path in \
  Contents/Info.plist \
  Contents/MacOS/retract \
  Contents/Resources/icon.icns \
  Contents/Resources/lib/libtdjson.dylib \
  Contents/Resources/licenses/TDLib-LICENSE_1_0.txt \
  Contents/Resources/licenses/TDLib-build-stamp.txt \
  Contents/_CodeSignature/CodeResources
do
  if [ ! -f "$APP_PATH/$required_path" ]; then
    echo "Missing required app-bundle file: $required_path" >&2
    exit 1
  fi
done

echo "Retract.app contains only the reviewed release files."
