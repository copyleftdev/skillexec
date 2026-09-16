#!/usr/bin/env bash
# TLC cross-check of the .skill graph model.
#
# Every run declares its expected outcome. The canary configs disable exactly one
# validator check each, and this script FAILS if a canary passes -- a suite whose
# canaries also pass is a suite that checked nothing.
#
# Bounded on purpose: capped workers and heap, niced, a per-run timeout, and a fresh
# hermetic metadir so a stale states/ directory can never poison a verdict.
set -euo pipefail
cd "$(dirname "$0")"

JAR="${TLA2TOOLS:-$HOME/.local/lib/tla2tools.jar}"
WORKERS="${TLC_WORKERS:-4}"
HEAP="${TLC_HEAP:-4g}"
LIMIT="${TLC_TIMEOUT:-600}"

tlc() { # cfg module
    timeout "$LIMIT" nice -n 19 java -Xmx"$HEAP" -XX:+UseParallelGC \
        -cp "$JAR" tlc2.TLC -nowarning -deadlock -workers "$WORKERS" \
        -cleanup -metadir "$(mktemp -d)" -config "$1" "$2" 2>&1 || true
}

expect_holds() { # cfg module note
    if tlc "$1" "$2" | grep -q "No error has been found"; then
        printf '  ok        %-28s %s\n' "$1" "$3"
    else
        printf '  FAIL      %-28s expected to hold\n' "$1"; exit 1
    fi
}

expect_violated() { # cfg module note
    if tlc "$1" "$2" | grep -qiE "is violated|Temporal properties were violated"; then
        printf '  ok        %-28s %s\n' "$1" "$3"
    else
        printf '  FAIL      %-28s canary did not fire -- the property is vacuous\n' "$1"; exit 1
    fi
}

echo "SkillTree -- is the one forward pass equivalent to GRAPH.md §9?"
expect_holds    SkillTree.cfg              SkillTree.tla  "Validator <=> Semantic, all 5-node graphs"
expect_holds    TreeRedundancy.cfg         SkillTree.tla  "pre-order scan implies parent < self"
expect_violated TreeCanaryParentBefore.cfg SkillTree.tla  "parent < self alone permits a forest"
expect_violated TreeCanaryPreOrderScan.cfg SkillTree.tla  "dropping the scan admits split subtrees"
expect_violated TreeCanaryTier.cfg         SkillTree.tla  "dropping tier admits a hot child"
expect_violated TreeCanaryRootTier.cfg     SkillTree.tla  "dropping root tier admits a cold root"

echo "SkillSeq -- what the dst > src trick buys, and what it costs"
expect_holds    SkillSeq.cfg               SkillSeq.tla   "forward implies acyclic"
expect_violated SeqCanaryConverse.cfg      SkillSeq.tla   "acyclic does NOT imply forward (documented restriction)"

echo "SkillEdges -- does activation terminate?"
expect_violated SkillEdges.cfg             SkillEdges.tla "without the union check, ALT alone cycles"
expect_violated EdgesCanaryPiecewise.cfg   SkillEdges.tla "per-relation acyclicity is not enough"
expect_holds    EdgesFixed.cfg             SkillEdges.tla "with the union check, activation terminates"

echo "SkillGraph -- the tree and the obligations, joined"
expect_holds    SkillGraph.cfg             SkillGraph.tla "composed claim holds"
expect_violated GraphWithDescent.cfg       SkillGraph.tla "obligation into an ancestor cycles with descent (open)"

echo
echo "specs passed"
