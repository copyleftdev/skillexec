# `.skill` — Binary Skill Container, Format Version 1.0

Status: draft 0.1. Realizes the data model in `GRAPH.md`.
All integers **little-endian**, unsigned, naturally aligned. Endianness is never negotiated.
All offsets are **absolute from start of file** unless a table says otherwise.
Reserved fields **MUST** be written zero and readers **MUST** reject non-zero.

## 0. Design commitments

| Commitment | Consequence |
|---|---|
| Verify before parse | signatures cover a fixed 64-byte range at offset 0; no variable-length parsing precedes verification |
| Map, don't parse | fixed-width records, natural alignment, offsets not pointers |
| One encoding per graph | canonical ordering (`GRAPH.md` §7); `serialize(parse(b)) ≡ b` |
| Unknown kind ⇒ reject | must-understand bits; skipping is a signature-stripping vector |
| Lazy verification | BLAKE3 is a Merkle tree — verify a 1 KiB slice without hashing the file |
| 4 GiB file cap | deliberate: `u32` offsets. Median skill is 5.5 KB; p95 is 18.8 KB |

## 1. File structure

```
0x0000  Header             64 bytes, fixed, frozen across all v1.x
0x0040  Signature block    sig_count × 112 bytes   (may be 0)
        Manifest           tables; digest committed in the header
        Payload HOT        tier-0 payloads
        Payload COLD       tier-1/2 payloads, per-segment compressed
```

## 2. Header (64 bytes, offset 0)

| Off | Size | Field | Notes |
|---|---|---|---|
| 0x00 | 8 | `magic` | `8F 53 4B 4C 0D 0A 1A 0A` — high bit (8-bit-clean test), `SKL`, CRLF pair (text-mode mangling trap), `1A` (DOS EOF), `0A` (LF-stripping trap). PNG's trick, and it still works. |
| 0x08 | 2 | `ver_major` | `1`. Reader rejects unknown major. |
| 0x0A | 2 | `ver_minor` | Reader ignores unknown minor. |
| 0x0C | 4 | `feature_flags` | **Must-understand bitmask.** Any set bit the reader does not implement ⇒ reject. |
| 0x10 | 4 | `file_len` | MUST equal actual length. Truncation detector. |
| 0x14 | 4 | `manifest_off` | |
| 0x18 | 4 | `manifest_len` | |
| 0x1C | 2 | `sig_count` | |
| 0x1E | 2 | `reserved` | zero |
| 0x20 | 32 | `manifest_root` | BLAKE3 root of manifest bytes |

The header is the **entire commitment**: it names the manifest root, the manifest names every
section, every node names its subtree hash. Signing 64 bytes signs the file.

## 3. Signature block (offset 0x40, `sig_count` × 112 bytes)

| Off | Size | Field |
|---|---|---|
| 0x00 | 2 | `alg` — `1` = Ed25519 |
| 0x02 | 2 | `flags` — bit 0: publisher, bit 1: endorsement, bit 2: transparency-log inclusion |
| 0x04 | 4 | `reserved` (zero) |
| 0x08 | 32 | `keyid` — BLAKE3 of the public key |
| 0x28 | 64 | `sig` — over `BLAKE3(header[0x00..0x40])` |
| 0x68 | 8 | `reserved` (zero) |

`sig_count = 0` is well-formed (local/dev files). Whether to *accept* an unsigned file is policy,
not format. Multiple signatures are independent assertions over the same bytes — a publisher and
a reviewer sign the same digest, no envelope nesting, no re-serialization.

## 4. Manifest

### 4.1 Manifest header (48 bytes, at `manifest_off`)

| Off | Size | Field |
|---|---|---|
| 0x00 | 2 | `sect_count` |
| 0x02 | 2 | `flags` |
| 0x04 | 4 | `name_idx` → string heap |
| 0x08 | 4 | `desc_idx` → string heap |
| 0x0C | 4 | `version_idx` (`0xFFFFFFFF` = absent) |
| 0x10 | 4 | `license_idx` (`0xFFFFFFFF` = absent) |
| 0x14 | 4 | `node_count` |
| 0x18 | 4 | `hot_off` |
| 0x1C | 4 | `hot_len` — **stored** length |
| 0x20 | 4 | `cold_off` |
| 0x24 | 4 | `cold_len` — **stored** length |
| 0x28 | 4 | `hot_orig_len` |
| 0x2C | 4 | `cold_orig_len` |

`flags` at 0x02: bit 0 `HOT_ZSTD`, bit 1 `COLD_ZSTD`, bit 2 `USES_DICT`.

`name` and `desc` are manifest fields, not nodes, because the corpus says they are the only
universal elements (9,008 / 9,035 of 9,212 files). Hoisting them means **routing reads a fixed
offset and never walks the graph.**

### 4.2 Section directory (`sect_count` × 24 bytes)

| Off | Size | Field |
|---|---|---|
| 0x00 | 2 | `id` |
| 0x02 | 2 | `flags` — bit 0 must-understand, bit 1 zstd |
| 0x04 | 4 | `off` — **relative to manifest start** |
| 0x08 | 4 | `len` — stored length |
| 0x0C | 4 | `count` — records, for fixed-width sections |
| 0x10 | 4 | `orig_len` — uncompressed length; **must equal `len`** when bit 1 is clear |
| 0x14 | 4 | `reserved` (zero) |

| id | Section | Record |
|---|---|---|
| 1 | STRINGS | heap, §4.3 |
| 2 | NODES | 32 B, §4.4 |
| 3 | EDGES | 8 B, §4.5 |
| 4 | HASHES | 32 B |
| 5 | SEGMENTS | 96 B, §4.6 |
| 6 | CAPS | 8 B |
| 7 | EXTREFS | external `CITES` targets |
| 8 | SRCSPANS | 8 B, optional, round-trip only |
| 9 | DICTREF | 32 B — BLAKE3 of the shared zstd dictionary this file requires |
| 10 | REGIONS | 64 B — BLAKE3 of the **stored** HOT and COLD bytes |

Sections are 8-byte aligned. Absent optional sections are omitted, not zero-length.

### 4.3 String heap

`count u32`, then `offsets[count+1] u32` (last is an end sentinel), then the bytes.
Sorted lexicographically by byte value, deduplicated, NFC-normalized, not NUL-terminated.
Sorted ⇒ lookup is a binary search; deduped ⇒ equality is an integer comparison.

### 4.4 Node record (32 bytes — two per cache line)

| Off | Size | Field |
|---|---|---|
| 0x00 | 1 | `kind` — 1 `Applicability`, 2 `Prose`, 3 `Contract`, 4 `Segment`, 5 `Resource`, 6 `Binding` |
| 0x01 | 1 | `tier` — 0/1/2 |
| 0x02 | 1 | `flags` — bit 0 must-understand, bit 1 payload is in COLD, bit 2 payload compressed |
| 0x03 | 1 | `depth` — original heading depth, 0 = none (round-trip) |
| 0x04 | 2 | `role` — descriptive; unknown roles render as plain `Prose` |
| 0x06 | 2 | `edge_cnt` |
| 0x08 | 4 | `name_idx` |
| 0x0C | 4 | `edge_off` — index into EDGES |
| 0x10 | 4 | `payload_off` — into HOT or COLD per `flags` bit 1 |
| 0x14 | 4 | `payload_len` |
| 0x18 | 4 | `hash_idx` — into HASHES, or `0xFFFFFFFF` for a node carrying no stored commitment (§11.2) |
| 0x1C | 4 | `parent` — node index, `0xFFFFFFFF` for root |

Nodes are stored in `CONTAINS` pre-order. A reader therefore checks, for each node `n > 0`:

```
parent[n] ∈ {n-1} ∪ ancestors(n-1)
```

one ancestor walk per node, no auxiliary structure, no cycle detection, no allocation. TLC
confirms this is **exactly equivalent** to the semantic tree, pre-order and tier invariants over
every five-node graph (`specs/SkillTree.tla`).

`parent < self` follows from that scan rather than standing beside it, so the `ParentNotBefore`
check is redundant and kept only for a precise error. An earlier draft of this section claimed
the tree invariant was proven by *one comparison* per node; that is false, and the counterexample
is `parent = ⟨0,1,1,1,2⟩` — every parent precedes its child, and node 2's subtree is still split
in half.

### 4.5 Edge record (8 bytes)

Edges are grouped by source, so the source is implicit in `edge_off`/`edge_cnt`.

| Off | Size | Field |
|---|---|---|
| 0x00 | 4 | `dst` — node index, or EXTREFS index if `kind` bit 7 set |
| 0x04 | 1 | `kind` — 2 `SEQ`, 3 `GUARDS`, 4 `NEEDS`, 5 `ALT`, 6 `CITES`; bit 7 = external. Tag `1` (`CONTAINS`) is **reserved and rejected** — see §10.1 |
| 0x05 | 1 | `ordinal` |
| 0x06 | 2 | `label` |

Only `CITES` may set bit 7. Enforced: everything the loader is obliged to follow stays inside
the file and stays acyclic, so activation terminates.

`SEQ` additionally requires `dst > src`. Because nodes are stored in pre-order, that makes the
canonical node order a topological order of `SEQ`, so acyclicity is an `O(E)` comparison instead
of a traversal.

This is a **restriction, not a free win**, and the cost is stated rather than hidden: a `SEQ`
order that disagrees with document order is acyclic but unrepresentable. The minimal case TLC
produces is `seq = {⟨2,1⟩}` — "the step written second runs first". Authors who need that reorder
the document.

**The union of obligation edges must be acyclic** — `SEQ ∪ GUARDS ∪ NEEDS ∪ ALT`, checked as one
graph, not four (§10.6). `CITES` is excluded and may cycle freely.

### 4.6 Segment record (96 bytes)

| Off | Size | Field |
|---|---|---|
| 0x00 | 32 | `root` — BLAKE3 root of the decompressed segment |
| 0x20 | 4 | `orig_len` — uncompressed length |
| 0x24 | 2 | `abi` — 1 `wasm32-wasip2`, 2 `sh`, 3 `python3`, 4 `node`, 5 `native`, 6 `wasm32-core` |
| 0x26 | 1 | `codec` — 0 raw, 1 zstd, 2 zstd+dict |
| 0x27 | 1 | `trust_class` — 0 `Portable`, 1 `HostTrusted` |
| 0x28 | 4 | `cap_off` · 0x2C 2 `cap_cnt` · 0x2E 2 `flags` |
| 0x30 | 4 | `mem_kib` · 0x34 4 `cpu_ms` · 0x38 4 `wall_ms` |
| 0x3C | 4 | `dict_idx` — shared zstd dictionary, `0xFFFFFFFF` = none |
| 0x40 | 4 | `signer_idx` — into the signature block; `0xFFFFFFFF` = file signature only |
| 0x44 | 28 | `reserved` (zero) |

The record carries **no offset or length**: a segment's bytes are the payload of the
`Segment`-kind node it is parallel-indexed to (§10.4).

Capability record (8 B): `kind u16`, `flags u16`, `arg_idx u32` → string heap.
Kinds: `1 net:host`, `2 fs:read`, `3 fs:write`, `4 env`, `5 exec`, `6 clock`, `7 rand`, `8 stdio`.

ABIs `1` and `6` are the `Portable` values. A `HostTrusted` segment is *labelled*, never
contained — the format cannot sandbox a shell script, and says so rather than implying
otherwise. Policy of the form *"unknown publisher ⇒ Portable only"* is expressible because the
distinction is a signed field, not a convention.

**Capabilities are enforced at link time, not at call time.** A wasm module reaches the host only
through its imports, so `crates/skill-run` refuses to *satisfy* an import the segment did not
declare, and refuses to instantiate at all if one is missing. There is no call site left to
guard, and an import this host does not recognise is a refusal rather than a shrug — a host that
ignores imports it does not understand has no capability model.

Limits are enforced by the runtime: `cpu_ms` becomes wasmtime fuel, `mem_kib` a store limit,
`wall_ms` an epoch deadline. A segment asking for more than the host's ceiling is **refused, not
clamped**: a skill written for 4 GB should fail loudly on a host that will not give it, rather
than run in a shape its author never tested.

## 5. Verification order (normative)

A conforming reader performs these in order and aborts at the first failure:

1. Read exactly 64 bytes. Check `magic`, `ver_major`, and that `feature_flags` has no unknown bits.
2. Check `file_len` equals the real file length.
3. Verify every signature over `BLAKE3(header[0x00..0x40])` against the trust policy.
4. Bounds-check `manifest_off + manifest_len ≤ file_len` (no overflow).
5. `BLAKE3(manifest bytes) == manifest_root`, else abort.
6. **Only now** parse the manifest. Validate `GRAPH.md` §9 invariants 1–10 in one pass.
7. Payloads and segments are verified **lazily** against their subtree hash at first touch.

Step 6 additionally requires that every byte of the HOT and COLD regions **not claimed by some
node's payload range is zero** (§10.3).

Steps 1–5 touch no variable-length data. Step 6 operates on bytes already proven to be the
publisher's. That ordering is the whole security argument, and it is what JAR/APK-v1, PDF
incremental update, and XML-DSig each got wrong in their own way.

## 6. Size strategy

Compression is **per region and per section**, never per node. A median skill is 5.5 KB across
21 nodes — about 260 bytes each — and zstd on 260 bytes usually *grows* it. Compressing the
region once is what makes the ratio real, and it costs nothing that matters: a region is
decompressed on first touch, and routing never touches one.

- **HOT and COLD are each one zstd frame.** Node offsets index the *decompressed* region, so
  the tier split still decides what gets decompressed at all.
- **Tables are compressed only in the `Compact` profile** (§11.1). In `Mapped` they stay
  readable in place, which is what "map, don't parse" was for.
- **A shared dictionary** (§11.3) is referenced by BLAKE3, never embedded: a 110 KB dictionary
  inside each of 8,776 files would cost more than it saves.
- Payloads are content-addressed, so identical sections across skills store once.

Measured numbers, and how they compare to simply compressing the Markdown, are in
`docs/RESULTS.md`. The short version is that the container is **not smaller than dictionary-
compressed Markdown**, and the comparison is not really like-for-like in either direction —
which is stated there rather than argued away here.

## 7. Worked example — minimal valid file

Emitted by the reference writer (`cargo run --example dump_minimal`, refreshed by
`scripts/refresh-spec-dump.py`), so the spec's example and the implementation cannot drift.
Built with `Profile::None`: a hexdump of a zstd frame teaches nothing about this format.
390 bytes; `name="demo"`,
`description="Use when demonstrating the skill format."`; a tier-0 `Prose` root with an empty
payload and one tier-1 child carrying `"Do the thing.\n"`. `sig_count = 0`.

```
00000000  8f 53 4b 4c 0d 0a 1a 0a  01 00 00 00 00 00 00 00   magic, v1.0, feature_flags=0
00000010  86 01 00 00 40 00 00 00  38 01 00 00 00 00 00 00
00000020  d7 88 62 59 b3 e7 18 ed  2d b1 0e 79 1b 97 12 30   manifest_root (BLAKE3 of the manifest bytes)
00000030  f4 0b d7 3c d4 e2 fb 88  c1 92 ed 45 88 8e b1 3a
00000040  03 00 00 00 01 00 00 00  00 00 00 00 ff ff ff ff   sect_count  flags=0 (uncompressed)  name_idx  desc_idx
00000050  ff ff ff ff 02 00 00 00  78 01 00 00 00 00 00 00
00000060  78 01 00 00 0e 00 00 00  00 00 00 00 0e 00 00 00
00000070  01 00 00 00 78 00 00 00  3c 00 00 00 02 00 00 00
00000080  3c 00 00 00 00 00 00 00  02 00 00 00 b8 00 00 00
00000090  40 00 00 00 02 00 00 00  40 00 00 00 00 00 00 00
000000a0  04 00 00 00 f8 00 00 00  40 00 00 00 02 00 00 00
000000b0  40 00 00 00 00 00 00 00  02 00 00 00 00 00 00 00
000000c0  28 00 00 00 2c 00 00 00  55 73 65 20 77 68 65 6e
000000d0  20 64 65 6d 6f 6e 73 74  72 61 74 69 6e 67 20 74
000000e0  68 65 20 73 6b 69 6c 6c  20 66 6f 72 6d 61 74 2e
000000f0  64 65 6d 6f 00 00 00 00  02 00 00 00 01 00 00 00
00000100  ff ff ff ff 00 00 00 00  00 00 00 00 00 00 00 00
00000110  00 00 00 00 ff ff ff ff  02 01 02 01 01 00 00 00
00000120  ff ff ff ff 00 00 00 00  00 00 00 00 0e 00 00 00
00000130  01 00 00 00 00 00 00 00  0b 6a a1 83 70 03 20 af
00000140  4e ee d9 5f de 5d b7 0e  53 cb 5a 31 51 7c fb 9d
00000150  8c 49 18 00 35 31 5e 13  1f 5d d5 ad 20 f5 3a 75
00000160  65 56 8d 05 46 b5 73 f9  59 86 90 26 94 a3 4a b4
00000170  02 f1 fc 1a 6b b3 10 a4  44 6f 20 74 68 65 20 74
00000180  68 69 6e 67 2e 0a
```

Note the string heap ordering: `"Use…"` (0x55) sorts before `"demo"` (0x64), so `name_idx = 1`
and `desc_idx = 0`. Canonical ordering is observable in the bytes — which is the point.

There is no EDGES section: this graph has none, and an empty section is omitted rather than
written zero-length. Absence and emptiness are the same fact, so the format spells it one way.

## 8. Conformance corpus (to be built alongside the reader)

Valid: minimal (§7) · every node kind · every edge kind · all three tiers · multi-signature ·
compressed and dictionary-compressed segments · a full round-trip of a real corpus skill.

Hostile — each MUST be rejected, and the corpus names which step of §5 rejects it:

| Class | Cases |
|---|---|
| Truncation | cut at every section boundary; `file_len` ≠ actual |
| Ranges | overlapping payloads; offset past EOF; `off + len` overflow; offset into the header |
| Signature | valid sig over a mutated manifest; sig over a *different* range; duplicate `keyid`; `sig_count` exceeding the file |
| Must-understand | unknown `feature_flags` bit; unknown `kind`; unknown section with bit 0 set |
| Graph | `CONTAINS` cycle; `parent ≥ self`; forest instead of tree; `SEQ` cycle; `NEEDS` cycle; tier decreasing along `CONTAINS`; `GUARDS` from a non-`Contract` |
| Canonicality | valid graph in non-canonical node order; unsorted or duplicated string heap; non-zero reserved; non-zero padding |
| Segment | declared caps narrower than actual imports; `Portable` claim on a non-wasm ABI; wrong `orig_len`; zip-bomb ratio |

Invariants 1–6 of `GRAPH.md` §9 go to TLC; the parser state machine goes to the fuzzer; both
round-trip properties (§`GRAPH.md` 8) go to property-based tests.

## 9. Open decisions

1. Bundle vs. store — single self-contained file (assumed here) with content-addressed segments
   so a CDN can dedupe, vs. a thin file referencing an external CAS.
2. Signing — raw Ed25519 + transparency log (assumed; matches AION) vs. X.509/sigstore.
3. Whether tier-0 across a corpus gets a *separate* index file, or is scavenged from N files by
   mmap-ing 64 bytes of each. The corpus is 8,776 files; both are viable and the measurement is
   cheap.


## 10. Defects found by the reference implementation

Three changes to §§1–9 were forced by writing the reader and its corpus. They are recorded here
rather than silently folded in, because each is an instance of a failure mode the methodology
predicts.

### 10.1 `CONTAINS` was two encodings of one fact

The draft carried the tree *both* as `Node::parent` and as `CONTAINS` edge records. Two
encodings of one fact is precisely the canonicality hazard that makes a signature meaningless —
a file could assert a parent pointer and an edge that disagree, and two readers could resolve it
differently.

`CONTAINS` is now carried **solely** by `parent` plus pre-order position. Edge tag `1` is
reserved and any edge bearing it is rejected (`Error::ContainsInEdgeTable`).

### 10.2 The root must be tier 0

Tier monotonicity (`tier(child) ≥ tier(parent)`) and "the root holds the body prose" are
inconsistent: it makes an `Applicability` child — the routing predicate, necessarily tier 0 —
unrepresentable under a tier-1 root. That is the shape *every* real skill has, so the fixture
that exposed it was not a contrived case.

Node 0 MUST be `Tier::Routing`. Body prose is a child. `Error::RootNotRouting`.

### 10.3 Padding was committed by nothing

A test flipped the last byte of a padded file and it still verified. Payload regions are padded
to 8 bytes; those pad bytes lie outside every subtree hash and outside the manifest digest, so
they were a malleability channel — mutate them freely and every signature still checks out.

Every byte of a payload region not claimed by a node's payload range MUST be zero, and the
validator enforces it (`Error::UncommittedNonZero`). The general rule, stated in §0 but not
originally carried through: *if a byte is in the file and no commitment covers it, the format is
malleable there.*


### 10.4 A SEGMENTS section with no way to reach it

§4.6 defined a segment record and §4.4 defined a node record, and nothing connected them. The
node has no spare field — all 32 bytes are spoken for — so the obvious fixes were to overload an
existing field or to grow the record.

Neither was necessary. **Segment records are parallel-indexed to `Segment`-kind nodes in
pre-order**: the k-th `Segment` node uses the k-th record. Costs nothing, is canonical because
both orders are already canonical, and is validated by requiring the counts to agree
(`Error::SegmentCountMismatch`).

The same reasoning removed `off` and `len` from the record. A segment's bytes are its node's
payload; storing that location twice would have reintroduced §10.1 verbatim.

### 10.5 Dedup by substring manufactures overlapping ranges

The first writer deduplicated payloads with a substring search, which looks like a strictly
better exact-match dedup. It is not. Short payloads occur inside longer ones constantly — a lone
`"\n"`, a bare fence marker — so 30% of a 300-file corpus slice produced *partially* overlapping
payload ranges and were rejected by the validator's own overlap rule.

Dedup is now exact-blob only, keyed by `(region, bytes)`. Identical ranges stay legal, which is
what content addressing needs; partial overlap remains a rejection. The substring version was
also `O(region × payload)` per node, which the corpus run made visible.


### 10.6 Activation did not terminate

`GRAPH.md` §3 claimed that everything the loader is obliged to follow stays acyclic, so
activation terminates. TLC was asked to confirm it and refused, twice
(`specs/SkillEdges.tla`).

**ALT was not checked for cycles at all.** The validator enforced ordinal uniqueness on `ALT`
and nothing else, so `alt = {⟨1,2⟩, ⟨2,1⟩}` was accepted: node 1 falls back to node 2, node 2
falls back to node 1, forever. Two nodes, one infinite loop, and every unit test passed.

**Checking each relation separately is strictly weaker than checking their union.** Even with
every relation individually acyclic, `guards = {⟨1,2⟩}` with `alt = {⟨2,1⟩}` is two acyclic
relations whose union is a cycle. Nothing looked at the union.

The validator now builds one obligation graph from `SEQ ∪ GUARDS ∪ NEEDS ∪ ALT` and rejects a
cycle in it (`Error::ObligationCycle`). `NEEDS` keeps its own separate pass so that the common
case still reports `Error::NeedsCycle`. With the union check in place, TLC confirms the claim.

Neither cycle is a byte mutation. Both are graphs the writer emits happily, which is why 24
hostile-corpus cases and 46M fuzz inputs missed them: the fuzzer mutates *files*, and these are
defects in what counts as a valid *graph*.

### 10.7 Open: obligations that point at an ancestor

A loader descends the `CONTAINS` tree from parent to child. If descent counts as an edge, then
`oblig = {⟨2,1⟩}` with node 2 a child of node 1 is a cycle: enter 1, descend to 2, and 2 requires
1. TLC finds it immediately (`specs/SkillGraph.tla`, `INCLUDE_DESCENT = TRUE`).

Whether that is a defect depends on loader semantics this spec has not pinned down — a memoizing
loader that marks a node in-progress resolves it; a naive one loops. It is recorded here rather
than fixed, because inventing a rule before the execution model exists is how formats acquire
restrictions nobody can explain later.


## 11. Compression

### 11.1 Profiles

A publisher chooses one; the file records the choice, so a reader never guesses.

| Profile | Regions | Tables | Trade |
|---|---|---|---|
| `None` | raw | raw | every byte in place; largest |
| `Mapped` *(default)* | zstd | raw | graph walked in place, payloads decompressed on touch |
| `Compact` | zstd | zstd | smallest; tables are decoded into memory at open |

A compressed encoding is kept **only when it is smaller**. On a near-empty region a zstd frame
costs more than it saves, and a format that stores the larger of two encodings has chosen
ceremony over bytes.

### 11.2 Commitments are stored at boundaries, not everywhere

A Merkle tree does not store its interior nodes, and neither does this. `hash_idx` is
`0xFFFFFFFF` for a node whose commitment is not stored; a commitment is kept only where
something can be verified on its own:

- the root, always;
- every `Segment` node;
- every node whose `tier` is greater than its parent's — a disclosure step-up, which is exactly
  the set of nodes a loader can fetch without having already fetched more.

Verifying an interior node means verifying the smallest committed subtree that contains it,
which is a subtree the caller has necessarily already loaded. Storing a hash per node cost
6.5 MB across the corpus and bought nothing that was not already implied.

### 11.3 Dictionaries

`SECT_DICTREF` holds the BLAKE3 of the dictionary a file requires. A reader given no dictionary,
or the wrong one, is **refused** (`MissingDictionary`, `WrongDictionary`) rather than allowed to
decode into plausible nonsense.

### 11.4 What compression costs the canonicality rule

§5 says: canonicalize, or you cannot sign. Compression weakens that, and the weakening is worth
stating precisely rather than discovering later.

`serialize(parse(b)) == b` still holds for every file **this** writer produced, because zstd at a
fixed level with a fixed dictionary is deterministic. It does **not** hold across writers or
across zstd versions: two encoders can emit different bytes for the same content, and both are
valid.

That is tolerable only because of where the signature sits. The header commits to the manifest
root, which commits to the subtree hashes, which cover **decompressed** content. Re-compressing
a file therefore changes its bytes and not its meaning, and the signature still verifies —
correctly, because nothing about the skill has changed. What it does mean is that byte identity
is no longer a proxy for content identity, and a system that needs the former must compare
`manifest_root`, not the file.

### 11.5 Regions are no longer padded

Earlier drafts padded each payload region to 8 bytes and then required the pad bytes to be zero
(§10.3). The padding is gone. Removing the bytes removes the malleability surface at its source,
which is better than keeping the surface and checking it. The zero-fill rule still applies to
any byte of a raw region that no node's payload claims — a hostile writer can still leave gaps,
and `Error::UncommittedNonZero` still rejects them.

A compressed region is covered instead by `SECT_REGIONS`, a BLAKE3 over the **stored** bytes,
verified when the region is first decompressed. Without it a compressed region would be committed
by nothing at all, which is §10.3 again in a new costume.
