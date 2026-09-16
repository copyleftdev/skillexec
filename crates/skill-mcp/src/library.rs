//! A set of `.skill` containers, searched through their routing planes.
//!
//! Containers are held as raw bytes and reopened per request. That sounds wasteful and is not:
//! `Skill::routing_view` reads the 64-byte header and the routing block that follows it, and
//! nothing else — measured at 183 ns and 249 bytes per skill on a corpus of 680,924. Holding
//! parsed `Skill` values instead would mean a self-referential struct to save work that does not
//! exist.
//!
//! The split between `search` and `load` is the format's tier split made visible: search never
//! touches a payload region, so a body is decompressed only once something has asked for it.

use std::path::{Path, PathBuf};

/// How much of the ranking meaning contributes when both signals are available.
///
/// Measured, not chosen. Sweeping this against 49 labelled queries (`eval/queries.tsv`) over a
/// 192-skill library gives an inverted U: 44.9% top-1 at pure lexical, 65.3% at 0.7, 55.1% at
/// pure semantic. The blend beats both ends.
///
/// What the sample can and cannot settle, by required sample size at 80% power:
///
/// | claim | observed | n needed | n had |
/// |---|---|---|---|
/// | blend beats pure lexical | 44.9% → 65.3% | 46 | 49 |
/// | blend beats pure semantic | 55.1% → 65.3% | 181 | 49 |
/// | 0.7 beats 0.6 | 63.3% → 65.3% | 4,504 | 49 |
///
/// So the first claim is supported and the other two are not. The optimum is a plateau across
/// roughly 0.5–0.8, not a point, and 0.7 is taken from the middle of it because it happens to
/// lead on both top-1 and MRR — not because those few points mean anything.
const SEMANTIC_WEIGHT: f32 = 0.7;
use std::sync::Mutex;

use skill_format::{Dictionary, Skill, TrustPolicy, embed};

pub struct Library {
    containers: Vec<(PathBuf, Vec<u8>)>,
    dict: Option<Dictionary>,
    policy: TrustPolicy,
    /// Built on first semantic query, and only when some container actually carries vectors.
    /// Loading an ONNX model costs about half a second; a lexical-only library never pays it.
    embedder: Mutex<Option<skill_embed::Embedder>>,
    has_vectors: bool,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub name: String,
    pub description: String,
    pub container: String,
    /// How much of the ranking came from matching words, and how much from meaning. Reported so
    /// a caller can see why something ranked rather than having to trust the order.
    pub lexical: f32,
    pub semantic: f32,
}

#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub name: String,
    pub abi: String,
    pub trust_class: String,
    pub capabilities: Vec<String>,
    pub mem_kib: u32,
    pub cpu_ms: u32,
    pub bytes: u32,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("containers", &self.containers.len())
            .field("dictionary", &self.dict.is_some())
            .finish_non_exhaustive()
    }
}

impl Library {
    /// Loads every `.skill` file at `path`, which may be a file or a directory.
    ///
    /// # Errors
    /// Fails if the path cannot be read, or if it holds no containers.
    pub fn open(path: &Path, dict: Option<Dictionary>) -> anyhow::Result<Self> {
        let mut containers = Vec::new();
        if path.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(path)?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "skill"))
                .collect();
            entries.sort();
            for p in entries {
                let bytes = std::fs::read(&p)?;
                containers.push((p, bytes));
            }
        } else {
            containers.push((path.to_path_buf(), std::fs::read(path)?));
        }
        anyhow::ensure!(
            !containers.is_empty(),
            "no .skill containers at {}",
            path.display()
        );
        let has_vectors = containers
            .iter()
            .any(|(_, b)| Skill::routing_view_with_embeddings(b).is_ok_and(|(_, e)| e.is_some()));
        Ok(Self {
            containers,
            dict,
            policy: TrustPolicy::permissive(),
            embedder: Mutex::new(None),
            has_vectors,
        })
    }

    /// Every skill the library carries, read from routing planes only.
    ///
    /// # Errors
    /// Fails if a container's routing block is malformed.
    pub fn catalogue(&self) -> anyhow::Result<Vec<Hit>> {
        let mut out = Vec::new();
        for (path, bytes) in &self.containers {
            let container = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let entries = Skill::routing_view(bytes)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            for e in entries {
                out.push(Hit {
                    name: e.name.to_string(),
                    description: e.description.to_string(),
                    container: container.clone(),
                    lexical: 0.0,
                    semantic: 0.0,
                });
            }
        }
        Ok(out)
    }

    /// Ranks skills by matching words and, when the library carries vectors, by meaning.
    ///
    /// Lexical matching is precise when it fires and blind to paraphrase. It took two rounds to
    /// make it honest — substring matching let `i` hit "interactive", and counting matched terms
    /// tied "make" with "aggressive" — and even corrected it cannot answer "how do I make this
    /// UI less aggressive" with `quieter`, because three other skills quote the phrase "how do I
    /// set this up" and match the question form better. That is the ceiling of lexical search
    /// over prose, not a bug in it.
    ///
    /// So both signals are used, each normalised across the candidates for this query, and both
    /// are reported on every hit. A search that cannot be asked why it ranked something is a
    /// search you have to take on faith.
    ///
    /// # Errors
    /// Fails if a container's routing block is malformed.
    pub fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<Hit>> {
        self.search_weighted(query, limit, SEMANTIC_WEIGHT)
    }

    /// As [`Library::search`], with the blend fixed by the caller. Used to sweep the weight
    /// against a labelled query set rather than choosing it by taste.
    ///
    /// # Errors
    /// Fails if a container's routing block is malformed.
    pub fn search_weighted(
        &self,
        query: &str,
        limit: usize,
        semantic_weight: f32,
    ) -> anyhow::Result<Vec<Hit>> {
        let terms = terms_of(query);
        let catalogue = self.catalogue()?;
        // A query that reduces to no usable terms is no query. Listing is honest; inventing a
        // ranking out of stopwords is not.
        if terms.is_empty() {
            let mut all = catalogue;
            all.sort_by(|a, b| a.name.cmp(&b.name));
            return Ok(all.into_iter().take(limit).collect());
        }

        let lexical = Self::lexical_scores(&catalogue, &terms);
        let semantic = self.semantic_scores(query, catalogue.len());

        let lex_max = lexical.iter().copied().fold(0.0f32, f32::max);
        let sem_max = semantic.iter().copied().fold(0.0f32, f32::max);

        let mut scored: Vec<(f32, Hit)> = Vec::new();
        for (i, mut h) in catalogue.into_iter().enumerate() {
            let lex = if lex_max > 0.0 {
                lexical[i] / lex_max
            } else {
                0.0
            };
            let sem = if sem_max > 0.0 {
                semantic[i] / sem_max
            } else {
                0.0
            };
            if lex <= 0.0 && sem <= 0.0 {
                continue;
            }
            h.lexical = lexical[i];
            h.semantic = semantic[i];
            scored.push((semantic_weight * sem + (1.0 - semantic_weight) * lex, h));
        }
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.name.cmp(&b.1.name))
        });
        Ok(scored.into_iter().take(limit).map(|(_, h)| h).collect())
    }

    /// Inverse document frequency over the library itself: a term is worth what it rules out.
    fn lexical_scores(catalogue: &[Hit], terms: &[String]) -> Vec<f32> {
        #[allow(clippy::cast_precision_loss)]
        let total = catalogue.len().max(1) as f32;
        let docs: Vec<(Vec<String>, Vec<String>)> = catalogue
            .iter()
            .map(|h| (words_of(&h.name), words_of(&h.description)))
            .collect();

        let weights: Vec<f32> = terms
            .iter()
            .map(|t| {
                #[allow(clippy::cast_precision_loss)]
                let df = docs
                    .iter()
                    .filter(|(n, d)| hits_word(n, t) || hits_word(d, t))
                    .count() as f32;
                if df == 0.0 {
                    0.0
                } else {
                    (total / df).ln().max(0.0) + 0.1
                }
            })
            .collect();

        docs.iter()
            .map(|(name_words, desc_words)| {
                let mut score = 0.0;
                for (t, w) in terms.iter().zip(&weights) {
                    // A name is a claim; a description is prose that brushes against any word.
                    if hits_word(name_words, t) {
                        score += 3.0 * w;
                    } else if hits_word(desc_words, t) {
                        score += *w;
                    }
                }
                score
            })
            .collect()
    }

    /// Cosine similarity against the stored routing vectors, or zeros when there are none.
    ///
    /// Negative similarities are clamped away: a vector pointing the other way is not evidence
    /// against a skill, it is the absence of evidence for it.
    fn semantic_scores(&self, query: &str, n: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; n];
        if !self.has_vectors {
            return out;
        }
        let Ok(mut guard) = self.embedder.lock() else {
            return out;
        };
        if guard.is_none() {
            match skill_embed::Embedder::new() {
                Ok(e) => *guard = Some(e),
                // A missing model degrades search to lexical rather than failing the call.
                Err(e) => {
                    tracing::warn!(error = %e, "semantic search unavailable; lexical only");
                    return out;
                }
            }
        }
        let Some(model) = guard.as_mut() else {
            return out;
        };
        let Ok(q) = model.embed_one(query) else {
            return out;
        };

        let mut at = 0usize;
        for (_, bytes) in &self.containers {
            let Ok((entries, emb)) = Skill::routing_view_with_embeddings(bytes) else {
                continue;
            };
            for i in 0..entries.len() {
                if at >= n {
                    break;
                }
                if let Some(e) = emb.as_ref().and_then(|e| e.get(i)) {
                    out[at] = embed::cosine(&q, e).max(0.0);
                }
                at += 1;
            }
        }
        out
    }

    /// Whether this library can answer semantically at all.
    #[must_use]
    pub fn has_vectors(&self) -> bool {
        self.has_vectors
    }

    fn find(&self, name: &str) -> anyhow::Result<(&[u8], u32)> {
        for (path, bytes) in &self.containers {
            let entries = Skill::routing_view(bytes)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            if let Some(e) = entries.iter().find(|e| e.name == name) {
                return Ok((bytes, e.root));
            }
        }
        anyhow::bail!("no skill named {name:?}")
    }

    /// Renders one skill back to Markdown. This is the first read that touches a body.
    ///
    /// # Errors
    /// Fails if the skill is absent, the container does not verify, or a payload does not match
    /// its commitment.
    pub fn load(&self, name: &str) -> anyhow::Result<String> {
        let (bytes, root) = self.find(name)?;
        let s = Skill::open_with(bytes, &self.policy, self.dict.as_ref())
            .map_err(|e| anyhow::anyhow!("open: {e}"))?;
        // Verify before handing anything back: the commitment is the reason to use this format
        // rather than a directory of Markdown.
        s.verified_payload(root)
            .map_err(|e| anyhow::anyhow!("verify: {e}"))?;
        skillc::render_from(&s, root).map_err(|e| anyhow::anyhow!("render: {e}"))
    }

    /// What a skill would execute, and what it declares it needs to do so.
    ///
    /// # Errors
    /// Fails if the skill is absent or the container does not open.
    pub fn segments(&self, name: &str) -> anyhow::Result<Vec<SegmentInfo>> {
        let (bytes, root) = self.find(name)?;
        let s = Skill::open_with(bytes, &self.policy, self.dict.as_ref())
            .map_err(|e| anyhow::anyhow!("open: {e}"))?;

        let mut out = Vec::new();
        let mut seg_index = 0u32;
        for (i, n) in s.nodes.iter().enumerate() {
            if n.kind != skill_format::Kind::Segment {
                continue;
            }
            let idx = seg_index;
            seg_index += 1;
            if !in_subtree(&s, u32::try_from(i).unwrap_or(0), root) {
                continue;
            }
            let Ok(rec) = s.manifest.segment(idx) else {
                continue;
            };
            let caps: Vec<String> = (0..u32::from(rec.cap_cnt))
                .filter_map(|k| s.manifest.cap(rec.cap_off + k).ok())
                .map(|(kind, _, arg)| {
                    let label = skill_format::cap_kind::name(kind);
                    match s.manifest.string(arg) {
                        Ok(a) if !a.is_empty() => format!("{label}:{a}"),
                        _ => label.to_string(),
                    }
                })
                .collect();
            out.push(SegmentInfo {
                name: s
                    .manifest
                    .string(n.name_idx)
                    .unwrap_or("<unnamed>")
                    .to_string(),
                abi: abi_name(rec.abi).to_string(),
                trust_class: match rec.trust_class {
                    skill_format::TrustClass::Portable => "Portable",
                    skill_format::TrustClass::HostTrusted => "HostTrusted",
                }
                .to_string(),
                capabilities: caps,
                mem_kib: rec.mem_kib,
                cpu_ms: rec.cpu_ms,
                bytes: rec.orig_len,
            });
        }
        Ok(out)
    }

    #[must_use]
    pub fn container_count(&self) -> usize {
        self.containers.len()
    }
}

/// Query tokens worth matching on. Anything shorter than three characters is noise: it matches
/// most of the corpus and says nothing about what the caller wants.
fn terms_of(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect()
}

fn words_of(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A term matches a whole word, or a word beginning with it when the term is long enough for a
/// prefix to mean something — so "test" reaches "testing" and "ui" reaches nothing it should not.
fn hits_word(words: &[String], term: &str) -> bool {
    words
        .iter()
        .any(|w| w == term || (term.chars().count() >= 4 && w.starts_with(term)))
}

fn in_subtree(s: &Skill<'_>, mut node: u32, root: u32) -> bool {
    loop {
        if node == root {
            return true;
        }
        let Some(n) = s.nodes.get(node as usize) else {
            return false;
        };
        if n.parent == u32::MAX {
            return false;
        }
        node = n.parent;
    }
}

fn abi_name(abi: skill_format::Abi) -> &'static str {
    match abi {
        skill_format::Abi::Wasm32Wasip2 => "wasm32-wasip2",
        skill_format::Abi::Sh => "sh",
        skill_format::Abi::Python3 => "python3",
        skill_format::Abi::Node => "node",
        skill_format::Abi::Native => "native",
        skill_format::Abi::Wasm32Core => "wasm32-core",
    }
}
