#!/usr/bin/env bash
# Every check runs; the script fails if any did. `cmd && echo ok` is deliberately avoided —
# it swallows a non-zero status and prints green over red.
set -euo pipefail

cd "$(dirname "$0")/.."
fail=0
run() {
    local name=$1; shift
    if "$@" >/tmp/gate.$$.log 2>&1; then
        printf '  PASS  %s\n' "$name"
    else
        printf '  FAIL  %s\n' "$name"
        tail -30 /tmp/gate.$$.log
        fail=1
    fi
    rm -f /tmp/gate.$$.log
}

run "fmt"     cargo fmt --check
run "clippy"  cargo clippy --all-targets -- -D warnings
run "test"    cargo test --all
run "example" cargo run -q --example dump_minimal

# The spec's worked example must be the reference writer's actual output.
expected=$(cargo run -q --example dump_minimal 2>/dev/null | head -1 | cut -c1-58)
if grep -qF "$expected" docs/SPEC.md; then
    printf '  PASS  spec hexdump matches the writer\n'
else
    printf '  FAIL  spec hexdump has drifted from the writer\n'
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    printf '\ngate FAILED\n'
    exit 1
fi
printf '\ngate passed\n'
