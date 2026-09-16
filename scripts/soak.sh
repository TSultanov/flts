#!/usr/bin/env bash
# Explores past the pinned seed, in release, with a large case count.
#
# Never a gate. Its job is to find the case the deterministic run missed so a
# human can promote it into a named test -- which raises the deterministic floor
# and kills mutants that previously survived. Failure persistence is off by
# design, so the printed seed is the only reproduction recipe. Write it down
# before fixing anything.
set -euo pipefail

cd "$(dirname "$0")/.."

cases="${1:-20000}"
seed=$(od -An -N8 -tu8 < /dev/urandom | tr -d ' ')

echo "soak: seed $seed, $cases cases"
echo "reproduce: PROPTEST_RNG_SEED=$seed PROPTEST_CASES=$cases cargo test --release"

DBSYNC_PROPTEST_SOAK=1 PROPTEST_RNG_SEED="$seed" PROPTEST_CASES="$cases" \
    cargo test --release
