#!/bin/sh
# The install script's contract, tested with no network: a locally built
# release dir, a stubbed uname for the platform map, and the real binary.
set -eu
here=$(dirname "$0")
repo=$(cd "$here/.." && pwd)
script="$repo/install.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() { echo "FAIL: $1" >&2; exit 1; }

# A release dir with one asset and its SHA256, built from the release binary.
release="$work/releases"
mkdir -p "$release/bin"
cp "$repo/target/release/wattdeck" "$release/bin/"
tag="v9.9.9"
target="x86_64-unknown-linux-gnu"
tar -czf "$release/wattdeck-$tag-$target.tar.gz" -C "$release/bin" wattdeck
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$release/wattdeck-$tag-$target.tar.gz" \
        | sed "s# .*\$#  wattdeck-$tag-$target.tar.gz#" > "$release/SHA256SUMS.txt"
else
    shasum -a 256 "$release/wattdeck-$tag-$target.tar.gz" \
        | sed "s# .*\$#  wattdeck-$tag-$target.tar.gz#" > "$release/SHA256SUMS.txt"
fi

# 1. A good install, into a dir we control, prints the version.
dest="$work/install"
out=$(WATTDECK_TAG="$tag" WATTDECK_RELEASE_DIR="$release" WATTDECK_INSTALL_DIR="$dest" sh "$script")
[ -x "$dest/wattdeck" ] || fail "no binary in $dest"
case "$out" in
    *"$tag"*) ;;
    *) fail "version line does not name the tag: $out" ;;
esac
case "$out" in
    *"asset wattdeck-$tag-$target.tar.gz, target $target"*) ;;
    *) fail "the script must name the asset it chose: $out" ;;
esac

# 2. A tampered tarball stops the install, and the message names the file.
# The tamper is on a copy: the shared release dir must stay pristine for the
# good-install and PATH cases, which run after this one.
tampered="$work/tampered"
mkdir -p "$tampered"
cp "$release/wattdeck-$tag-$target.tar.gz" "$tampered/"
cp "$release/SHA256SUMS.txt" "$tampered/"
echo "x" >> "$tampered/wattdeck-$tag-$target.tar.gz"
set +e
out=$(WATTDECK_TAG="$tag" WATTDECK_RELEASE_DIR="$tampered" WATTDECK_INSTALL_DIR="$work/never" sh "$script" 2>&1)
rc=$?
set -e
[ "$rc" -ne 0 ] || fail "tampered tarball must fail (rc $rc, out $out)"
case "$out" in
    *SHA256*) ;;
    *) fail "the failure must name SHA256: $out" ;;
esac
[ ! -e "$work/never/wattdeck" ] || fail "a tampered tarball installed a binary"

# 3. An unsupported platform is refused by the target it wanted, with a stubbed uname.
stub="$work/stub"; mkdir -p "$stub"
# shellcheck disable=SC2016
# The single quotes are the point: "$1" is literal text inside the generated
# stub, expanded by the stub's own shell when uname is called, not by this test.
printf '#!/bin/sh\ncase "$1" in -s) echo Linux ;; -m) echo arm64 ;; esac\n' > "$stub/uname"
chmod 755 "$stub/uname"
set +e
out=$(PATH="$stub:$PATH" WATTDECK_TAG="$tag" WATTDECK_RELEASE_DIR="$release" WATTDECK_INSTALL_DIR="$work/never2" sh "$script" 2>&1)
rc=$?
set -e
[ "$rc" -ne 0 ] || fail "Linux/arm64 must be refused (out: $out)"
case "$out" in
    *"x86_64-unknown-linux-gnu"*) ;;
    *) fail "the refusal must name the built targets: $out" ;;
esac
[ ! -e "$work/never2/wattdeck" ] || fail "refused platform installed a binary"

# 4. A PATH that lacks the install dir prints the exact line to add.
dest3="$work/install3"
out=$(PATH="/usr/bin:/bin" WATTDECK_TAG="$tag" WATTDECK_RELEASE_DIR="$release" WATTDECK_INSTALL_DIR="$dest3" sh "$script" 2>&1)
case "$out" in
    *"export PATH=\"$dest3"* | *"export PATH=$dest3"*) ;;
    *) fail "no PATH line printed: $out" ;;
esac
[ -x "$dest3/wattdeck" ] || fail "no binary in $dest3"

echo "install.sh tests pass"
