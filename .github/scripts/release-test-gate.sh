#!/usr/bin/env bash
# The test gate of release.yml: decides whether the release must run the tests again.
#
# A release tag points at a version bump commit, usually on top of a docs commit that bumps the
# example versions. These commits change no code, so every commit from the tag back to the
# newest commit that changes code holds the same code, and a green Rust run of any of them
# proves it. When the code commit was pushed and tested before the bump, the gate passes at
# once instead of waiting about 9 minutes for the Rust run of the tag commit; when all of them
# were pushed together (GitHub then runs only the newest one), it waits for that run.
#
#   release-test-gate.sh same-code <commit>   prints the commits that hold the code of <commit>
#   release-test-gate.sh                      the gate; writes tested=true|false to GITHUB_OUTPUT
set -euo pipefail

# Workspace packages whose version line a version bump changes in Cargo.lock.
readonly WORKSPACE_PACKAGES='r3v3rs3|r3v3rs3-cli|r3v3rs3-api|r3v3rs3-webui'
readonly MAX_HOPS=10

# Prints a Cargo.toml without the version line of its [package] or [workspace.package] table.
strip_manifest_version() {
    awk '
        /^\[/ { in_package = ($0 == "[package]" || $0 == "[workspace.package]") }
        in_package && /^version = / { next }
        { print }
    '
}

# Prints a Cargo.lock without the version lines of the workspace packages.
strip_lock_versions() {
    awk -v names="^name = \"(${WORKSPACE_PACKAGES})\"$" '
        $0 ~ names { print; skip = 1; next }
        skip && /^version = / { skip = 0; next }
        { skip = 0; print }
    '
}

# Succeeds when the file differs between the commit and its parent only in version lines.
only_versions_changed() {
    local commit=$1 file=$2 strip=strip_manifest_version
    [ "$file" = Cargo.lock ] && strip=strip_lock_versions
    git cat-file -e "$commit^:$file" 2>/dev/null || return 1
    git cat-file -e "$commit:$file" 2>/dev/null || return 1
    diff -q <(git show "$commit^:$file" | $strip) <(git show "$commit:$file" | $strip) >/dev/null
}

# Succeeds when the commit changes only versions, the changelog or the documentation.
changes_no_code() {
    local commit=$1 file
    git rev-parse -q --verify "$commit^" >/dev/null || return 1
    while IFS= read -r file; do
        case $file in
            CHANGELOG.md | README.md | docs/*) ;;
            Cargo.lock | Cargo.toml | */Cargo.toml) only_versions_changed "$commit" "$file" || return 1 ;;
            *) return 1 ;;
        esac
    done < <(git diff --name-only "$commit^" "$commit")
}

# Prints the given commit and its ancestors down to the newest commit that changes code, one
# per line, newest first. All of them hold the same code.
same_code_commits() {
    local commit hops=0
    commit=$(git rev-parse "$1")
    echo "$commit"
    while [ "$hops" -lt "$MAX_HOPS" ] && changes_no_code "$commit"; do
        commit=$(git rev-parse "$commit^")
        echo "$commit"
        hops=$((hops + 1))
    done
}

# Prints the state of the Rust workflow for the commit: success, running, none or a failure.
rust_run_state() {
    gh run list --repo "$GITHUB_REPOSITORY" --workflow Rust --commit "$1" \
        --json conclusion,status,createdAt \
        --jq 'if length == 0 then "none"
              elif any(.status != "completed") then "running"
              else (sort_by(.createdAt) | last | .conclusion) end'
}

# Prints the combined state of the commits: success when one run is green, running while one
# still runs, none when no commit has a run, else the first failure.
combined_state() {
    local commit state failure="" running=""
    for commit in "$@"; do
        state=$(rust_run_state "$commit")
        case $state in
            success) echo success; return ;;
            running) running=1 ;;
            none) ;;
            *) failure=${failure:-$state} ;;
        esac
    done
    if [ -n "$running" ]; then echo running; elif [ -n "$failure" ]; then echo "$failure"; else echo none; fi
}

# Waits up to 20 minutes for a final combined state of the commits and prints it.
final_state() {
    local state attempt
    for attempt in $(seq 1 40); do
        state=$(combined_state "$@")
        case $state in
            running) ;;
            none) [ "$attempt" -ge 3 ] && { echo none; return; } ;;
            *) echo "$state"; return ;;
        esac
        echo "the Rust runs are $state ($attempt/40)" >&2
        sleep 30
    done
    echo timeout
}

gate() {
    local -a commits
    local state tested=false
    mapfile -t commits < <(same_code_commits "$GITHUB_SHA")
    echo "commits with the code of the tag: ${commits[*]}"
    state=$(final_state "${commits[@]}")
    if [ "$state" = success ]; then
        echo "a Rust run of this code is green, the release skips the tests"
        tested=true
    else
        echo "the Rust runs of this code are $state, the release runs the tests" >&2
    fi
    echo "tested=$tested" >>"$GITHUB_OUTPUT"
}

case ${1:-gate} in
    same-code) same_code_commits "$2" ;;
    gate) gate ;;
    *) echo "usage: $0 [same-code <commit>]" >&2; exit 2 ;;
esac
