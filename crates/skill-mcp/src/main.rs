//! MCP server over `.skill` containers.
//!
//! `SKILL_LIBRARY` points at a `.skill` file or a directory of them; `SKILL_DICT` optionally
//! points at the shared zstd dictionary they were compiled with.

use std::path::PathBuf;

use anyhow::Context;
use rmcp::{ServiceExt, transport::stdio};
use skill_format::Dictionary;
use tracing_subscriber::{EnvFilter, fmt};

use skill_mcp::{library, server};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout is the MCP transport, so logs go to stderr.
    fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            EnvFilter::try_from_env("SKILL_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let path = PathBuf::from(
        std::env::var("SKILL_LIBRARY").unwrap_or_else(|_| "library.skill".to_string()),
    );
    let dict = match std::env::var("SKILL_DICT") {
        Ok(p) => Some(Dictionary::new(
            &std::fs::read(&p).with_context(|| format!("reading dictionary {p}"))?,
        )),
        Err(_) => None,
    };

    let lib = library::Library::open(&path, dict)
        .with_context(|| format!("opening skill library at {}", path.display()))?;
    let n = lib.catalogue().len();
    tracing::info!(
        containers = lib.container_count(),
        skills = n,
        path = %path.display(),
        "skill library ready"
    );

    let service = server::SkillServer::new(lib)
        .serve(stdio())
        .await
        .context("starting the MCP stdio transport")?;
    service.waiting().await?;
    Ok(())
}
