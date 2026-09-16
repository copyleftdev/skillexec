//! MCP server over `.skill` containers.
//!
//! The tool surface is deliberately shaped like the format's tier split: searching reads routing
//! planes and no bodies, loading is the first call that decompresses and verifies one, and
//! segment disclosure reports what a skill would execute and what it declared it needs.

pub mod library;
pub mod server;
