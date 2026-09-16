# skillexec

A binary container for agent skills: `SKILL.md` compiled into a signed, mmap-able, lazily
verified graph with executable segments.

- `docs/GRAPH.md` — the data model, derived by measuring 8,776 real `SKILL.md` files
- `docs/SPEC.md` — the byte layout, verification order, and conformance corpus
- `crates/skill-format` — reference reader, validator and canonical writer (`forbid(unsafe)`)

```sh
./scripts/gate.sh                      # fmt, clippy -D warnings, tests, spec/writer agreement
cargo run --example dump_minimal       # the spec's worked example, regenerated
```

## Why

`SKILL.md` is a linearization of a graph that is already there: across 8,776 independently
authored skills, section roles converge hard (393 antipattern/pitfall/limitation headings, 324
workflow steps, 281 prerequisite blocks in a 600-skill sample) without anyone agreeing on
vocabulary. Compiling that graph buys three things Markdown cannot:

- **Progressive disclosure becomes layout.** Tier is monotone from parent to child, so the
  loaded set is always a prefix-closed subtree, and the routing plane is physically separate
  from the bodies. A loader cannot read a body to decide whether it wants one.
- **Executable content gets an identity.** 184 of 8,776 skills ship a `scripts/` directory, but
  a 600-skill sample holds 687 bash and 357 python fenced blocks — executable in intent, inert
  in practice. A `Segment` node gives each one an ABI, a capability set, resource limits, a
  BLAKE3 root and its own signature.
- **Verification precedes parsing.** Signatures cover a fixed 64-byte range that commits to the
  manifest root, which commits to every section and subtree hash. The parser only ever runs on
  bytes already proven to be the publisher's.

Status: draft. The format is not frozen and the on-disk layout will change.
