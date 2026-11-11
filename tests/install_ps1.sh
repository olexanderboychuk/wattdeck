#!/bin/sh
# install.ps1's contract, run on PowerShell Core. There is no Windows machine in
# this session, so the platform map is driven by the env var the script reads
# ($env:PROCESSOR_ARCHITECTURE) and the release assets are local: no network.
# pwsh is found by PWSSH, or in PATH, or in the extracted tarball. When pwsh is
# absent the test says so and passes — it is a Linux-only test, and the Windows
# runner in CI is the real verification.
set -eu
here=$(dirname "$0")
repo=$(cd "$here/.." && pwd)
script="$repo/install.ps1"
pwsh=${PWSSH:-}
if [ -z "$pwsh" ]; then
    if command -v pwsh >/dev/null 2>&1; then
        pwsh=$(command -v pwsh)
    elif [ -x /tmp/opencode/pwsh/pwsh ]; then
        pwsh=/tmp/opencode/pwsh/pwsh
    fi
fi
if [ -z "$pwsh" ]; then
    echo "install.ps1 test skipped: pwsh is not installed (the Windows CI job runs install.ps1 -SelfTest)"
    exit 0
fi

fail() { echo "FAIL: $1" >&2; exit 1; }

# A release dir with one zip and its SHA256, built from the release binary.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
release="$work/releases"
mkdir -p "$release/bin"
cp "$repo/target/release/wattdeck" "$release/bin/wattdeck.exe"
# The tag equals the crate version, so the version rule below is the CI's rule
# (release.yml compares `wattdeck 0.1.0` against the tag with `v` stripped).
tag="v0.1.0"
target="x86_64-pc-windows-msvc"
asset="wattdeck-$tag-$target.zip"
# python3's zipfile stores the file's Unix mode in the archive, so
# Expand-Archive on Linux extracts an executable binary. Compress-Archive on
# Linux drops the exec bit, which is why the CI's zip is built on a Windows
# runner, where there is no exec bit to lose.
( cd "$release/bin" && python3 -m zipfile -c "$release/$asset" wattdeck.exe )
sha256sum "$release/$asset" \
    | sed "s# .*\$#  $asset#" > "$release/SHA256SUMS.txt"

# 1. Self-test: the map and the refusal text.
out=$("$pwsh" -NoProfile -File "$script" -SelfTest 2>&1)
case "$out" in
    *"install.ps1 self-test passes"*) ;;
    *) fail "self-test: $out" ;;
esac

# 2. A good install: names the asset and target, prints the PATH line, prints --version.
dest="$work/install"
out=$(PROCESSOR_ARCHITECTURE=AMD64 "$pwsh" -NoProfile -Command "& '$script' -Tag '$tag' -ReleaseDir '$release' -InstallDir '$dest'" 2>&1)
[ -f "$dest/wattdeck.exe" ] || fail "no binary in $dest (out: $out)"
case "$out" in
    *"asset $asset, target $target"*) ;;
    *) fail "the script must name the asset it chose: $out" ;;
esac
version=$(printf '%s\n' "$out" | tail -1)
[ "$version" = "wattdeck ${tag#v}" ] || fail "the last line is not the version rule: $version"

# 3. A tampered zip stops the install and names SHA256.
tampered="$work/tampered"; mkdir -p "$tampered"
cp "$release/$asset" "$tampered/"
cp "$release/SHA256SUMS.txt" "$tampered/"
echo "x" >> "$tampered/$asset"
set +e
out=$(PROCESSOR_ARCHITECTURE=AMD64 "$pwsh" -NoProfile -Command "& '$script' -Tag '$tag' -ReleaseDir '$tampered' -InstallDir '$work/never'" 2>&1)
rc=$?
set -e
[ "$rc" -ne 0 ] || fail "tampered zip must fail (rc $rc, out $out)"
case "$out" in
    *SHA256*) ;;
    *) fail "the failure must name SHA256: $out" ;;
esac
[ ! -e "$work/never/wattdeck.exe" ] || fail "a tampered zip installed a binary"

# 4. ARM64 is refused by the targets that exist.
set +e
out=$(PROCESSOR_ARCHITECTURE=ARM64 "$pwsh" -NoProfile -Command "& '$script' -Tag '$tag' -ReleaseDir '$release' -InstallDir '$work/never2'" 2>&1)
rc=$?
set -e
[ "$rc" -ne 0 ] || fail "Windows/ARM64 must be refused (out: $out)"
case "$out" in
    *"x86_64-pc-windows-msvc"*) ;;
    *) fail "the refusal must name the built targets: $out" ;;
esac
[ ! -e "$work/never2/wattdeck.exe" ] || fail "refused platform installed a binary"

# 5. A PATH that lacks the install dir prints the setx line.
dest3="$work/install3"
out=$(PATH="/usr/bin:/bin" PROCESSOR_ARCHITECTURE=AMD64 "$pwsh" -NoProfile -Command "& '$script' -Tag '$tag' -ReleaseDir '$release' -InstallDir '$dest3'" 2>&1)
case "$out" in
    *"setx PATH"*"$dest3"*) ;;
    *) fail "no PATH line printed: $out" ;;
esac

echo "install.ps1 tests pass"
