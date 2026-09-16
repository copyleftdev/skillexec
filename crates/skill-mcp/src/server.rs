//! The MCP surface over a skill library.
//!
//! Three tools, chosen so the transcript shows the format's tier split rather than hiding it:
//! `skill_search` reads routing planes and no bodies, `skill_load` is the first call that
//! decompresses and verifies one, and `skill_segments` reports what a skill would execute and
//! what it declared it needs — which the Markdown it was compiled from cannot say at all.

use std::sync::Arc;

use rmcp::{
    ErrorData, Json, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::library::Library;

const INSTRUCTIONS: &str = "\
A library of agent skills held in `.skill` containers — a binary format in which a skill is a \
typed graph with per-section BLAKE3 commitments, not a Markdown file.

Call `skill_search` first. It answers from each container's routing plane, which is a contiguous \
block at a fixed offset holding only names and descriptions, so it never decompresses or reads a \
skill body. That is what makes searching a large library cheap: 249 bytes and roughly 180 \
nanoseconds per skill, flat, measured across corpora from 8,776 to 680,924 skills.

`skill_load` returns the skill's Markdown. It is the first call that touches a body, and it \
verifies that body against the signed commitment before returning it.

`skill_segments` lists the executable fragments a skill carries, each with its ABI, its declared \
capabilities and its resource limits. Most skills in the wild carry shell and Python in fenced \
code blocks that nothing has ever checked; compiling them gives each fragment an identity and a \
declared capability set, and a `HostTrusted` trust class means the format can label that fragment \
but cannot contain it. Treat an undeclared capability as a reason to refuse, not a detail.";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Words to match against skill names and descriptions. Empty lists everything.
    #[serde(default)]
    pub query: String,
    /// Maximum hits to return. Defaults to 20.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NameParams {
    /// The skill's name, exactly as `skill_search` reported it.
    pub name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchHit {
    pub name: String,
    pub description: String,
    pub container: String,
    /// Weight from matching words, rarer terms counting for more.
    pub lexical: f32,
    /// Cosine similarity between the query and this skill's stored routing vector.
    pub semantic: f32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchResult {
    pub matched: usize,
    pub total_in_library: usize,
    pub hits: Vec<SearchHit>,
    /// How the ranking was produced, so a miss can be read as a miss rather than a failure.
    pub matching: &'static str,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LoadResult {
    pub name: String,
    pub bytes: usize,
    /// The body was checked against its BLAKE3 commitment before being returned.
    pub verified: bool,
    pub markdown: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Segment {
    pub name: String,
    pub abi: String,
    pub trust_class: String,
    pub capabilities: Vec<String>,
    pub mem_kib: u32,
    pub cpu_ms: u32,
    pub bytes: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SegmentsResult {
    pub name: String,
    pub segments: Vec<Segment>,
    pub note: &'static str,
}

#[derive(Clone)]
pub struct SkillServer {
    library: Arc<Library>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for SkillServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillServer").finish_non_exhaustive()
    }
}

fn fail(e: &anyhow::Error) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

#[tool_router(router = tool_router)]
impl SkillServer {
    #[must_use]
    pub fn new(library: Library) -> Self {
        Self {
            library: Arc::new(library),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "skill_search",
        description = "Find skills by name and description. Reads only each container's routing \
                       plane, so it decompresses nothing and touches no skill body. Call this \
                       before loading anything. An empty query lists the whole library."
    )]
    /// # Errors
    /// Fails if a container's routing block is malformed.
    pub fn skill_search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<Json<SearchResult>, ErrorData> {
        let limit = p.limit.unwrap_or(20).clamp(1, 200) as usize;
        let hits = self.library.search(&p.query, limit).map_err(|e| fail(&e))?;
        let total = self.library.catalogue().map_err(|e| fail(&e))?.len();
        Ok(Json(SearchResult {
            matched: hits.len(),
            total_in_library: total,
            hits: hits
                .into_iter()
                .map(|h| SearchHit {
                    name: h.name,
                    description: h.description,
                    container: h.container,
                    lexical: h.lexical,
                    semantic: h.semantic,
                })
                .collect(),
            matching: if self.library.has_vectors() {
                "rarity-weighted word match blended with cosine similarity over stored vectors"
            } else {
                "rarity-weighted word match only; this library carries no vectors"
            },
        }))
    }

    #[tool(
        name = "skill_load",
        description = "Return one skill's Markdown. This is the first read that touches a body: \
                       the payload region is decompressed and checked against its BLAKE3 \
                       commitment before anything is returned."
    )]
    /// # Errors
    /// Fails if the skill is absent, the container does not open, or the body does not match its
    /// commitment.
    pub fn skill_load(
        &self,
        Parameters(p): Parameters<NameParams>,
    ) -> Result<Json<LoadResult>, ErrorData> {
        let markdown = self.library.load(&p.name).map_err(|e| fail(&e))?;
        Ok(Json(LoadResult {
            name: p.name,
            bytes: markdown.len(),
            verified: true,
            markdown,
        }))
    }

    #[tool(
        name = "skill_segments",
        description = "List the executable fragments a skill carries, with the ABI, declared \
                       capabilities and resource limits recorded for each. A fragment that \
                       declares no capabilities is authorized for nothing, and a HostTrusted \
                       trust class means the format can label it but cannot contain it."
    )]
    /// # Errors
    /// Fails if the skill is absent or the container does not open.
    pub fn skill_segments(
        &self,
        Parameters(p): Parameters<NameParams>,
    ) -> Result<Json<SegmentsResult>, ErrorData> {
        let segs = self.library.segments(&p.name).map_err(|e| fail(&e))?;
        Ok(Json(SegmentsResult {
            name: p.name,
            segments: segs
                .into_iter()
                .map(|s| Segment {
                    name: s.name,
                    abi: s.abi,
                    trust_class: s.trust_class,
                    capabilities: s.capabilities,
                    mem_kib: s.mem_kib,
                    cpu_ms: s.cpu_ms,
                    bytes: s.bytes,
                })
                .collect(),
            note: "an empty capability list means the fragment is authorized for nothing",
        }))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SkillServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                    .with_title("Skill library"),
            )
            .with_instructions(INSTRUCTIONS)
    }
}
