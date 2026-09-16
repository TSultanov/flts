#!/usr/bin/env bash
# The deterministic mutation sweep.
#
# PROPTEST_CASES is lowered here because the whole suite runs once per mutant.
# The seed stays pinned by .cargo/config.toml, so a mutant is never a flaky
# survivor: classification is a pure function of (source, PROPTEST_CASES).
#
# That determinism holds only for a fixed proptest version and fixed strategy
# definitions. Editing a strategy reshuffles every case downstream of it, so a
# recorded survivor list is a statement about one commit and one Cargo.lock.
#
# Fails on a new unjustified survivor AND on a stale justification -- a mutant
# that is listed as justified but no longer survives, meaning its reason may be
# obsolete. exclude_re cannot detect that, because an excluded mutant is never
# generated and so can never be observed to have become killable.
set -uo pipefail

cd "$(dirname "$0")/.."

PROPTEST_CASES="${PROPTEST_CASES:-64}" cargo mutants -j 6 "$@"
swept=$?

case "$swept" in
    0 | 2 | 3) ;;
    4) echo "baseline is already failing; nothing was measured" >&2; exit 4 ;;
    *) echo "sweep did not run (exit $swept)" >&2; exit "$swept" ;;
esac

justified=.cargo/mutants-justified.txt
missed=mutants.out/missed.txt

[ -f "$missed" ] || { echo "no $missed; the sweep did not finish" >&2; exit 70; }

listed=$(mktemp) && survived=$(mktemp) || exit 70
trap 'rm -f "$listed" "$survived"' EXIT

grep -vE '^[[:space:]]*(#|$)' "$justified" | sort -u > "$listed"
sort -u "$missed" > "$survived"

status=0

unjustified=$(comm -13 "$listed" "$survived")
if [ -n "$unjustified" ]; then
    echo "survivors with no written justification:"
    echo "$unjustified"
    status=1
fi

stale=$(comm -23 "$listed" "$survived")
if [ -n "$stale" ]; then
    echo "justifications for mutants that no longer survive:"
    echo "$stale"
    status=1
fi

if [ "$swept" = 3 ]; then
    echo "some mutants timed out; see mutants.out/timeout.txt"
fi

[ "$status" = 0 ] && echo "every mutant is caught or justified"
exit "$status"
