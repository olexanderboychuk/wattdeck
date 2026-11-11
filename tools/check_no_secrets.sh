#!/bin/sh
# Local-only secret check. The values are arguments, never written in this file,
# so the checker itself cannot carry a secret into the public tree.
#
# Two scopes:
#   the tracked tree at HEAD  — what a push would ship
#   the history reachable from HEAD — a value deleted in a later commit is still
#   in the commits a push publishes
#
# Scope is HEAD, not `--all`: `master` is the private archive that is never
# pushed, so its history is not what this check protects. Run it on the branch
# that is going public.
set -eu
if [ "$#" -eq 0 ]; then
    echo "usage: check_no_secrets.sh <value> [value…]" >&2
    exit 2
fi
fail=0
for value in "$@"; do
    if git grep -n -- "$value" HEAD; then
        echo "found $value in the tracked tree at HEAD" >&2
        fail=1
    fi
    # The pickaxe (-S) lists commits that changed the number of occurrences;
    # `git log -p` prints the diffs, so the value is visible in the output.
    if [ -n "$(git log -p -S "$value" HEAD 2>/dev/null | grep -F "$value" | head -1)" ]; then
        echo "found $value in the history reachable from HEAD" >&2
        fail=1
    fi
done
if [ "$fail" -eq 0 ]; then
    echo "clean: none of the $# values is at HEAD or in its history"
fi
exit "$fail"
