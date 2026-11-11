#!/bin/sh
# wattdeck installer. POSIX sh: runs under bash, zsh, dash, and through
# `curl -sSL <raw url> | sh`.
#
# Detects OS and CPU, maps them to a built target, downloads that release
# asset, verifies its SHA256 before extracting, installs the binary, and
# prints the version it installed. No sudo, no root, no silent guesses.
set -eu

REPO="olexanderboychuk/wattdeck"
INSTALL_DIR="${WATTDECK_INSTALL_DIR:-$HOME/.local/bin}"
# Test seams, documented as test-only in the README's troubleshooting:
# a directory of release assets, and a tag, so the shell test needs no network.
RELEASE_DIR="${WATTDECK_RELEASE_DIR:-}"
TAG="${WATTDECK_TAG:-}"

die() { echo "wattdeck install failed: $1" >&2; exit 1; }

# The targets this project builds. A combination outside the list is refused by
# name, so a user on an unsupported machine learns what exists, not a guess.
os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
    Linux:x86_64|Linux:amd64) target="x86_64-unknown-linux-gnu" ;;
    Darwin:arm64|Darwin:aarch64) target="aarch64-apple-darwin" ;;
    *) die "no wattdeck build for $os on $arch. Built targets: x86_64-unknown-linux-gnu (Linux x86_64), aarch64-apple-darwin (macOS Apple silicon), x86_64-pc-windows-msvc (Windows x86_64)." ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ -n "$TAG" ]; then
    tag="$TAG"
else
    tag=$(curl -fsSL -H 'Accept: application/vnd.github+json' \
        "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
    [ -n "$tag" ] || die "no latest release in $REPO (one GitHub API request per install: 60/hour per IP)"
fi

asset="wattdeck-$tag-$target.tar.gz"
if [ -n "$RELEASE_DIR" ]; then
    [ -f "$RELEASE_DIR/$asset" ] || die "$asset is not in $RELEASE_DIR"
    cp "$RELEASE_DIR/$asset" "$tmp/"
    [ -f "$RELEASE_DIR/SHA256SUMS.txt" ] || die "SHA256SUMS.txt is not in $RELEASE_DIR"
    cp "$RELEASE_DIR/SHA256SUMS.txt" "$tmp/"
else
    curl -fsSL -o "$tmp/$asset" "https://github.com/$REPO/releases/download/$tag/$asset"
    curl -fsSL -o "$tmp/SHA256SUMS.txt" "https://github.com/$REPO/releases/download/$tag/SHA256SUMS.txt"
fi

# Verify the hash before extracting: unverified bytes are never run.
if command -v sha256sum >/dev/null 2>&1; then
    hash_cmd="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
    hash_cmd="shasum -a 256"
else
    die "no SHA256 tool found (sha256sum or shasum); install the binary from source"
fi
want=$(grep "  $asset\$" "$tmp/SHA256SUMS.txt" | cut -d' ' -f1)
[ -n "$want" ] || die "$asset is not listed in SHA256SUMS.txt"
got=$($hash_cmd "$tmp/$asset" | cut -d' ' -f1)
[ "$got" = "$want" ] || die "SHA256 mismatch for $asset: expected $want, got $got"

tar -xzf "$tmp/$asset" -C "$tmp"
[ -x "$tmp/wattdeck" ] || [ -f "$tmp/wattdeck" ] || die "$asset did not contain the wattdeck binary"

[ -d "$INSTALL_DIR" ] || mkdir -p "$INSTALL_DIR"
[ -w "$INSTALL_DIR" ] || die "$INSTALL_DIR is not writable; set WATTDECK_INSTALL_DIR to a directory you can write"
cp "$tmp/wattdeck" "$INSTALL_DIR/wattdeck"
chmod 755 "$INSTALL_DIR/wattdeck"

echo "installed $INSTALL_DIR/wattdeck (asset $asset, target $target)"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "add it to PATH: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
"$INSTALL_DIR/wattdeck" --version
