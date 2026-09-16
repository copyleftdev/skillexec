# Measured results

Numbers here are reproducible from this tree. Nothing is estimated.

```sh
find <your-skill-dirs> -name SKILL.md -not -path '*/node_modules/*' | sort -u > corpus.txt
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

**Size.** Superseded — see *Compression* below. The uncompressed container is 23% larger than
its source; the compressed one is 54% smaller; and it is still larger than compressed Markdown.
All three of those are true at once and the section below gives each its due.

**99.85% byte-exact round-trip.** The 13 files that are not byte-exact differ only in trailing
whitespace. Getting here found four parser defects, each caught by the corpus and by nothing
else: unterminated fences at EOF, indented fences (which let `#` comments inside Python parse as
headings), closing-fence indentation that differs from the opening line's, and heading text with
multiple spaces after the hashes.

### Fixed: the routing plane is now actually hot

An earlier draft hoisted `name` and `description` into the manifest header and claimed routing
therefore "reads a fixed offset and never walks the graph". The fields were there; the *bytes*
were not. Both were `u32` indices into a string heap that is sorted for canonicality, and sorting
puts them wherever they fall — so reading them meant reading the manifest, **1,524 bytes per
skill** to retrieve about 200.

`SPEC.md` §3.1 replaces that with a contiguous routing block at an offset derived from the header
alone, with the two strings removed from the heap entirely so there is still exactly one encoding
of each. Routing now touches **331 bytes per skill**. The numbers below are measured after the
fix; `SPEC.md` §10.8 records what it cost to get wrong.

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


## The sandbox

`crates/skill-run` executes a `Portable` segment under exactly the capabilities its record
declares. Ten tests, each asserting one property of the gate:

| Property | Test |
|---|---|
| an undeclared import is refused **before instantiation** | a module importing `fd_write` with no `stdio` capability |
| the same module runs once the capability is declared | output captured, `granted == ["stdio"]` |
| an unrecognised import is refused, not ignored | `import "evil" "backdoor"` → `needs: "unknown"` |
| a segment with no imports needs no capabilities | pure compute runs with an empty grant set |
| `cpu_ms` is enforced | an infinite loop returns `OutOfFuel`, not a hang |
| `mem_kib` is enforced | `memory.grow` fails inside a 64 KiB budget and succeeds inside 8 MiB |
| a segment may not exceed the host ceiling | refused with `LimitExceedsPolicy`, never clamped |
| a `HostTrusted` segment is refused by default | `Abi::Sh` → `NotPortable` |
| tampered bytes are refused before compilation | `IntegrityFailed`, via the §5 step-7 hash |
| every granted capability was declared in the file | grant set equals the file's capability table |

The design point is that enforcement happens at **linking**. A wasm module can only reach the
host through its imports, so declining to satisfy an undeclared import makes the capability
unreachable rather than merely guarded — there is no call site left to check, and nothing to
forget to check.

### What it is not

Not a WASI host. Granted capabilities other than `stdio` link and then report unsupported: the
segment is authorized and there is nothing behind the door yet. `wasm32-wasip2` components are
not executed either — ABI `6` (`wasm32-core`) is what runs today. The gate is real; the world
behind it is a stub, and pretending otherwise would be the kind of claim this project exists to
avoid making.


## Compression

Same 8,776 files. Every row is measured, not projected.

| Representation | Total | vs source | Read one description without decoding the body? |
|---|---|---|---|
| Markdown source | 64.9 MB | — | yes |
| Container, `None` | 80.3 MB | +23.8% | yes |
| Container, `Mapped` | 44.9 MB | −30.8% | yes |
| Container, `Compact` | 37.6 MB | −42.0% | yes |
| Container, `Mapped` + dict | 39.5 MB | −39.0% | yes |
| **Container, `Compact` + dict** | **31.7 MB** | **−51.2%** | yes |
| Markdown, per-file `zstd -19` | 26.5 MB | −59.2% | **no** |
| Markdown, per-file `zstd -19` + dict | 20.3 MB | −68.7% | **no** |
| Markdown, whole corpus as one stream | 13.4 MB | −79.4% | **no** |

The dictionary is 110 KB, trained once and shared; it is referenced by BLAKE3, not embedded,
because 110 KB inside each of 8,776 files would cost 940 MB to save 6 MB.

One caveat that cuts against the container, stated because it would be easy to leave out: the
three Markdown rows were measured with the `zstd` CLI over **68.0 MB** — every file `cat` could
read — while `skillc` reports **64.9 MB**, the subset that parsed as UTF-8. The baselines
therefore compress about 5% more input than the container rows do, which flatters the container.
`skillc route` re-measures the dictionary baseline over exactly the same file set and gets
**19.35 MB**, not 20.3 MB. That is the number the comparison below uses.

### Reading the table honestly

**The gap that was open is closed.** The container used to be *larger than its own source*
(+29.3%, before commitments were pruned to boundaries). It is now 51% smaller. That was the
claim `SPEC.md` §6 had no evidence for, and now it does.

**Against compressed Markdown, the container still loses**, 31.7 MB against 19.35 MB — about
1.64× the bytes. Anyone whose only goal is bytes on disk should gzip the Markdown and stop
reading here. That is worth saying plainly rather than burying under a favourable subset of the
table.

**The last two rows are not the same product.** A zstd'd `SKILL.md` has to be decompressed in
full before you can read its `description`, because frontmatter sits at the front of a stream
that decodes from the start. The whole-corpus row is worse still: 13.4 MB only exists if you
treat 8,776 skills as one stream, and no individual skill can be read out of it at all. The
container answers "is this skill relevant" from a fixed offset with no decompression in
`Mapped`, and by decoding one small string section in `Compact`.

Where the difference goes, for `Compact` + dict: about 19 MB of compressed payload — that part
*is* the Markdown — plus roughly 10 MB of manifest tables and 2 MB of routing blocks. The 12 MB
on top is everything Markdown does not have: a typed graph, per-boundary BLAKE3 commitments,
capability records, and a routing plane that can be read without decoding any of it.

### What the routing block cost

`SPEC.md` §3.1 is not free. `name` and `description` left the compressed string heap for an
uncompressed block, which across the corpus is about **250 bytes per skill that used to
compress**: `Compact` + dict went from 29.6 MB to 31.7 MB, up 7.1%.

It bought routing going from 4,330 ns to 182 ns in that profile, and from 1,256 bytes touched to
364. If nothing ever routes over the corpus, that is 2.1 MB spent on nothing; the block is a bet
that something does, which is the same bet the whole format makes.

### What made the difference

Three changes, in order of how much they moved:

1. **Compress regions, not nodes.** A median skill is 5.5 KB across 21 nodes — 260 bytes each —
   and zstd on 260 bytes usually *grows* it. Compressing HOT and COLD once each took the payload
   from 61.1 MB to 24.9 MB, and to 19.0 MB with the dictionary. The earlier design in `SPEC.md` §6 said "per node/segment" and would
   have made the file bigger.
2. **Store commitments only at boundaries.** A hash per node cost 6.5 MB and bought nothing a
   Merkle tree does not already imply. Keeping them for the root, segments and tier step-ups cut
   the manifest from 23.0 MB to 18.2 MB before compression (`SPEC.md` §11.2).
3. **A shared dictionary.** 37.6 → 31.7 MB in `Compact`. Small files are where a dictionary
   earns its keep, and at a 5.5 KB median every file here is a small file.

### A hypothesis that did not survive being measured

`skillc dict` trains on the COLD regions of compiled containers rather than on the Markdown,
on the reasoning that a dictionary should learn the bytes it will actually be asked to help
with. Measured, that dictionary produced **29.9 MB** against the Markdown-trained one's
**29.6 MB** (both before the routing block) — very slightly worse, and inside the noise of dictionary training.

The reasoning was wrong about this corpus for a mundane reason: a skill's COLD region *is*
mostly its Markdown prose, and frontmatter — which sounded like Markdown framing the container
had stripped away — is itself a payload node. There was never much for the two dictionaries to
disagree about. The training path is kept because it is the right default for a corpus whose
payloads are not prose; it is not kept because it helped here.

### One performance defect worth recording

The first dictionary implementation called `Compressor::with_dictionary` per region per file,
which re-digests the dictionary every time — work proportional to the *dictionary*, paid
per *region*. Compiling the corpus went from ~1 s to minutes, and the cause was invisible in a
unit test because a single-file test digests it once either way. `codec::Dictionary` now
prepares the encoder and decoder forms once.


## Routing cost

Size is only half the comparison, and it is the half that favours compressed Markdown. The other
half is what it costs to answer the question a router actually asks 8,776 times: *is this skill
relevant?*

`skillc route` builds every skill both ways in memory and times reading `name` and `description`
from each. Same corpus, same dictionary, same process.

| | Bytes | vs baseline | Routing | vs baseline |
|---|---|---|---|---|
| Container, `Mapped` + dict | 39.5 MB | 2.04× | **176 ns/skill** | **165.7× faster** |
| Container, `Compact` + dict | 31.7 MB | 1.64× | **182 ns/skill** | **159.5× faster** |
| Markdown, per-file zstd + same dict | 19.35 MB | 1.00× | 29,033 ns/skill | 1.00× |

Routing costs the same in both profiles now, because it reads the routing block (§3.1) and
nothing else — not the string heap, compressed or otherwise. Before that block existed, `Compact`
paid 4,330 ns against `Mapped`'s 453: it had to inflate a section to read a name, a 9.5× penalty
on the profile whose only job was to be smaller. `Compact` is now simply the better default.

The asymmetry against Markdown is structural, not an optimisation. Frontmatter sits at the front
of a stream that decodes from the start, so reading one description out of a zstd'd `SKILL.md`
means decompressing the whole file — 5.5 KB of median body, to read maybe 200 bytes of it. The
container never touches a payload region to answer the question, which is what the tier split in
`GRAPH.md` §4 was for: not a convention a loader politely follows, but a layout in which the body
is somewhere the router does not go.

So the summary is a trade, not a win: **1.5× to 2× the bytes on disk, 7× to 65× less work per
routing decision**, plus a signed graph, per-boundary commitments and capability records the
Markdown does not carry at all. Whether that is worth it depends entirely on whether anything
ever routes over the corpus. For an archive nobody queries, gzip the Markdown.
