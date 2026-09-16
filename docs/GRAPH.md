# The Skill Graph Model

Status: draft 0.1 — data model only. No byte layout here; see `SPEC.md`.

## 1. Why a graph, and where it came from

This model was not designed from first principles. It was **derived from 8,776 `SKILL.md`
files** found on one workstation, across plugin caches, project skill directories and
vendored corpora, by measuring what authors actually wrote.

Corpus shape:

| Measure | Value |
|---|---|
| Skills surveyed | 8,776 |
| Body size | median 5,523 B · p95 18,769 B · max 567,484 B · 64.9 MB total |
| Headings per skill | p25 9 · median 12 · p75 20 · p95 49 · max 2,119 |
| Skills with `references/` | 276 |
| Skills with `scripts/` | 184 |
| Skills with `examples/` · `templates/` · `agents/` | 45 · 29 · 37 |

The finding that justifies the whole format: **independently-authored skills converge on a
small, stable set of section roles.** In a 600-skill random sample:

| Authored heading | Occurrences |
|---|---|
| limitations / antipatterns / pitfalls (all spellings) | 393 |
| prerequisites / required inputs / setup | 281 |
| "core workflow pattern" + "step N: …" | 324 |
| when to use (this skill) | 159 |
| output format / what this skill produces | 132 |
| quick reference | 113 |
| overview / purpose | 127 |
| quality checks | 91 |
| tool discovery | 83 |
| references / related skills | 105 |

Nobody agreed on this vocabulary and it recurs anyway, which is the evidence that a typed
structure is already present and Markdown is a lossy flattening of it.

### How strong that evidence actually is

The counts above are matches, not a proportion, and an earlier draft presented them as though
they were the whole story. Measured against the denominator, on two corpora — the one the rules
were fitted to, and a held-out one 14× larger:

| | Headings | Classified |
|---|---|---|
| Corpus A — 8,776 skills, rules fitted on it | 130,528 | **40.6%** |
| Corpus B — 128,292 skills from 236 GitHub repos, held out | 1,737,846 | **26.1%** |

Corpus B excludes, by content hash, every file that also appears in A. The 14-point drop is the
overfitting, measured rather than assumed.

Two things make 26% a ceiling rather than a to-do list. The tail is enormous — 452,057 *distinct*
unclassified heading texts — and it is **multilingual**: `arbeitsweg` (16,024), `prüfprogramm`,
`normenanker`, `ausgabe` all appear in the thousands. An English keyword table cannot reach them,
and 3.8% of headings have no alphanumeric content at all (emoji or symbols).

So the honest claim is much weaker than "skills converge". A recognisable core vocabulary exists
and covers roughly a quarter of headings on unseen skills; the rest is idiosyncratic or not in
English. The model survives that only because **role never drives dispatch** (§2) — an
unrecognised heading becomes plain `Prose` and every structural property still holds. That is the
number that justifies the design decision: a taxonomy required to be exhaustive would have failed
on three quarters of the real world.

`SKILL.md` is not the skill. It is a **linearization** of the skill. `.skill` stores the graph.

## 2. Node kinds

The taxonomy deliberately does **not** mirror the authored vocabulary. Two nodes share a kind
when the runtime *dispatches on them identically*. Vocabulary goes in `role`, which is
descriptive and never load-bearing.

> **Kind determines dispatch. Role determines meaning.**
> Formats that conflate the two grow element zoos forever (see: HTML).

Six kinds, distinguished by what the loader does:

| Kind | Loader behaviour | Derived from |
|---|---|---|
| `Applicability` | **Evaluated during routing**, never injected into context | "when to use", frontmatter `description` |
| `Prose` | **Injected as context** verbatim | overview, procedure, antipatterns, quick reference, examples |
| `Contract` | **Checked as a gate** before/after a node | prerequisites, required inputs, quality checks, output format |
| `Segment` | **Executed** under a declared capability set | `scripts/`, and embedded code fences (§5) |
| `Resource` | **Resolved to bytes on demand** | `references/`, `templates/`, `examples/`, data files |
| `Binding` | **Required from the host**, resolved at activation | tool discovery, MCP deps, `requires:`, sibling skills |

`role: u16` carries the authored semantics — `intent`, `procedure`, `step`, `antipattern`,
`pitfall`, `limitation`, `exemplar`, `cheatsheet`, `citation`, … — for display, search and
lossless round-trip. A reader that does not know a role renders it as plain `Prose`. A reader
that does not know a **kind** must refuse the file.

## 3. Edge kinds

Same test: distinct kinds only where traversal semantics differ. Note that each kind carries a
*different structural invariant*, and the validator enforces each separately.

`CONTAINS` is **not** in this table. The tree is carried by each node's parent pointer plus its
pre-order position, and storing it again as edge records would be a second encoding of one fact
— see `SPEC.md` §10.1.

| Edge | Meaning | Invariant |
|---|---|---|
| `SEQ` | execution order between siblings | **must be a DAG** |
| `GUARDS` | contract → the node it gates; evaluated before entry | source must be `Contract` |
| `NEEDS` | hard dependency; unresolvable ⇒ skill unavailable | target ∈ {`Binding`,`Resource`,`Segment`} |
| `ALT` | ordered fallback when the source fails | ordinal unique per source |
| `CITES` | soft reference, no load obligation | **may cycle, may cross skills** |

`CITES` is the only kind permitted to leave the file, and the only one permitted to cycle.
That asymmetry is the point: everything the loader is *obliged* to follow is acyclic and
bounded, so activation always terminates.

That sentence was false when first written. Per-relation acyclicity does not give it — see
`SPEC.md` §10.6. It holds now because the validator checks `SEQ ∪ GUARDS ∪ NEEDS ∪ ALT` as **one
graph**, which is the thing a loader actually walks, and TLC confirms the claim over that
formulation (`specs/SkillEdges.tla`). Cross-skill composition (measured: 171 `../<skill>/SKILL.md`
links in the 600-skill sample) is expressive but never mandatory.

## 4. Tiers — progressive disclosure as a layout property

Every node carries `tier: u2`.

| Tier | Contents | Residency |
|---|---|---|
| 0 | name, description, `Applicability`, **the tree root** | always resident; the routing plane |
| 1 | `Intent` + procedure backbone | on activation |
| 2 | `Reference`, `Exemplar`, `Resource`, `Segment` bodies | on demand |

Two rules turn this from convention into guarantee:

0. **The root is tier 0.** Monotonicity only composes if the tree's root is the hottest node;
   otherwise the routing predicate cannot be a child of the body. See `SPEC.md` §10.2.
1. **Monotonicity.** For every parent→child pair `p → c`: `tier(c) ≥ tier(p)`.
   Therefore the loaded set is always a *prefix-closed subtree* — it is structurally impossible
   to hold a child without its parent's context.
2. **Tier is a placement directive.** Tier 0 payloads are written to the hot region, tier 2 to
   the cold region. The graph's shape *is* the file's shape.

Budget check against the measured corpus: tier 0 for all 8,776 skills is ~9k fixed records plus
description text — order 3 MB, scannable without touching the 64.9 MB of bodies. Today that
separation is a convention the runtime politely follows; here the loader physically cannot read
a body to decide whether it wants one.

## 5. Segments: promotion, not invention

The corpus says executable content is already there — and already broken.

- 184 of 8,776 skills ship a `scripts/` directory.
- The 600-skill sample contains **687 `bash` and 357 `python` fenced code blocks.**

Extrapolated, the corpus holds on the order of ten thousand executable fragments that have no
identity, no declared capabilities, no signature, and no way to run except a model retyping
them into a shell. They are executable in intent and inert in practice.

A `Segment` node is that fragment, promoted:

```
Segment {
  abi:        wasm32-wasip2 | sh | python3.13 | node22 | native
  trust_class: Portable | HostTrusted        # wasm is the only Portable class
  caps:       bitset + resource list          # net:host, fs:path, env:name, exec:bin
  limits:     mem, cpu_ms, wall_ms
  root:       blake3 root of the segment bytes
  signer:     optional per-segment signature
}
```

The loader refuses any behaviour not declared. `trust_class` is what lets policy say *"from an
unknown publisher, Portable segments only"* — the format can contain a wasm segment; it can only
*label* a shell one. The enclosing `Prose` node keeps its `CONTAINS` edge to the segment, so
linearizing back to Markdown reproduces the original fenced block in place.

## 6. Identity: a Merkle DAG, not a name

Node identity is positional within a file. **Semantic** identity is the BLAKE3 hash of a node's
canonical subtree. Consequences, all free:

- A skill's identity is its root hash.
- A section's identity is its subtree hash — so two skills sharing a "quality checks" section
  store those bytes once, and dedup is automatic across a corpus.
- Any subtree can be proven a member of a signed skill without disclosing its siblings
  (selective disclosure, the same shape as a W3C Verifiable Presentation).
- A diff between two skill versions is a tree diff over stable hashes, not a text diff.

## 7. Canonical form

Exactly one byte encoding per logical graph, or the signatures mean nothing.

- Nodes ordered by `CONTAINS` pre-order; ties by `(tier, role, name_idx)`.
- Edges ordered by `(src, kind, ordinal)`.
- String heap sorted, deduplicated, NFC-normalized, referenced by `u32` index.
- No optional padding; defined padding is zero and is covered by the hash.
- Unknown-`role` values preserved verbatim; unknown-`kind` values rejected.

## 8. Round-trip obligation

`.skill` must reproduce `SKILL.md`. Migration needs it (8,776 files must compile), and trust
needs it (a human must be able to read what they are signing).

Each node therefore carries `src_span` — offset and length into an optional preserved-source
blob — plus its original heading depth. Two properties, both tested, the second being the one
that makes signing sound:

```
render(parse(md))    ≡ md     (modulo normalization)
serialize(parse(b))  ≡ b      (byte-identical; canonicality)
```

## 9. Invariants the validator enforces

A file is well-formed only if all of these hold. These are the TLA+ obligations.

1. The parent pointers form a single tree rooted at node 0, and node 0 is tier 0.
2. `SEQ` is acyclic.
3. `NEEDS` is acyclic and targets only `Binding` / `Resource` / `Segment`.
3a. The **union** `SEQ ∪ GUARDS ∪ NEEDS ∪ ALT` is acyclic. Not implied by 3 and 2 — see
    `SPEC.md` §10.6.
4. `tier` is monotone non-decreasing from parent to child.
5. `GUARDS` sources are `Contract` nodes.
6. `ALT` ordinals are unique per source.
7. Every offset+length lies within the file, no two payload ranges *partially* overlap
   (identical ranges are legal — that is content-addressed dedup), and every payload-region byte
   claimed by no node is zero.
8. Every `Segment` names an ABI the reader knows, or carries the must-understand bit set.
9. The node ordering equals the canonical ordering of §7.
10. Every reserved field is zero.

Invariants 1–6 are graph properties and belong in the model checker; 7–10 are parser properties
and belong in the fuzzer. Both now exist: `specs/check.sh` runs 13 TLC configurations, six of
them canaries that must fail, and `scripts/fuzz.sh` runs the bounded parser fuzzer.

The division is not cosmetic. The fuzzer mutates files and found nothing here; the model checker
reasons about graphs and found two ways activation could fail to terminate.
