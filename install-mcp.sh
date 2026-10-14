#!/bin/sh
# Installs the newest r3v3rs3-mcp binary from the GitHub releases of r3v3rs3.
# Runs on Linux and macOS. Pass a release tag with --version v1.7.9, or set
# INSTALL_DIR (the default is /usr/local/bin).
set -eu

REPO="KilimcininKorOglu/r3v3rs3"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
VERSION=""

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || { echo "--version needs a value like v1.7.9" >&2; exit 1; }
            VERSION="$2"
            shift 2
            ;;
        --help|-h)
            echo "usage: install-mcp.sh [--version vX.Y.Z]"
            echo "  INSTALL_DIR overrides the target directory (default /usr/local/bin)"
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            exit 1
            ;;
    esac
done

case "$(uname -s)/$(uname -m)" in
    Linux/x86_64)  ASSET="r3v3rs3-mcp-x86_64-unknown-linux-musl.tar.gz" ;;
    Linux/aarch64|Linux/arm64) ASSET="r3v3rs3-mcp-aarch64-unknown-linux-musl.tar.gz" ;;
    Darwin/arm64)  ASSET="r3v3rs3-mcp-aarch64-apple-darwin.tar.gz" ;;
    Darwin/x86_64) ASSET="r3v3rs3-mcp-x86_64-apple-darwin.tar.gz" ;;
    *)
        echo "r3v3rs3-mcp ships no archive for $(uname -s)/$(uname -m). Windows uses install-mcp.ps1." >&2
        exit 1
        ;;
esac

if [ -z "$VERSION" ]; then
    VERSION="$(curl -fsSL -H 'user-agent: r3v3rs3-mcp installer' \
        "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
    [ -n "$VERSION" ] || { echo "the latest release tag could not be read" >&2; exit 1; }
fi
case "$VERSION" in
    v*) ;;
    *) VERSION="v$VERSION" ;;
esac

BASE="https://github.com/$REPO/releases/download/$VERSION"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "Downloading r3v3rs3-mcp $VERSION..."
curl -fsSL -H 'user-agent: r3v3rs3-mcp installer' "$BASE/SHA256SUMS" -o "$WORK/SHA256SUMS"
curl -fsSL -H 'user-agent: r3v3rs3-mcp installer' "$BASE/$ASSET" -o "$WORK/$ASSET"

EXPECTED="$(sed 's/[[:space:]]\.\// /' "$WORK/SHA256SUMS" | awk -v a="$ASSET" '$2 == a {print $1}')"
[ -n "$EXPECTED" ] || { echo "$ASSET is not in the checksums of the release" >&2; exit 1; }
if command -v sha256sum > /dev/null 2>&1; then
    ACTUAL="$(sha256sum "$WORK/$ASSET" | cut -d' ' -f1)"
else
    ACTUAL="$(shasum -a 256 "$WORK/$ASSET" | cut -d' ' -f1)"
fi
[ "$ACTUAL" = "$EXPECTED" ] || { echo "the sha256 digest of $ASSET does not match the checksums of the release" >&2; exit 1; }

mkdir -p "$WORK/unpack"
tar -C "$WORK/unpack" -xzf "$WORK/$ASSET" r3v3rs3-mcp

if [ ! -w "$INSTALL_DIR" ]; then
    echo "$INSTALL_DIR is not writable; using sudo."
    sudo install -m 755 "$WORK/unpack/r3v3rs3-mcp" "$INSTALL_DIR/r3v3rs3-mcp"
else
    install -m 755 "$WORK/unpack/r3v3rs3-mcp" "$INSTALL_DIR/r3v3rs3-mcp"
fi

# The login shells read ~/.profile, zsh reads ~/.zshrc, so the directory lands in the
# PATH of every common setup. A directory that is already in the PATH stays untouched.
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        LINE="export PATH=\"$INSTALL_DIR:\$PATH\""
        for RC in "${HOME}/.profile" "${HOME}/.zshrc"; do
            [ -f "$RC" ] || continue
            if ! grep -qF "$INSTALL_DIR" "$RC" 2>/dev/null; then
                printf '\n# added by the r3v3rs3-mcp installer\n%s\n' "$LINE" >> "$RC"
                echo "$INSTALL_DIR was added to your PATH through $RC."
            fi
        done
        ;;
esac
echo "r3v3rs3-mcp $VERSION is installed in $INSTALL_DIR. Run 'r3v3rs3-mcp update' to upgrade it later."
