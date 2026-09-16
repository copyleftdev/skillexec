//! A deliberately small Markdown reader: skills use headings, paragraphs and fences, and a
//! full `CommonMark` parser would introduce normalisations that break the round-trip obligation.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Text(String),
    /// `open` is the opening line verbatim, indentation included: fences inside list items are
    /// indented, and a parser that only looks at column 0 walks straight into treating `# foo`
    /// inside a Python block as a heading.
    Fence {
        open: String,
        close: Option<String>,
        lang: String,
        code: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Document {
    pub frontmatter: Vec<(String, String)>,
    pub fm_raw: String,
    pub blocks: Vec<Block>,
}

impl Document {
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.frontmatter
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[must_use]
pub fn parse(src: &str) -> Document {
    let mut doc = Document::default();
    let mut rest = src;

    if let Some(after) = src.strip_prefix("---\n")
        && let Some(end) = find_fm_end(after)
    {
        // Stored verbatim, delimiters included. Reconstructing `---` lines on render meant
        // guessing, and the corpus supplied all three ways to guess wrong: a file opening with
        // an empty `---\n---\n` block, a file that is nothing but frontmatter, and one with a
        // stray `---` further down. Store, do not derive.
        let close = after[end..]
            .strip_prefix("---\n")
            .map_or(after.len(), |r| after.len() - r.len());
        doc.fm_raw = src[..4 + close].to_string();
        doc.frontmatter = parse_frontmatter(&after[..end]);
        rest = &after[close..];
    }

    let mut text = String::new();
    let mut lines = rest.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if let Some((indent, marker)) = fence_marker(trimmed) {
            flush(&mut text, &mut doc.blocks);
            let lang = trimmed[indent + marker.len()..].trim().to_string();
            let mut code = String::new();
            let mut close = None;
            for l in lines.by_ref() {
                let bare = l.trim_end_matches(['\n', '\r']);
                // The closing line carries its own indentation, which is not always the
                // opening line's. Deriving it instead of storing it cost 12 files.
                if bare.trim() == marker {
                    close = Some(bare.to_string());
                    break;
                }
                code.push_str(l);
            }
            doc.blocks.push(Block::Fence {
                open: trimmed.to_string(),
                close,
                lang,
                code,
            });
            continue;
        }
        if let Some((level, htext)) = heading(trimmed) {
            flush(&mut text, &mut doc.blocks);
            doc.blocks.push(Block::Heading { level, text: htext });
            continue;
        }
        text.push_str(line);
    }
    flush(&mut text, &mut doc.blocks);
    doc
}

fn find_fm_end(after: &str) -> Option<usize> {
    let mut at = 0usize;
    for line in after.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            return Some(at);
        }
        at += line.len();
    }
    None
}

/// Top-level `key: value` pairs, including YAML block scalars.
///
/// `description: >-` followed by indented lines is extremely common in real skills, and reading
/// the marker as the value leaves those skills with no description at all — which means no
/// routing signal, since the routing plane is exactly name and description. Twenty-five of one
/// 192-skill library were unroutable for this reason before it was noticed.
///
/// Folded (`>`) joins continuation lines with spaces; literal (`|`) keeps the newlines. The
/// chomping suffixes (`-`, `+`) only affect trailing newlines, which are trimmed here anyway.
fn parse_frontmatter(raw: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = raw.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.starts_with(char::is_whitespace) || line.starts_with('#') || line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (key, marker) = (k.trim().to_string(), v.trim());

        let folded = marker.starts_with('>');
        if !folded && !marker.starts_with('|') {
            out.push((key, marker.to_string()));
            continue;
        }

        let mut parts: Vec<String> = Vec::new();
        while i < lines.len() {
            let l = lines[i];
            if !l.is_empty() && !l.starts_with(char::is_whitespace) {
                break;
            }
            parts.push(l.trim().to_string());
            i += 1;
        }
        let joined = if folded {
            parts.join(" ")
        } else {
            parts.join("\n")
        };
        out.push((key, joined.trim().to_string()));
    }
    out
}

fn fence_marker(line: &str) -> Option<(usize, String)> {
    let indent = line.len() - line.trim_start().len();
    let body = &line[indent..];
    for ch in ['`', '~'] {
        let n = body.chars().take_while(|c| *c == ch).count();
        if n >= 3 {
            return Some((indent, std::iter::repeat_n(ch, n).collect()));
        }
    }
    None
}

fn heading(line: &str) -> Option<(u8, String)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line.chars().nth(hashes) == Some(' ') {
        // Kept verbatim, not trimmed: the extra spaces in `##  Title` are round-trip data, and
        // a commented-out `#     return x` that slips past fence detection survives unharmed.
        // Classification trims for itself.
        return Some((
            u8::try_from(hashes).unwrap_or(6),
            line[hashes + 1..].to_string(),
        ));
    }
    None
}

fn flush(text: &mut String, out: &mut Vec<Block>) {
    if !text.is_empty() {
        out.push(Block::Text(std::mem::take(text)));
    }
}
