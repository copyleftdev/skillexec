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
/// Normative guidance: "best practices", "design principles", "always"/"never". Distinct from
/// a pitfall, which names a specific failure, and from a step, which is part of a procedure.
pub const ROLE_PRINCIPLE: u16 = 17;
pub const ROLE_FENCE: u16 = 100;
pub const ROLE_FENCE_UNCLOSED: u16 = 101;

#[must_use]
pub fn role_name(role: u16) -> &'static str {
    match role {
        ROLE_INTENT => "intent",
        ROLE_PROSE => "prose",
        ROLE_STEP => "step",
        ROLE_ANTIPATTERN => "antipattern",
        ROLE_PITFALL => "pitfall",
        ROLE_LIMITATION => "limitation",
        ROLE_EXEMPLAR => "exemplar",
        ROLE_CHEATSHEET => "cheatsheet",
        ROLE_CITATION => "citation",
        ROLE_PREREQ => "prereq",
        ROLE_QUALITY => "quality",
        ROLE_OUTPUT => "output",
        ROLE_APPLICABILITY => "applicability",
        ROLE_TOOLS => "tools",
        ROLE_PRINCIPLE => "principle",
        _ => "other",
    }
}

/// One row of the empirical vocabulary. `exact` rows match the whole normalised heading, which
/// is what keeps a section literally titled "Always" from also catching "Always validate input".
struct Rule {
    needles: &'static [&'static str],
    kind: Kind,
    tier: Tier,
    role: u16,
    exact: bool,
}

const fn sub(needles: &'static [&'static str], kind: Kind, tier: Tier, role: u16) -> Rule {
    Rule {
        needles,
        kind,
        tier,
        role,
        exact: false,
    }
}

const fn whole(needles: &'static [&'static str], kind: Kind, tier: Tier, role: u16) -> Rule {
    Rule {
        needles,
        kind,
        tier,
        role,
        exact: true,
    }
}

/// First match wins, so order is meaning: "do not use this skill when" has to reach
/// `Applicability` before "do not" reaches anything else.
///
/// This is a table rather than a chain of branches because the vocabulary is measured, not
/// designed. `skillc roles` reports what it misses, and what it misses changes with the corpus.
static RULES: &[Rule] = &[
    sub(
        &[
            "when to use",
            "when not to use",
            "use this skill when",
            "use this when",
            "do not use this skill",
            "dont use this skill",
            "applicability",
            "triggers",
            "when to invoke",
        ],
        Kind::Applicability,
        Tier::Routing,
        ROLE_APPLICABILITY,
    ),
    sub(
        &[
            "prerequisite",
            "required input",
            "requirements",
            "setup",
            "installation",
        ],
        Kind::Contract,
        Tier::Body,
        ROLE_PREREQ,
    ),
    sub(
        &[
            "troubleshooting",
            "error handling",
            "common errors",
            "debugging",
        ],
        Kind::Prose,
        Tier::Body,
        ROLE_PITFALL,
    ),
    sub(
        &["quality check", "validation", "verify", "acceptance"],
        Kind::Contract,
        Tier::Body,
        ROLE_QUALITY,
    ),
    sub(
        &[
            "output format",
            "output structure",
            "what this skill produces",
            "deliverable",
        ],
        Kind::Contract,
        Tier::Body,
        ROLE_OUTPUT,
    ),
    sub(
        &["antipattern", "anti pattern"],
        Kind::Prose,
        Tier::Body,
        ROLE_ANTIPATTERN,
    ),
    sub(
        &["pitfall", "gotcha", "common mistake"],
        Kind::Prose,
        Tier::Body,
        ROLE_PITFALL,
    ),
    sub(
        &["limitation", "constraint", "does not", "out of scope"],
        Kind::Prose,
        Tier::Body,
        ROLE_LIMITATION,
    ),
    sub(
        &[
            "tool discovery",
            "tools used",
            "available tools",
            "mcp",
            "skills to invoke",
            "required tools",
        ],
        Kind::Binding,
        Tier::Body,
        ROLE_TOOLS,
    ),
    sub(
        &[
            "best practice",
            "design principle",
            "core philosophy",
            "philosophy",
            "mental model",
            "guiding principle",
            "conventions",
        ],
        Kind::Prose,
        Tier::Body,
        ROLE_PRINCIPLE,
    ),
    whole(
        &["always", "never", "prefer", "avoid", "rules"],
        Kind::Prose,
        Tier::Body,
        ROLE_PRINCIPLE,
    ),
    sub(
        &[
            "workflow",
            "process",
            "procedure",
            "instructions",
            "how it works",
            "how to use",
        ],
        Kind::Prose,
        Tier::Body,
        ROLE_STEP,
    ),
    sub(
        &["quick start", "getting started", "quickstart"],
        Kind::Prose,
        Tier::Body,
        ROLE_EXEMPLAR,
    ),
    sub(
        &[
            "quick reference",
            "cheat sheet",
            "cheatsheet",
            "reference table",
        ],
        Kind::Prose,
        Tier::OnDemand,
        ROLE_CHEATSHEET,
    ),
    sub(
        &[
            "example",
            "sample",
            "walkthrough",
            "code pattern",
            "common pattern",
        ],
        Kind::Prose,
        Tier::OnDemand,
        ROLE_EXEMPLAR,
    ),
    sub(
        &[
            "reference",
            "related skill",
            "see also",
            "further reading",
            "resources",
        ],
        Kind::Resource,
        Tier::OnDemand,
        ROLE_CITATION,
    ),
    sub(
        &["overview", "purpose", "intent", "what this", "summary"],
        Kind::Prose,
        Tier::Body,
        ROLE_INTENT,
    ),
];

fn normalize(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
        .collect::<String>()
        .trim()
        .to_string()
}

#[must_use]
pub fn heading(title: &str) -> (Kind, Tier, u16) {
    let t = normalize(title);
    if t.starts_with("step ") || t.starts_with("phase ") {
        return (Kind::Prose, Tier::Body, ROLE_STEP);
    }
    for r in RULES {
        let hit = if r.exact {
            r.needles.contains(&t.as_str())
        } else {
            r.needles.iter().any(|n| t.contains(n))
        };
        if hit {
            return (r.kind, r.tier, r.role);
        }
    }
    (Kind::Prose, Tier::Body, ROLE_PROSE)
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
