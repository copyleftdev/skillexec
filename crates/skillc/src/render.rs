//! Renders a container back to `SKILL.md`. The round-trip obligation in `GRAPH.md` §8 is not a
//! nicety: 8,776 existing files have to compile, and a human has to be able to read what they
//! are being asked to sign.

use skill_format::{Kind, Skill};

use crate::classify::{ROLE_CONTINUATION, ROLE_FENCE, ROLE_FENCE_UNCLOSED, ROLE_FRONTMATTER};

/// # Errors
/// Propagates any failure to resolve a node's name or payload.
pub fn render(s: &Skill<'_>) -> skill_format::Result<String> {
    let mut out = String::new();
    emit(s, 0, &mut out)?;
    Ok(out)
}

fn emit(s: &Skill<'_>, idx: u32, out: &mut String) -> skill_format::Result<()> {
    let n = s.nodes[idx as usize];
    let payload = core::str::from_utf8(s.payload(idx)?).unwrap_or_default();

    if idx != 0 {
        if n.role == ROLE_FRONTMATTER {
            // The payload is the block as written, delimiters and all.
            out.push_str(payload);
            return Ok(());
        }
        if n.role == ROLE_FENCE || n.role == ROLE_FENCE_UNCLOSED || n.kind == Kind::Segment {
            let label = if n.name_idx == u32::MAX {
                ""
            } else {
                s.manifest.string(n.name_idx)?
            };
            let (open, close) = label
                .split_once('\n')
                .map_or((label, None), |(o, c)| (o, Some(c)));
            out.push_str(open);
            out.push('\n');
            out.push_str(payload);
            if n.role != ROLE_FENCE_UNCLOSED
                && let Some(c) = close
            {
                out.push_str(c);
                out.push('\n');
            }
            return Ok(());
        }
        if n.role != ROLE_CONTINUATION && n.depth > 0 {
            for _ in 0..n.depth {
                out.push('#');
            }
            out.push(' ');
            out.push_str(s.manifest.string(n.name_idx)?);
            out.push('\n');
        }
    }
    out.push_str(payload);

    for &c in s.children_of(idx) {
        emit(s, c, out)?;
    }
    Ok(())
}
