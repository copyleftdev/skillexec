//! Sentence embeddings for skill names and descriptions.
//!
//! Kept in its own crate because it drags in ONNX Runtime, which nothing else in this workspace
//! needs. The format stores vectors; only the compiler and the server ever produce them.
//!
//! The model is quantised MiniLM-L6: 384 dimensions, a few tens of megabytes, fetched once into
//! the user's cache. It is enough to separate the queries lexical search cannot. Measured on the
//! query that motivated this — "how do I make this UI less aggressive" — it scores the `quieter`
//! description at 0.435 and an MCP-authoring description at -0.005.

use anyhow::Context;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

/// What a skill's vector is computed from.
///
/// Name and description only, deliberately. Those are the routing plane, and a vector built from
/// the body would describe a document the router is not allowed to read.
#[must_use]
pub fn routing_text(name: &str, description: &str) -> String {
    format!("{name}. {description}")
}

pub struct Embedder {
    model: TextEmbedding,
}

impl std::fmt::Debug for Embedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embedder").finish_non_exhaustive()
    }
}

impl Embedder {
    /// Loads the model, fetching it into the local cache on first use.
    ///
    /// # Errors
    /// Fails if the model cannot be fetched or initialised.
    pub fn new() -> anyhow::Result<Self> {
        let model = TextEmbedding::try_new(
            TextInitOptions::new(EmbeddingModel::AllMiniLML6V2Q).with_show_download_progress(false),
        )
        .context("loading the embedding model")?;
        Ok(Self { model })
    }

    /// Embeds a batch, preserving order.
    ///
    /// # Errors
    /// Fails if the model rejects the input.
    pub fn embed(&mut self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.model.embed(texts, None).context("embedding text")
    }

    /// Embeds one string.
    ///
    /// # Errors
    /// Fails if the model rejects the input.
    pub fn embed_one(&mut self, text: &str) -> anyhow::Result<Vec<f32>> {
        let mut v = self.embed(std::slice::from_ref(&text.to_string()))?;
        v.pop().context("model returned no vector")
    }
}
