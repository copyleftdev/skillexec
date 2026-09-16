//! Compiling an organization: several skills in one container, wired into a graph.
//!
//! A bundle says *which* skills share a file. An organization says how they relate — who hands
//! what to whom, and who refuses. The container has always been able to carry that; `GRAPH.md` §3
//! lists five edge kinds and `validate.rs` enforces an invariant for each. Until this module
//! nothing emitted a single edge, so the graph was a capability no command could reach.
//!
//! Three constraints in the format shape the whole design, and each makes the model more precise
//! than a free-form org chart would be:
//!
//! - **`GUARDS` must originate from a `Contract`.** A gate is therefore a contract, not a label.
//! - **`NEEDS` must target `Binding | Resource | Segment`.** A role cannot need another *role*; it
//!   needs that role's artifact. The format refused the vaguer model.
//! - **`SEQ` must run forward in pre-order.** The file's pre-order *is* the pipeline order, so
//!   members are topologically sorted before emission and a rework loop cannot be a back-edge.
//!
//! What the container cannot do is notice that an organization deadlocks. `GUARDS` sources are
//! gate nodes and `GUARDS` targets are member roots, and those sets never overlap, so the
//! node-level cycle check accepts every organization ever written — including one whose approvals
//! form a ring. That is not a container defect; it is a question about a layer the container does
//! not have. `specs/SkillOrg.tla` states it and TLC confirms both halves: the node-level check is
//! vacuous here, and the role-level check in this module is exactly strong enough.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;
use skill_format::{Builder, EdgeKind, Kind, Profile, Tier};

use crate::classify::{ROLE_ORG_ARTIFACT, ROLE_ORG_CHARTER, ROLE_ORG_GATE, ROLE_ORG_MEMBER};
use crate::compile::build_into;
use crate::md::Document;

/// A manifest as written. Unknown fields are rejected rather than ignored, because a misspelled
/// `guards` that silently compiles to no gate is worse than a file that will not compile.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "member")]
    pub members: Vec<Member>,
    #[serde(default)]
    pub graph: Graph,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    /// What the `[graph]` section calls this member.
    pub id: String,
    /// A path to a `SKILL.md`, or a skill name to resolve out of a library.
    pub skill: String,
    /// What this member hands on. Required if anything `needs` it.
    #[serde(default)]
    pub artifact: Option<String>,
    /// What this member refuses. Required if it `guards` anything.
    #[serde(default)]
    pub gate: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    #[serde(default)]
    pub seq: Vec<[String; 2]>,
    #[serde(default)]
    pub needs: Vec<[String; 2]>,
    #[serde(default)]
    pub guards: Vec<[String; 2]>,
    #[serde(default)]
    pub alt: Vec<[String; 2]>,
    /// Soft reference between members. The only kind permitted to cycle — which makes it the
    /// only way to say "review sends it back to implement", because `SEQ` cannot run backwards.
    #[serde(default)]
    pub cites: Vec<[String; 2]>,
}

#[derive(Debug)]
pub enum Error {
    Parse(String),
    UnknownMember { edge: &'static str, id: String },
    DuplicateMember(String),
    NoMembers,
    MissingArtifact { needs: String, target: String },
    MissingGate { guards: String, target: String },
    ApprovalCycle(Vec<String>),
    SeqCycle(Vec<String>),
    Build(skill_format::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "manifest: {e}"),
            Self::UnknownMember { edge, id } => {
                write!(f, "{edge} names `{id}`, which is not a member")
            }
            Self::DuplicateMember(id) => write!(f, "`{id}` is declared twice"),
            Self::NoMembers => write!(f, "an organization needs at least one member"),
            Self::MissingArtifact { needs, target } => write!(
                f,
                "`{needs}` needs `{target}`, but `{target}` declares no artifact to hand on"
            ),
            Self::MissingGate { guards, target } => write!(
                f,
                "`{guards}` guards `{target}`, but `{guards}` declares no gate — say what it refuses"
            ),
            Self::ApprovalCycle(path) => write!(
                f,
                "circular approval chain: {} — no member can act first, so the organization \
                 cannot ship",
                path.join(" waits on ")
            ),
            Self::SeqCycle(path) => {
                write!(f, "the pipeline loops: {}", path.join(" then "))
            }
            Self::Build(e) => write!(f, "container: {e}"),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = core::result::Result<T, Error>;

/// Reads a manifest.
///
/// # Errors
/// Returns [`Error::Parse`] if the TOML is malformed or carries a field this format does not know.
pub fn parse(src: &str) -> Result<Manifest> {
    toml::from_str(src).map_err(|e| Error::Parse(e.to_string()))
}

/// What `plan` reports and what the compiler emits, in the order the pipeline runs.
#[derive(Debug)]
pub struct Plan {
    /// Member ids in an order where every `seq` edge runs forward.
    pub order: Vec<String>,
    /// `(guard, guarded)` pairs, by member id.
    pub gates: Vec<(String, String)>,
}

impl Manifest {
    /// Member ids in declaration order.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.members.iter().map(|m| m.id.as_str()).collect()
    }

    /// Checks every edge endpoint, every required artifact and gate, and both cycle properties.
    ///
    /// The approval check is this module's to make. TLC shows the container accepts every
    /// organization, deadlocked or not, so if this does not reject a circular approval chain
    /// nothing will (`specs/SkillOrg.tla`).
    ///
    /// # Errors
    /// Names the member and the edge at fault rather than reporting a node index.
    pub fn plan(&self) -> Result<Plan> {
        if self.members.is_empty() {
            return Err(Error::NoMembers);
        }
        let mut seen = HashSet::new();
        for m in &self.members {
            if !seen.insert(m.id.as_str()) {
                return Err(Error::DuplicateMember(m.id.clone()));
            }
        }

        let index: HashMap<&str, usize> = self
            .members
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.as_str(), i))
            .collect();

        // `shape.artifact` and `review.gate` are accepted as explicit synonyms for the bare id,
        // because the manifest reads better either way and both name the same member.
        let resolve = |raw: &str, edge: &'static str| -> Result<usize> {
            let id = raw.split_once('.').map_or(raw, |(head, tail)| match tail {
                "artifact" | "gate" | "root" => head,
                _ => raw,
            });
            index.get(id).copied().ok_or_else(|| Error::UnknownMember {
                edge,
                id: raw.to_string(),
            })
        };

        let mut seq = vec![Vec::new(); self.members.len()];
        for e in &self.graph.seq {
            let (a, b) = (resolve(&e[0], "seq")?, resolve(&e[1], "seq")?);
            seq[a].push(b);
        }

        let mut approvals = vec![Vec::new(); self.members.len()];
        let mut gates = Vec::new();
        for e in &self.graph.guards {
            let (a, b) = (resolve(&e[0], "guards")?, resolve(&e[1], "guards")?);
            if self.members[a].gate.is_none() {
                return Err(Error::MissingGate {
                    guards: self.members[a].id.clone(),
                    target: self.members[b].id.clone(),
                });
            }
            approvals[a].push(b);
            gates.push((self.members[a].id.clone(), self.members[b].id.clone()));
        }

        let mut handoffs = vec![Vec::new(); self.members.len()];
        for e in &self.graph.needs {
            let (a, b) = (resolve(&e[0], "needs")?, resolve(&e[1], "needs")?);
            if self.members[b].artifact.is_none() {
                return Err(Error::MissingArtifact {
                    needs: self.members[a].id.clone(),
                    target: self.members[b].id.clone(),
                });
            }
            // Reversed on purpose: `a needs b` means b must have acted, so b comes first.
            handoffs[b].push(a);
        }
        for e in &self.graph.alt {
            resolve(&e[0], "alt")?;
            resolve(&e[1], "alt")?;
        }
        for e in &self.graph.cites {
            resolve(&e[0], "cites")?;
            resolve(&e[1], "cites")?;
        }

        // Three relations order members and they go in one graph, which is the lesson SPEC.md
        // §10.6 recorded about checking relations separately: `seq` hands the pipeline on,
        // `needs` waits for an artifact, and `guards` is evaluated *before entry* — so a gate
        // runs ahead of what it gates. A review that happens after implementing therefore gates
        // the stage that follows it, not the one behind it; gating backwards is the circular
        // approval chain this check exists to refuse.
        let mut obligation = seq.clone();
        for (a, targets) in approvals.iter().enumerate() {
            obligation[a].extend(targets.iter().copied());
        }
        for (a, targets) in handoffs.iter().enumerate() {
            obligation[a].extend(targets.iter().copied());
        }
        if let Some(cycle) = find_cycle(&obligation) {
            let path: Vec<String> = cycle.iter().map(|&i| self.members[i].id.clone()).collect();
            return if self.graph.guards.is_empty() {
                Err(Error::SeqCycle(path))
            } else {
                Err(Error::ApprovalCycle(path))
            };
        }

        let order = topological(&obligation)
            .into_iter()
            .map(|i| self.members[i].id.clone())
            .collect();
        Ok(Plan { order, gates })
    }
}

/// Emits the container.
///
/// `docs` supplies each member's compiled document, keyed by member id; the caller resolves those
/// from disk or out of a library, which is not this module's business.
///
/// # Errors
/// Any planning failure, or any violation the writer detects while canonicalising the graph.
///
/// # Panics
/// Panics only if the plan names a member the manifest does not declare, which `plan` maintains
/// as an invariant.
pub fn compile(
    manifest: &Manifest,
    docs: &BTreeMap<String, Document>,
    profile: Profile,
    dict: Option<&skill_format::Dictionary>,
    vectors: Vec<Vec<f32>>,
) -> Result<Vec<u8>> {
    let plan = manifest.plan()?;

    let mut b = Builder::bundle().profile(profile);
    if !vectors.is_empty() {
        b = b.vectors(vectors);
    }
    if let Some(d) = dict {
        b = b.dictionary(d.clone());
    }

    // Routing entry 0 is the organization. Routing entries carry the only name and description in
    // the file, so without this the org's own name has nowhere to live.
    let charter = b.add_skill(manifest.name.clone(), manifest.description.clone());
    b.set_role(charter, ROLE_ORG_CHARTER);

    // Members in pipeline order, so every SEQ edge runs forward in pre-order as §3 requires.
    let mut root = HashMap::new();
    let mut artifact = HashMap::new();
    let mut gate = HashMap::new();
    for id in &plan.order {
        let m = manifest
            .members
            .iter()
            .find(|m| &m.id == id)
            .expect("plan only names declared members");
        let name = doc_name(docs.get(id), m);
        let desc = doc_desc(docs.get(id));
        let r = b.add_skill(name, desc);
        b.set_role(r, ROLE_ORG_MEMBER);
        if let Some(doc) = docs.get(id) {
            build_into(&mut b, r, doc);
        }
        if let Some(text) = &m.artifact {
            artifact.insert(
                id.clone(),
                b.child(
                    r,
                    Kind::Binding,
                    Tier::Body,
                    ROLE_ORG_ARTIFACT,
                    Some("artifact"),
                    text.as_bytes().to_vec(),
                    0,
                ),
            );
        }
        if let Some(text) = &m.gate {
            gate.insert(
                id.clone(),
                b.child(
                    r,
                    Kind::Contract,
                    Tier::Body,
                    ROLE_ORG_GATE,
                    Some("gate"),
                    text.as_bytes().to_vec(),
                    0,
                ),
            );
        }
        root.insert(id.clone(), r);
    }

    let member_of = |raw: &str| -> String {
        raw.split_once('.')
            .map_or(raw, |(head, tail)| match tail {
                "artifact" | "gate" | "root" => head,
                _ => raw,
            })
            .to_string()
    };

    for (i, e) in manifest.graph.seq.iter().enumerate() {
        let (a, z) = (member_of(&e[0]), member_of(&e[1]));
        b.edge(root[&a], EdgeKind::Seq, root[&z], ord(i), 0);
    }
    // NEEDS targets the artifact, never the member: the format restricts it to a Binding, and
    // naming the artifact is the truer statement anyway.
    for (i, e) in manifest.graph.needs.iter().enumerate() {
        let (a, z) = (member_of(&e[0]), member_of(&e[1]));
        b.edge(root[&a], EdgeKind::Needs, artifact[&z], ord(i), 0);
    }
    // GUARDS originates at the gate, never the member: a gate is a Contract, and the contract is
    // the thing that refuses.
    for (i, e) in manifest.graph.guards.iter().enumerate() {
        let (a, z) = (member_of(&e[0]), member_of(&e[1]));
        b.edge(gate[&a], EdgeKind::Guards, root[&z], ord(i), 0);
    }
    for (i, e) in manifest.graph.alt.iter().enumerate() {
        let (a, z) = (member_of(&e[0]), member_of(&e[1]));
        b.edge(root[&a], EdgeKind::Alt, root[&z], ord(i), 0);
    }
    // CITES is the only kind that may cycle, and a rework loop is exactly a cycle: SEQ must run
    // forward in pre-order, so "review sends it back to implement" is unrepresentable as SEQ and
    // representable as CITES. The format made rework a soft reference rather than an obligation,
    // which is the right answer — a loader that had to follow it would not terminate.
    for (i, e) in manifest.graph.cites.iter().enumerate() {
        let (a, z) = (member_of(&e[0]), member_of(&e[1]));
        b.edge(root[&a], EdgeKind::Cites, root[&z], ord(i), 0);
    }

    b.build().map_err(Error::Build)
}

/// ALT ordinals must be unique per source; the others are positional and wrap harmlessly.
fn ord(i: usize) -> u8 {
    u8::try_from(i % 256).unwrap_or(0)
}

fn doc_name(doc: Option<&Document>, m: &Member) -> String {
    doc.and_then(|d| d.get("name")).unwrap_or(&m.id).to_string()
}

fn doc_desc(doc: Option<&Document>) -> String {
    doc.and_then(|d| d.get("description"))
        .unwrap_or("")
        .to_string()
}

/// Returns the members on a cycle, in the order they wait on each other.
fn find_cycle(adj: &[Vec<usize>]) -> Option<Vec<usize>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Grey,
        Black,
    }
    fn walk(
        n: usize,
        adj: &[Vec<usize>],
        mark: &mut [Mark],
        stack: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        mark[n] = Mark::Grey;
        stack.push(n);
        for &m in &adj[n] {
            match mark[m] {
                Mark::Grey => {
                    // Report from where the cycle closes, not from where the walk started.
                    let at = stack.iter().position(|&x| x == m).unwrap_or(0);
                    let mut path = stack[at..].to_vec();
                    path.push(m);
                    return Some(path);
                }
                Mark::White => {
                    if let Some(p) = walk(m, adj, mark, stack) {
                        return Some(p);
                    }
                }
                Mark::Black => {}
            }
        }
        stack.pop();
        mark[n] = Mark::Black;
        None
    }

    let mut mark = vec![Mark::White; adj.len()];
    for n in 0..adj.len() {
        if mark[n] == Mark::White {
            let mut stack = Vec::new();
            if let Some(p) = walk(n, adj, &mut mark, &mut stack) {
                return Some(p);
            }
        }
    }
    None
}

/// Kahn's algorithm, ties broken by declaration order so a manifest compiles to the same bytes
/// every time. Only called after [`find_cycle`] has returned `None`.
fn topological(adj: &[Vec<usize>]) -> Vec<usize> {
    let mut indegree = vec![0usize; adj.len()];
    for targets in adj {
        for &t in targets {
            indegree[t] += 1;
        }
    }
    let mut ready: Vec<usize> = (0..adj.len()).filter(|&i| indegree[i] == 0).collect();
    let mut out = Vec::with_capacity(adj.len());
    while let Some(&n) = ready.iter().min() {
        ready.retain(|&x| x != n);
        out.push(n);
        for &t in &adj[n] {
            indegree[t] -= 1;
            if indegree[t] == 0 {
                ready.push(t);
            }
        }
    }
    out
}

/// The members a plan reports, with what each hands on and what it refuses.
///
/// # Errors
/// Any planning failure.
pub fn describe(manifest: &Manifest) -> Result<Plan> {
    manifest.plan()
}
