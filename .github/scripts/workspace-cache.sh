#!/usr/bin/env bash
# Keeps the release build of the server library crates between workflow runs.
#
#   workspace-cache.sh restore   run after the cache restore: makes the restored crates fresh
#   workspace-cache.sh record    run after the build: records the commit of the cached crates
#
# The library crates (r3v3rs3-core, -proxy, -platform, -server, -admin) have a fixed package
# version, so their compiled artifacts stay valid across a version bump. Cargo decides from file
# times whether a crate is fresh, and a checkout gives every file the time of the checkout, which
# is newer than any cached artifact. `restore` therefore sets every tracked file that did not
# change since the cached commit to an old time and leaves the changed files at the checkout
# time, so Cargo compiles exactly the crates whose sources changed.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
readonly COMMIT_FILE=.workspace-cache/commit
# Older than any artifact that a workflow run builds.
readonly OLD_TIME=200001010000

restore() {
    if [[ ! -f $COMMIT_FILE ]]; then
        echo "no cached crates: the build compiles every crate"
        return
    fi
    local cached
    cached=$(<"$COMMIT_FILE")
    if ! git cat-file -e "$cached^{commit}" 2>/dev/null; then
        echo "the cached commit $cached is not in the history: the build compiles every crate"
        rm -rf .workspace-cache
        return
    fi
    local changed
    changed=$(git diff --name-only "$cached" HEAD --)
    git ls-files -z | xargs -0 touch -c -t "$OLD_TIME" --
    if [[ -n $changed ]]; then
        printf '%s\n' "$changed" | while IFS= read -r file; do
            [[ -e $file ]] && touch -c -- "$file"
        done
    fi
    echo "cached crates of $cached; changed files since then:"
    printf '%s\n' "${changed:-(none)}"
}

record() {
    mkdir -p .workspace-cache
    git rev-parse HEAD > "$COMMIT_FILE"
}

case "${1:-}" in
    restore) restore ;;
    record) record ;;
    *)
        echo "usage: $0 restore|record" >&2
        exit 2
        ;;
esac
