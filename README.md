# skillexec

A binary container for agent skills: `SKILL.md` compiled into a signed, mmap-able, lazily
verified graph with executable segments.

- `docs/GRAPH.md` — the data model, derived by measuring 8,776 real `SKILL.md` files
- `docs/SPEC.md` — the byte layout, verification order, conformance corpus, and the defects
  that writing the implementation forced back into the spec (§10)
- `docs/RESULTS.md` — what compiling and fuzzing the corpus actually measured
- `crates/skill-format` — reference reader, validator and canonical writer (`forbid(unsafe)`)
- `crates/skillc` — `SKILL.md` → `.skill` compiler, and the renderer that proves the round-trip
- `specs/` — TLA+ models of the graph invariants, with canaries that must fail

```sh
./scripts/gate.sh                    # fmt, clippy -D warnings, tests, 5s fuzz, spec agreement
./scripts/fuzz.sh 600 4              # long fuzz, capped at one core and 2G under systemd
cargo run --example dump_minimal     # the spec's worked example, regenerated
cargo run --release --bin skillc -- corpus <list-of-SKILL.md-paths>
./specs/check.sh                     # 13 TLC runs, 6 of them canaries, ~90s
```

All 8,776 `SKILL.md` files on this workstation compile, verify, and render back — 99.85%
byte-exact, 100% modulo trailing whitespace. 46M fuzz inputs, zero panics, 31 MB peak RSS. TLC
proves the validator's single forward pass equivalent to the semantic tree invariants over every
five-node graph, and found two ways activation could fail to terminate that the fuzzer could
not reach.

## Why

`SKILL.md` is a linearization of a graph that is already there: across 8,776 independently
authored skills, section roles converge hard (393 antipattern/pitfall/limitation headings, 324
workflow steps, 281 prerequisite blocks in a 600-skill sample) without anyone agreeing on
vocabulary. Compiling that graph buys three things Markdown cannot:

- **Progressive disclosure becomes layout.** Tier is monotone from parent to child, so the
  loaded set is always a prefix-closed subtree, and the routing plane is physically separate
  from the bodies. A loader cannot read a body to decide whether it wants one.
- **Executable content gets an identity.** 184 of 8,776 skills ship a `scripts/` directory, yet
  compiling the corpus lifts **19,534** executable fragments out of fenced code blocks —
  executable in intent, inert in practice. A `Segment` node gives each one an ABI, a capability
  set, resource limits, a BLAKE3 root and its own signature, and authorizes it for nothing.
- **Verification precedes parsing.** Signatures cover a fixed 64-byte range that commits to the
  manifest root, which commits to every section and subtree hash. The parser only ever runs on
  bytes already proven to be the publisher's.

Status: draft. The format is not frozen and the on-disk layout will change.
