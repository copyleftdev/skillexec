//! Heading → (kind, tier, role). The vocabulary is the one measured across 8,776 `SKILL.md`
//! files; anything unrecognised becomes plain `Prose`, which is the safe default because role
//! never drives dispatch.

use skill_format::{Kind, Tier};

pub const ROLE_INTENT: u16 = 1;
pub const ROLE_PROSE: u16 = 2;
pub const ROLE_STEP: u16 = 3;
pub const ROLE_ANTIPATTERN: u16 = 4;
pub const ROLE_PITFALL: u16 = 5;
pub const ROLE_LIMITATION: u16 = 6;
pub const ROLE_EXEMPLAR: u16 = 7;
pub const ROLE_CHEATSHEET: u16 = 8;
pub const ROLE_CITATION: u16 = 9;
pub const ROLE_PREREQ: u16 = 10;
pub const ROLE_QUALITY: u16 = 11;
pub const ROLE_OUTPUT: u16 = 12;
pub const ROLE_APPLICABILITY: u16 = 13;
pub const ROLE_TOOLS: u16 = 14;
pub const ROLE_FRONTMATTER: u16 = 15;
pub const ROLE_CONTINUATION: u16 = 16;
pub const ROLE_FENCE: u16 = 100;
pub const ROLE_FENCE_UNCLOSED: u16 = 101;

#[must_use]
pub fn heading(title: &str) -> (Kind, Tier, u16) {
    let t: String = title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
        .collect();
    let t = t.trim();

    if t.starts_with("step ") || t.starts_with("phase ") {
        return (Kind::Prose, Tier::Body, ROLE_STEP);
    }
    let has = |needles: &[&str]| needles.iter().any(|n| t.contains(n));

    match () {
        () if has(&[
            "when to use",
            "when not to use",
            "applicability",
            "triggers",
        ]) =>
        {
            (Kind::Applicability, Tier::Routing, ROLE_APPLICABILITY)
        }
        () if has(&[
            "prerequisite",
            "required input",
            "requirements",
            "setup",
            "installation",
        ]) =>
        {
            (Kind::Contract, Tier::Body, ROLE_PREREQ)
        }
        () if has(&["quality check", "validation", "verify", "acceptance"]) => {
            (Kind::Contract, Tier::Body, ROLE_QUALITY)
        }
        () if has(&["output format", "what this skill produces", "deliverable"]) => {
            (Kind::Contract, Tier::Body, ROLE_OUTPUT)
        }
        () if has(&["antipattern", "anti pattern"]) => (Kind::Prose, Tier::Body, ROLE_ANTIPATTERN),
        () if has(&["pitfall", "gotcha", "common mistake"]) => {
            (Kind::Prose, Tier::Body, ROLE_PITFALL)
        }
        () if has(&["limitation", "constraint", "does not", "out of scope"]) => {
            (Kind::Prose, Tier::Body, ROLE_LIMITATION)
        }
        () if has(&["tool discovery", "tools used", "available tools", "mcp"]) => {
            (Kind::Binding, Tier::Body, ROLE_TOOLS)
        }
        () if has(&[
            "quick reference",
            "cheat sheet",
            "cheatsheet",
            "reference table",
        ]) =>
        {
            (Kind::Prose, Tier::OnDemand, ROLE_CHEATSHEET)
        }
        () if has(&["example", "sample", "walkthrough"]) => {
            (Kind::Prose, Tier::OnDemand, ROLE_EXEMPLAR)
        }
        () if has(&["reference", "related skill", "see also", "further reading"]) => {
            (Kind::Resource, Tier::OnDemand, ROLE_CITATION)
        }
        () if has(&["overview", "purpose", "intent", "what this", "summary"]) => {
            (Kind::Prose, Tier::Body, ROLE_INTENT)
        }
        () => (Kind::Prose, Tier::Body, ROLE_PROSE),
    }
}

/// Only languages that actually execute become segments. A `json` or `markdown` fence is data,
/// and calling it executable would put a capability question where none exists.
#[must_use]
pub fn fence_abi(lang: &str) -> Option<skill_format::Abi> {
    let l = lang.split_whitespace().next().unwrap_or("").to_lowercase();
    match l.as_str() {
        "bash" | "sh" | "shell" | "zsh" | "console" => Some(skill_format::Abi::Sh),
        "python" | "py" | "python3" => Some(skill_format::Abi::Python3),
        "javascript" | "js" | "node" | "mjs" | "typescript" | "ts" => Some(skill_format::Abi::Node),
        "wasm" | "wat" => Some(skill_format::Abi::Wasm32Wasip2),
        _ => None,
    }
}
