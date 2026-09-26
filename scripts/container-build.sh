#!/bin/sh
set -eu

mode=${1:-}
case "$mode" in
  check|build) ;;
  *)
    echo "usage: $0 check|build" >&2
    exit 64
    ;;
esac

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
context_directory=$(mktemp -d "${TMPDIR:-/tmp}/retract-docker-context.XXXXXX")
cleanup() {
  rm -rf -- "$context_directory"
}
trap cleanup EXIT HUP INT TERM

cd "$repository_root"

# Only paths already present in Git's index can enter Docker. The bytes come
# from the current working tree so edits to tracked files are testable, while
# an untracked export, token, database, archive, or secret is excluded even if
# it sits below src/, docs/, assets/, or another allowed source directory.
git ls-files --cached -z \
  | tar --null --files-from=- --create --file=- \
  | tar --extract --file=- --directory="$context_directory"
: > "$context_directory/.retract-tracked-context"

case "$mode" in
  check)
    docker buildx build \
      --target checks \
      --output type=cacheonly \
      --progress plain \
      "$context_directory"
    ;;
  build)
    mkdir -p "$repository_root/artifacts/frontend"
    docker buildx build \
      --target frontend-artifact \
      --output "type=local,dest=$repository_root/artifacts/frontend" \
      "$context_directory"
    ;;
esac
