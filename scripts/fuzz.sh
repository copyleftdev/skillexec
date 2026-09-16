#!/usr/bin/env bash
# Runs the parser fuzzer under a systemd scope so the ceiling belongs to the kernel, not to the
# fuzzer's own good intentions. The fuzzer is single-threaded by construction; this caps it at
# one core and a fixed memory high-water mark anyway, because a box that is usually busy should
# not have to trust a process to behave.
#
#   ./scripts/fuzz.sh                 # 60s, one seed
#   ./scripts/fuzz.sh 600 4           # 600s across 4 seeds
set -euo pipefail

cd "$(dirname "$0")/.."
SECS=${1:-60}
SEEDS=${2:-1}
CPU=${FUZZ_CPU_QUOTA:-100%}
MEM=${FUZZ_MEM_HIGH:-2G}

cargo build --release --quiet --bin fuzz

status=0
for i in $(seq 1 "$SEEDS"); do
    seed=$(( 0x5EED0000 + i * 7919 ))
    unit="skillfuzz-$$-$i"
    if systemd-run --user --quiet --wait --collect --unit="$unit" \
        -p CPUQuota="$CPU" -p MemoryHigh="$MEM" -p CPUWeight=20 \
        -p WorkingDirectory="$PWD" \
        ./target/release/fuzz --iters 100000000 --seconds "$SECS" --seed "$seed"
    then
        printf '  seed %s: clean\n' "$seed"
    else
        printf '  seed %s: FAILED (see: journalctl --user -u %s)\n' "$seed" "$unit"
        status=1
    fi
done
exit "$status"
