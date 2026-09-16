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
| 0x1C | 4 | `hot_len` |
| 0x20 | 4 | `cold_off` |
| 0x24 | 4 | `cold_len` |
| 0x28 | 8 | `reserved` (zero) |

`name` and `desc` are manifest fields, not nodes, because the corpus says they are the only
universal elements (9,008 / 9,035 of 9,212 files). Hoisting them means **routing reads a fixed
offset and never walks the graph.**

### 4.2 Section directory (`sect_count` × 16 bytes)

| Off | Size | Field |
|---|---|---|
| 0x00 | 2 | `id` |
| 0x02 | 2 | `flags` — bit 0 = must-understand |
| 0x04 | 4 | `off` — **relative to manifest start** |
| 0x08 | 4 | `len` |
| 0x0C | 4 | `count` — records, for fixed-width sections |

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
| 0x18 | 4 | `hash_idx` — into HASHES; BLAKE3 root of the canonical subtree |
| 0x1C | 4 | `parent` — node index, `0xFFFFFFFF` for root |

Nodes are stored in `CONTAINS` pre-order. Therefore **`parent < self` for every non-root node**,
and the tree invariant (`GRAPH.md` §9.1) is proven by one comparison per node in a single
forward pass. No auxiliary structure, no cycle detection, no allocation.

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
of a traversal — the same trick as `parent < self`.

### 4.6 Segment record (96 bytes)

| Off | Size | Field |
|---|---|---|
| 0x00 | 32 | `root` — BLAKE3 root of the decompressed segment |
| 0x20 | 4 | `off` · 0x24 4 `len` (stored) · 0x28 4 `orig_len` |
| 0x2C | 2 | `abi` — 1 `wasm32-wasip2`, 2 `sh`, 3 `python3`, 4 `node`, 5 `native` |
| 0x2E | 1 | `codec` — 0 raw, 1 zstd, 2 zstd+dict |
| 0x2F | 1 | `trust_class` — 0 `Portable`, 1 `HostTrusted` |
| 0x30 | 4 | `cap_off` · 0x34 2 `cap_cnt` · 0x36 2 `flags` |
| 0x38 | 4 | `mem_kib` · 0x3C 4 `cpu_ms` · 0x40 4 `wall_ms` |
| 0x44 | 4 | `dict_idx` — shared zstd dictionary, `0xFFFFFFFF` = none |
| 0x48 | 4 | `signer_idx` — into the signature block; `0xFFFFFFFF` = file signature only |
| 0x4C | 20 | `reserved` (zero) |

Capability record (8 B): `kind u16`, `flags u16`, `arg_idx u32` → string heap.
Kinds: `net:host`, `fs:read`, `fs:write`, `env`, `exec`, `clock`, `rand`.

`abi = 1` is the only `Portable` value. A `HostTrusted` segment is *labelled*, never contained —
the format cannot sandbox a shell script, and says so rather than implying otherwise. Policy of
the form *"unknown publisher ⇒ Portable only"* is expressible because the distinction is a
signed field, not a convention.

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

- Structure uncompressed, payloads compressed **per node/segment** — routing never decompresses.
- zstd with a dictionary trained on the skill corpus. Skills are prose with shared vocabulary;
  at median 5.5 KB a shared dictionary is worth more than the compressor's own window.
- Payloads are content-addressed, so identical sections across skills store once.
- Measured fixed overhead: **~230 bytes** (see §7). On a median 5.5 KB skill that is ~4%
  pre-compression. This format is not efficient for a trivial skill; it is efficient at corpus
  scale, where dedup and the separated routing plane are the wins.

## 7. Worked example — minimal valid file

Emitted by the reference writer (`cargo run --example dump_minimal`), so the spec's example and
the implementation cannot drift. 368 bytes; `name="demo"`,
`description="Use when demonstrating the skill format."`; a tier-0 `Prose` root with an empty
payload and one tier-1 child carrying `"Do the thing.\n"`. `sig_count = 0`.

```
00000000  8f 53 4b 4c 0d 0a 1a 0a  01 00 00 00 00 00 00 00   magic, v1.0, feature_flags=0
00000010  70 01 00 00 40 00 00 00  20 01 00 00 00 00 00 00   file_len=368  manifest@64  manifest_len=288  sigs=0
00000020  67 41 ee f8 2f 3f fc 78  59 b8 d4 e1 c3 4a 9e 05   manifest_root (BLAKE3 of the 288 manifest bytes)
00000030  05 4e a2 e8 d4 1f 74 cc  23 2e 9c a3 b5 f0 de e3
00000040  03 00 00 00 01 00 00 00  00 00 00 00 ff ff ff ff   sects=3  name_idx=1  desc_idx=0  version=none
00000050  ff ff ff ff 02 00 00 00  60 01 00 00 00 00 00 00   license=none  node_count=2  hot@352 len=0
00000060  60 01 00 00 10 00 00 00  00 00 00 00 00 00 00 00   cold@352 len=16, reserved zero
00000070  01 00 00 00 60 00 00 00  3c 00 00 00 02 00 00 00   dir: STRINGS  rel@0x60 len=60  n=2
00000080  02 00 00 00 a0 00 00 00  40 00 00 00 02 00 00 00   dir: NODES    rel@0xa0 len=64  n=2
00000090  04 00 00 00 e0 00 00 00  40 00 00 00 02 00 00 00   dir: HASHES   rel@0xe0 len=64  n=2
000000a0  02 00 00 00 00 00 00 00  28 00 00 00 2c 00 00 00   heap: n=2, offsets 0 / 40 / 44
000000b0  55 73 65 20 77 68 65 6e  20 64 65 6d 6f 6e 73 74   "Use when demonst
000000c0  72 61 74 69 6e 67 20 74  68 65 20 73 6b 69 6c 6c   rating the skill
000000d0  20 66 6f 72 6d 61 74 2e  64 65 6d 6f 00 00 00 00    format."  "demo"  pad
000000e0  02 00 00 00 01 00 00 00  ff ff ff ff 00 00 00 00   node0: Prose tier0 role=1 name=none
000000f0  00 00 00 00 00 00 00 00  00 00 00 00 ff ff ff ff          payload 0..0  hash_idx=0  parent=none  <- root
00000100  02 01 02 01 01 00 00 00  ff ff ff ff 00 00 00 00   node1: Prose tier1 flags=COLD depth=1 role=1
00000110  00 00 00 00 0e 00 00 00  01 00 00 00 00 00 00 00          payload 0..14 hash_idx=1 parent=0
00000120  0b 6a a1 83 70 03 20 af  4e ee d9 5f de 5d b7 0e   hashes[0] = subtree(root)
00000130  53 cb 5a 31 51 7c fb 9d  8c 49 18 00 35 31 5e 13
00000140  1f 5d d5 ad 20 f5 3a 75  65 56 8d 05 46 b5 73 f9   hashes[1] = subtree(node1)
00000150  59 86 90 26 94 a3 4a b4  02 f1 fc 1a 6b b3 10 a4
00000160  44 6f 20 74 68 65 20 74  68 69 6e 67 2e 0a 00 00   COLD: "Do the thing.
" + 2 bytes zero pad
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
