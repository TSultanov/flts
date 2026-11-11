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

# A mutant can make a test allocate without bound: one in the arena reached
# 18 GB resident and tripped the kernel OOM killer, taking the sweep with it.
#
# The cap has to be per process, because a cgroup cap over the whole sweep kills
# the sweep rather than the runaway. But per process only helps if JOBS * VMAX
# stays under the RAM actually free -- a 16 GB cap across 4 jobs is a 64 GB
# ceiling on a 30 GB machine, which is not a cap at all. That mistake is what
# the two defaults below exist to prevent, so change them together.
#
# rustc builds this crate inside 2 GB of address space and the suite needs far
# less, so 2 GB is generous for the honest work and quick to reject a runaway.
# ulimit -v is inherited by every child, so the test binary dies on its own
# failed allocation and cargo-mutants records the mutant as caught -- the right
# answer, since a mutation that exhausts memory is one the suite detected.
# --workspace is load-bearing, not tidiness. Without it cargo mutants sweeps
# only the package in the current directory, so dbsync-macros -- where all the
# codegen lives -- would report zero mutants and the gate would pass by looking
# at nothing. default-members governs cargo test, not this.
jobs="${DBSYNC_MUTANTS_JOBS:-4}"
ulimit -v "${DBSYNC_MUTANTS_VMAX:-2000000}"

PROPTEST_CASES="${PROPTEST_CASES:-64}" cargo mutants --workspace -j "$jobs" "$@"
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

# Scope the comparison to the files this sweep actually covered, or a targeted
# run (cargo mutants -f ...) would call every justification for an untouched
# file stale, having never generated the mutant to begin with.
swept=$(mktemp) || exit 70
trap 'rm -f "$listed" "$survived" "$swept"' EXIT
cat mutants.out/caught.txt "$missed" mutants.out/unviable.txt mutants.out/timeout.txt \
    2>/dev/null | sed 's/:.*//' | sort -u > "$swept"

grep -vE '^[[:space:]]*(#|$)' "$justified" | grep -Ff "$swept" | sort -u > "$listed"
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
