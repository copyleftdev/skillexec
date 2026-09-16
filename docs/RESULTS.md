# Measured results

Numbers here are reproducible from this tree. Nothing is estimated.

```sh
find ~/.claude ~/Project -name SKILL.md -not -path '*/node_modules/*' | sort -u > corpus.txt
cargo run --release --bin skillc -- corpus corpus.txt
./scripts/fuzz.sh 600 4
./specs/check.sh
```

## Compiling the corpus

8,776 `SKILL.md` files, every one found on one workstation. Single-threaded, 1.0 s wall.

| | |
|---|---|
| compiled | 8,776 (100.00%) |
| opened and fully verified | 8,776 (100.00%) |
| round-trip **byte-exact** | 8,763 (99.85%) |
| round-trip modulo trailing whitespace | 8,776 (100.00%) |
| nodes per skill | median 21 · p95 70 · max 899 |
| segments lifted out of prose | **19,534** |
| tier clamps | 5,473 |
| size | 64.9 MB source → 83.9 MB container (+29.3%) |

### What the numbers say

**19,534 segments.** Only 184 of 8,776 skills ship a `scripts/` directory, but the corpus holds
nineteen thousand executable fragments — bash, python, node — sitting in fenced code blocks with
no identity, no declared capabilities, no signature, and no way to run them except a model
retyping them into a shell. The format does not invent executable content; it gives content that
already exists an ABI, a BLAKE3 root and a capability set. Every one of those 19,534 compiles to
a segment authorized for **nothing**, which is the correct default for code lifted from prose.

**5,473 tier clamps.** A section whose heading says it belongs in the routing plane but which sits
under a body section gets promoted to its parent's tier, because tier must be monotone. That is
0.6 clamps per skill: authors nest "when to use" under "overview" routinely. The clamp is the
compiler resolving a real ambiguity, not an error.

**+29.3% size, uncompressed.** This is the honest number and it is not a win on its own. The
container carries fixed-width tables and per-node BLAKE3 commitments that Markdown does not, and
neither zstd nor the shared corpus dictionary described in `SPEC.md` §6 is implemented yet. The
size argument is unproven until they are.

**99.85% byte-exact round-trip.** The 13 files that are not byte-exact differ only in trailing
whitespace. Getting here found four parser defects, each caught by the corpus and by nothing
else: unterminated fences at EOF, indented fences (which let `#` comments inside Python parse as
headings), closing-fence indentation that differs from the opening line's, and heading text with
multiple spaces after the hashes.

### Known issue: the routing plane is not as hot as claimed

`SPEC.md` §4.1 hoists `name` and `description` to fixed manifest offsets so routing "reads a
fixed offset and never walks the graph". Measured, the header-plus-manifest span averages
**2,725 bytes per skill** — because those two fields are *indices* into a sorted string heap, and
sorting scatters them anywhere in the manifest. Reading them touches more pages than it should.

The fix is a contiguous routing block placed immediately after the signature block, with `name`
and `description` removed from the heap entirely so the bytes still have exactly one encoding.
Not yet implemented.

## Fuzzing

`crates/skill-format/src/bin/fuzz.rs`. Bounded by construction, because the box this runs on is
usually busy:

- **single-threaded**; no pool, no `-jobs` equivalent, nothing that scales with core count
- stops at whichever of `--iters` or `--seconds` arrives first
- `--max-bytes` (default 64 KiB, hard ceiling 1 MiB) caps every input. This is the bound that
  matters: the validator allocates proportionally to lengths the *file* declares, so capping the
  file caps the allocation
- deterministic from `--seed`; a failure prints the seed and the offending bytes
- `scripts/fuzz.sh` additionally runs it under a systemd scope with `CPUQuota=100%` and
  `MemoryHigh=2G`, so the ceiling belongs to the kernel rather than to the fuzzer's good intentions

Three 20-second campaigns, seeds 1 / 99999 / 4242424:

| | |
|---|---|
| inputs | 46,062,144 |
| accepted by the validator | 4,996,948 (10.8%) |
| panics | 0 |
| peak RSS | 31 MB |

The 10.8% accept rate is the number that makes the campaign meaningful: mutations are reaching
graph validation and lazy hash verification, not dying at the magic check. The gate runs a
5-second version of the same campaign on every invocation.


## Model checking

`specs/check.sh` — 13 TLC configurations, 88 s wall, capped at 4 workers and a 4 GB heap, niced,
per-run timeout, hermetic metadir. **Six are canaries that must fail**; the script exits non-zero
if one passes, because a suite whose canaries also pass checked nothing.

| Configuration | Expected | What it establishes |
|---|---|---|
| `SkillTree.cfg` | holds | the one forward pass is **equivalent** to the semantic tree, pre-order and tier invariants, over all 1,889,568 five-node graphs |
| `TreeRedundancy.cfg` | holds | the pre-order scan implies `parent < self` |
| `TreeCanaryParentBefore.cfg` | violated | `parent < self` alone permits a split subtree |
| `TreeCanaryPreOrderScan.cfg` | violated | the scan is load-bearing |
| `TreeCanaryTier.cfg` | violated | tier monotonicity is load-bearing |
| `TreeCanaryRootTier.cfg` | violated | the root-is-tier-0 rule is load-bearing |
| `SkillSeq.cfg` | holds | `dst > src` implies `SEQ` acyclicity |
| `SeqCanaryConverse.cfg` | violated | the converse fails: `seq = {⟨2,1⟩}` is the cost |
| `SkillEdges.cfg` | violated | **without a union check, `ALT` alone cycles** |
| `EdgesCanaryPiecewise.cfg` | violated | per-relation acyclicity ≠ union acyclicity |
| `EdgesFixed.cfg` | holds | with the union check, activation terminates |
| `SkillGraph.cfg` | holds | tree and obligations composed, 15,136,875 states |
| `GraphWithDescent.cfg` | violated | open question: an obligation into an ancestor (`SPEC.md` §10.7) |

### What it found that nothing else did

Two defects, both written up in `SPEC.md` §10.6.

**`ALT` was never checked for cycles.** `alt = {⟨1,2⟩, ⟨2,1⟩}` — two nodes falling back to each
other forever — was a valid file. **And checking each relation separately is strictly weaker than
checking the union**: `guards = {⟨1,2⟩}` with `alt = {⟨2,1⟩}` is two acyclic relations and one
infinite loop.

Neither is a byte mutation. Both are graphs the *writer emits happily*, which is exactly why
46 million fuzz inputs and 24 hostile-corpus cases missed them — a fuzzer mutates files, and
these are defects in what counts as a valid graph. The division of labour is real: the fuzzer
found nothing here, and the model checker found nothing the fuzzer was looking for.

It also corrected the spec. `SPEC.md` §4.4 claimed the tree invariant was proven by one
comparison per node. It is not; `parent = ⟨0,1,1,1,2⟩` has every parent preceding its child and a
subtree split in half. The ancestor walk is doing the work.
