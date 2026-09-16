# skillexec

[![Tip my tokens](https://tokentip.to/badge/copyleftdev.svg?logo=1)](https://tokentip.to/@copyleftdev)

A binary container for agent skills: `SKILL.md` compiled into a signed, mmap-able, lazily
verified graph with executable segments.

- `docs/GRAPH.md` — the data model, derived by measuring 8,776 real `SKILL.md` files
- `docs/SPEC.md` — the byte layout, verification order, conformance corpus, and the defects
  that writing the implementation forced back into the spec (§10)
- `docs/RESULTS.md` — every measured number, including the ones that went against the design
- `crates/skill-format` — reference reader, validator and canonical writer (`forbid(unsafe)`)
- `crates/skillc` — `SKILL.md` → `.skill` compiler, and the renderer that proves the round-trip
- `crates/skill-run` — capability-gated, resource-bounded wasm executor for segments
- `crates/skill-mcp` — MCP server over a library of containers: hybrid lexical + semantic
  routing-plane search, lazy verified loading, segment capability disclosure
- `crates/skill-embed` — sentence embeddings for routing planes (ONNX MiniLM, 384 dims)
- `crates/corpusctl` — corpus assembly: supervised clones, pruning, BLAKE3 dedup, experiment runs
- `eval/queries.tsv` — 49 labelled queries; `skill-eval` sweeps the ranking weight against them
- `specs/` — TLA+ models of the graph invariants, with canaries that must fail

```sh
./scripts/gate.sh                    # fmt, clippy -D warnings, tests, 5s fuzz, spec agreement
./scripts/fuzz.sh 600 4              # long fuzz, capped at one core and 2G under systemd
cargo run --example dump_minimal     # the spec's worked example, regenerated
./specs/check.sh                     # 13 TLC runs, 6 of them canaries, ~90s

# assemble a corpus and run every experiment against it
corpusctl clone --list repos.tsv --dest repos --jobs 8 --budget-gb 40
corpusctl build --dest repos --out corpus.txt --exclude other-corpus.txt
corpusctl run   --list corpus.txt --out results

# many skills in one signed, verifiable file
skillc bundle <list-of-SKILL.md> personas.skill --embed
skillc show   personas.skill

# serve that library to an agent over MCP
SKILL_LIBRARY=personas.skill skill-mcp

# or drive one experiment directly
skillc roles  <list>    # does the heading taxonomy generalise?
skillc cas    <list>    # what a corpus-wide content-addressed store would save
skillc route  <list>    # routing cost against zstd'd Markdown
```

**128,292 held-out skills from GitHub** compile, verify and render back with zero failures,
alongside the 8,776 local ones the design came from. Compressed they are **51% smaller than
their source** and route **160× faster** than the same skills stored as zstd'd Markdown, at
1.6× the bytes.

The heading taxonomy is the part that does *not* hold up: **40.6% of headings recognised on the
corpus it was fitted to, 26.1% on the held-out one**, with a long multilingual tail. Nothing
breaks, because role never reaches the dispatch path — which is the measurement that justifies
that separation rather than a claim about it.

46M fuzz inputs, zero panics. TLC proves the validator's single forward pass equivalent to the
semantic tree invariants over every five-node graph, and found two ways activation could fail to
terminate that the fuzzer could not reach.

## Why

`SKILL.md` is a linearization of a graph that is already there. Independently authored skills
reuse a recognisable core vocabulary — antipatterns, workflow steps, prerequisites — without
anyone agreeing on it. That vocabulary turns out to cover about a quarter of headings on skills
the model has never seen, which is *why* the graph is typed by dispatch rather than by
vocabulary. Compiling it buys three things Markdown cannot:

- **Progressive disclosure becomes layout.** Tier is monotone from parent to child, so the
  loaded set is always a prefix-closed subtree, and the routing plane is physically separate
  from the bodies — separately compressed, so a router decompresses nothing. A loader cannot
  read a body to decide whether it wants one. Measured: 182 ns and 364 bytes touched to route one
  skill, against 29 µs to decompress its Markdown and read the frontmatter.
- **Executable content gets an identity.** 184 of 8,776 skills ship a `scripts/` directory, yet
  compiling the corpus lifts **19,534** executable fragments out of fenced code blocks —
  executable in intent, inert in practice. A `Segment` node gives each one an ABI, a capability
  set, resource limits, a BLAKE3 root and its own signature, and authorizes it for nothing.
  `crates/skill-run` then enforces that set at **link time**: a wasm module reaches the host only
  through its imports, so an undeclared import is never satisfied and the module never
  instantiates.
- **One file can carry many skills.** A bundle roots each skill at a child of node 0 and lists
  them all in the routing block, so choosing between forty personas costs one read of 13,954
  bytes and touches no body. Forty real skills compile to a single 166 KB file, 61% smaller than
  their source.
- **Verification precedes parsing.** Signatures cover a fixed 64-byte range that commits to the
  manifest root, which commits to every section and subtree hash. The parser only ever runs on
  bytes already proven to be the publisher's.

Status: draft. The format is not frozen and the on-disk layout will change.
