#!/usr/bin/env bash
# Prints the cache key of the release WebUI build: a hash of every input of the build.
#
# release-cache.yml builds the WebUI on main and saves it under this key, and release.yml
# restores it, so a release does not build the WebUI again. A version bump changes only the
# versions of r3v3rs3 and r3v3rs3-cli in Cargo.lock, which the WebUI does not build, so the
# key leaves those two lines out and the bump commit finds the WebUI of the commit before it.
#
# The two workflows and this script are inputs too: they hold the trunk version and the build
# command, so a change there must not reuse a WebUI that the old command built.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
{
    git ls-files -z -- r3v3rs3-webui r3v3rs3-api Cargo.toml rust-toolchain.toml \
        .github/workflows/release.yml .github/workflows/release-cache.yml \
        .github/scripts/webui-cache-key.sh |
        xargs -0 sha256sum
    awk '
        /^name = "(r3v3rs3|r3v3rs3-cli)"$/ { print; skip = 1; next }
        skip && /^version = / { skip = 0; next }
        { skip = 0; print }
    ' Cargo.lock
} | sha256sum | cut -c1-32
