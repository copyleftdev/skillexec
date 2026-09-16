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

use skill_format::{Dictionary, Skill, TrustPolicy};

pub struct Library {
    containers: Vec<(PathBuf, Vec<u8>)>,
    dict: Option<Dictionary>,
    policy: TrustPolicy,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub name: String,
    pub description: String,
    pub container: String,
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
        Ok(Self {
            containers,
            dict,
            policy: TrustPolicy::permissive(),
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
                });
            }
        }
        Ok(out)
    }

    /// Substring match over names and descriptions.
    ///
    /// This is deliberately not a semantic search. It reports what the routing plane literally
    /// says, so that a miss means the corpus does not describe the thing rather than that a
    /// model failed to connect two words.
    ///
    /// # Errors
    /// Fails if a container's routing block is malformed.
    pub fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<Hit>> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|t| !t.is_empty())
            .collect();
        let mut hits: Vec<(usize, Hit)> = Vec::new();
        for h in self.catalogue()? {
            let hay = format!("{} {}", h.name, h.description).to_lowercase();
            let score = terms.iter().filter(|t| hay.contains(t.as_str())).count();
            if terms.is_empty() || score > 0 {
                hits.push((score, h));
            }
        }
        // Ties break on name, so repeated calls give the same answer.
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        Ok(hits.into_iter().take(limit).map(|(_, h)| h).collect())
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
