//! Gutenberg block-comment HTML rendering (P2).
//!
//! Port of v1 `src/modules/render/blocks.ts` — same parser, same
//! renderer, same safety strip. `post_content` stores block-comment
//! HTML, e.g. `<!-- wp:paragraph --><p>Hello</p><!-- /wp:paragraph -->`.
//! Unknown blocks render their inner HTML unchanged (never dropped
//! silently). Content with no block markers (classic posts) is
//! returned as stored.
//!
//! Safety: author content is trusted (only logged-in editors/authors
//! can write it), but `<script>` tags, inline event handlers and
//! `javascript:` links are stripped as defence in depth. This is a
//! best-effort strip, not a full HTML sanitizer — untrusted-HTML
//! rendering is out of scope for M2 (same caveat as v1).

use serde_json::Value;

/// Re-exported from the kernel (P3): the escapers live in
/// `wordless-kernel::html` so `wordless-elementor` can use them
/// without depending on this crate.
pub use wordless_kernel::html::{escape_html, strip_dangerous_html};

/// HTML-escape a value for attribute / text interpolation.
/// Port of v1 `escapeHtml` (`src/modules/render/theme.ts`): `&` first,
/// then `<`, `>`, `"`, `'`.

/// One parsed block.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub name: String,
    pub attrs: Value,
    pub inner: String,
    pub self_closing: bool,
}

impl Block {
    pub fn new(name: &str, attrs: Value, inner: &str, self_closing: bool) -> Self {
        Self {
            name: name.to_string(),
            attrs,
            inner: inner.to_string(),
            self_closing,
        }
    }

    /// Attribute as a string, or `""` when absent / not a string.
    pub fn attr_str(&self, key: &str) -> &str {
        self.attrs.get(key).and_then(|v| v.as_str()).unwrap_or("")
    }

    /// Attribute as a number, or `None` when absent / not a number.
    pub fn attr_f64(&self, key: &str) -> Option<f64> {
        self.attrs.get(key).and_then(|v| v.as_f64())
    }
}

/// Decode the five entities the renderer emits (used by [`plain_text_excerpt`]).
pub fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Strip `<script>` elements, inline `on*` handlers and `javascript:`
/// hrefs. Port of v1 `stripDangerousHtml` (blocks.ts).

fn parse_attrs(raw: Option<&str>) -> Value {
    let Some(raw) = raw else {
        return Value::Object(Default::default());
    };
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(_)) => {
            serde_json::from_str(raw).unwrap_or(Value::Object(Default::default()))
        }
        _ => Value::Object(Default::default()),
    }
}

fn is_block_name_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '/' || c == '-'
}

/// Scan one block-comment open token starting at `start` (which points
/// at `<!--`). Returns `(name, attrs_json, self_closing, token_end)`.
/// Mirrors v1 `OPEN_RE`:
/// `<!--\s*wp:([a-z0-9]+(?:\/[a-z0-9-]+)?)\s*(\{[^>]*\})?\s*\/?-->`
fn scan_open_token(s: &str, start: usize) -> Option<(String, Option<String>, bool, usize)> {
    let bytes = s.as_bytes();
    let mut i = start + 4; // past `<!--`
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if !s[i..].starts_with("wp:") {
        return None;
    }
    i += 3;
    let name_start = i;
    while i < bytes.len() && is_block_name_char(bytes[i] as char) {
        i += 1;
    }
    let name = &s[name_start..i];
    if name.is_empty() || !name.chars().next().unwrap().is_ascii_alphanumeric() {
        return None;
    }
    if name.matches('/').count() > 1 {
        return None;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut attrs: Option<String> = None;
    if i < bytes.len() && bytes[i] == b'{' {
        // Balanced-brace scan (string-aware). v1's `\{[^>]*\}` takes the
        // last `}` before `>`; for well-formed JSON attrs both agree.
        let (json, end) = scan_balanced_json(s, i)?;
        attrs = Some(json);
        i = end;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
    }
    let mut self_closing = false;
    if i < bytes.len() && bytes[i] == b'/' {
        self_closing = true;
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
    }
    if s[i..].starts_with("-->") {
        Some((name.to_string(), attrs, self_closing, i + 3))
    } else {
        None
    }
}

/// Scan `{...}` starting at `start`, balancing braces and honouring
/// JSON string literals. Returns `(json_text, index_past_close)`.
fn scan_balanced_json(s: &str, start: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else if b == b'"' {
            in_str = true;
        } else if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some((s[start..=i].to_string(), i + 1));
            }
        } else if b == b'>' {
            return None;
        }
        i += 1;
    }
    None
}

/// Scan a close token `<!-- /wp:name -->` starting at `start`.
/// Returns `(name, token_end)`.
fn scan_close_token(s: &str, start: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    let mut i = start + 4;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if !s[i..].starts_with("/wp:") {
        return None;
    }
    i += 4;
    let name_start = i;
    while i < bytes.len() && is_block_name_char(bytes[i] as char) {
        i += 1;
    }
    let name = &s[name_start..i];
    if name.is_empty() {
        return None;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if s[i..].starts_with("-->") {
        Some((name.to_string(), i + 3))
    } else {
        None
    }
}

/// Find the next close token at or after `from`. v1's `CLOSE_RE`
/// matches any block name; the flat parser takes the first close
/// token after the open, whatever its name.
fn find_close(s: &str, from: usize) -> Option<(String, usize, usize)> {
    let mut i = from;
    while let Some(rel) = s[i..].find("<!--") {
        let at = i + rel;
        if let Some((name, end)) = scan_close_token(s, at) {
            return Some((name, at, end));
        }
        i = at + 4;
    }
    None
}

/// Split stored content into blocks (plus any unparsed leading/trailing
/// HTML). Port of v1 `parseBlocks`.
pub fn parse_blocks(content: &str) -> (Vec<Block>, bool) {
    let mut blocks: Vec<Block> = Vec::new();
    let mut last_index = 0usize;
    let mut has_blocks = false;
    let mut i = 0usize;
    while let Some(rel) = content[i..].find("<!--") {
        let at = i + rel;
        let Some((name, attrs_raw, self_closing, open_end)) = scan_open_token(content, at) else {
            i = at + 4;
            continue;
        };
        has_blocks = true;
        let before = content[last_index..at].trim();
        if !before.is_empty() {
            blocks.push(Block::new(
                "freeform",
                Value::Object(Default::default()),
                before,
                false,
            ));
        }
        if self_closing {
            blocks.push(Block::new(
                &name,
                parse_attrs(attrs_raw.as_deref()),
                "",
                true,
            ));
            last_index = open_end;
            i = open_end;
            continue;
        }
        match find_close(content, open_end) {
            None => {
                blocks.push(Block::new(
                    &name,
                    parse_attrs(attrs_raw.as_deref()),
                    &content[open_end..],
                    false,
                ));
                last_index = content.len();
                break;
            }
            Some((_close_name, close_at, close_end)) => {
                blocks.push(Block::new(
                    &name,
                    parse_attrs(attrs_raw.as_deref()),
                    &content[open_end..close_at],
                    false,
                ));
                last_index = close_end;
                i = close_end;
            }
        }
    }
    let tail = content[last_index..].trim();
    if !tail.is_empty() {
        blocks.push(Block::new(
            "freeform",
            Value::Object(Default::default()),
            tail,
            false,
        ));
    }
    (blocks, has_blocks)
}

/// Remove every `<tag ...>` / `</tag>` pair for the given tag names
/// (case-insensitive), keeping the text between them.
fn strip_tags(html: &str, tags: &[&str]) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < html.len() {
        let Some(rel) = lower[i..].find('<') else {
            out.push_str(&html[i..]);
            break;
        };
        let lt = i + rel;
        let mut j = lt + 1;
        let closing = j < html.len() && html.as_bytes()[j] == b'/';
        if closing {
            j += 1;
        }
        let name_start = j;
        while j < html.len() && html.as_bytes()[j].is_ascii_alphanumeric() {
            j += 1;
        }
        let name = &lower[name_start..j];
        let is_tag = tags.contains(&name)
            && (j >= html.len()
                || html.as_bytes()[j].is_ascii_whitespace()
                || html.as_bytes()[j] == b'>'
                || html.as_bytes()[j] == b'/');
        if is_tag {
            out.push_str(&html[i..lt]);
            match html[lt..].find('>') {
                Some(gt) => i = lt + gt + 1,
                None => {
                    i = html.len();
                }
            }
        } else {
            out.push_str(&html[i..=lt]);
            i = lt + 1;
        }
    }
    out
}

fn starts_with_tag(inner: &str, tag: &str) -> bool {
    let lower = inner.to_ascii_lowercase();
    if !lower.starts_with('<') {
        return false;
    }
    let rest = &lower[1..];
    if !rest.starts_with(tag) {
        return false;
    }
    matches!(
        rest.as_bytes().get(tag.len()),
        Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'>') | Some(b'/')
    )
}

/// Render one parsed block. Port of v1 `renderBlock`
/// (exported for the M5 template engine — see `templates`).
pub fn render_block(block: &Block) -> String {
    let inner = strip_dangerous_html(block.inner.trim());
    match block.name.as_str() {
        "freeform" => inner,
        "paragraph" => {
            if starts_with_tag(&inner, "p") {
                return inner;
            }
            if inner.is_empty() {
                String::new()
            } else {
                format!("<p>{inner}</p>")
            }
        }
        "heading" => {
            let level = match block.attr_f64("level") {
                Some(l) if (1.0..=6.0).contains(&l) => l.floor() as i64,
                _ => 2,
            };
            let text = strip_tags(&inner, &["h1", "h2", "h3", "h4", "h5", "h6"]);
            if text.is_empty() {
                String::new()
            } else {
                format!("<h{level}>{text}</h{level}>")
            }
        }
        "list" => {
            if starts_with_tag(&inner, "ul") || starts_with_tag(&inner, "ol") {
                return inner;
            }
            if inner.is_empty() {
                String::new()
            } else {
                format!("<ul>{inner}</ul>")
            }
        }
        "quote" => {
            let text = strip_tags(&inner, &["blockquote"]);
            if text.is_empty() {
                String::new()
            } else {
                format!("<blockquote>{text}</blockquote>")
            }
        }
        "image" => {
            if starts_with_tag(&inner, "figure") {
                return inner;
            }
            let url = block.attr_str("url");
            let alt = block.attr_str("alt");
            let caption = block.attr_str("caption");
            if url.is_empty() {
                return String::new();
            }
            let figcaption = if caption.is_empty() {
                String::new()
            } else {
                format!("<figcaption>{}</figcaption>", escape_html(caption))
            };
            format!(
                "<figure><img src=\"{}\" alt=\"{}\">{figcaption}</figure>",
                escape_html(url),
                escape_html(alt)
            )
        }
        "code" => {
            let text = strip_tags(&inner, &["pre", "code"]);
            if text.is_empty() {
                String::new()
            } else {
                format!("<pre><code>{}</code></pre>", escape_html(&text))
            }
        }
        "separator" => "<hr>".to_string(),
        "spacer" => String::new(),
        _ => inner,
    }
}

/// Render stored `post_content` to safe HTML. Port of v1 `renderContent`.
pub fn render_content(content: &str) -> String {
    let (blocks, has_blocks) = parse_blocks(content);
    if !has_blocks {
        return strip_dangerous_html(content);
    }
    blocks
        .iter()
        .map(render_block)
        .filter(|html| !html.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Plain-text excerpt of block content (M10 fix). Renders the blocks
/// first, then strips every tag, decodes the entities the renderer
/// emits, collapses whitespace and truncates to `max_length`
/// characters (with an ellipsis when truncated). The caller still
/// escapes the result for HTML. Port of v1 `plainTextExcerpt`
/// (default 200).
pub fn plain_text_excerpt(content: &str, max_length: usize) -> String {
    let rendered = render_content(content);
    let no_comments = strip_html_comments(&rendered);
    let no_tags = strip_all_tags(&no_comments);
    let decoded = decode_entities(&no_tags).replace("&nbsp;", " ");
    let collapsed = collapse_whitespace(&decoded);
    let text = collapsed.trim();
    if text.chars().count() <= max_length {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_length).collect();
    format!("{}…", truncated.trim_end())
}

fn strip_html_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start + 4..].find("-->") {
            Some(end) => rest = &rest[start + 4 + end + 3..],
            None => {
                out.push(' ');
                rest = "";
                break;
            }
        }
        out.push(' ');
    }
    out.push_str(rest);
    out
}

fn strip_all_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i..].find('<') {
            Some(rel) => {
                let lt = i + rel;
                out.push_str(&s[i..lt]);
                match s[lt..].find('>') {
                    Some(gt) => {
                        out.push(' ');
                        i = lt + gt + 1;
                    }
                    None => {
                        out.push_str(&s[lt..]);
                        break;
                    }
                }
            }
            None => {
                out.push_str(&s[i..]);
                break;
            }
        }
    }
    out
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(c);
            last_ws = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_core_blocks_v1_vector() {
        let content = [
            "<!-- wp:paragraph --><p>Hello <strong>world</strong></p><!-- /wp:paragraph -->",
            "<!-- wp:heading {\"level\":3} --><h3>Section</h3><!-- /wp:heading -->",
            "<!-- wp:list --><ul><li>one</li><li>two</li></ul><!-- /wp:list -->",
            "<!-- wp:quote --><blockquote>Quoted</blockquote><!-- /wp:quote -->",
            "<!-- wp:image {\"url\":\"https://example.com/a.png\",\"alt\":\"Alt text\"} /-->",
            "<!-- wp:code --><pre><code>const x = 1;</code></pre><!-- /wp:code -->",
            "<!-- wp:separator /-->",
        ]
        .join("\n");
        let html = render_content(&content);
        assert!(
            html.contains("<p>Hello <strong>world</strong></p>"),
            "{html}"
        );
        assert!(html.contains("<h3>Section</h3>"), "{html}");
        assert!(html.contains("<ul><li>one</li><li>two</li></ul>"), "{html}");
        assert!(html.contains("<blockquote>Quoted</blockquote>"), "{html}");
        assert!(
            html.contains("<img src=\"https://example.com/a.png\" alt=\"Alt text\">"),
            "{html}"
        );
        assert!(
            html.contains("<pre><code>const x = 1;</code></pre>"),
            "{html}"
        );
        assert!(html.contains("<hr>"), "{html}");
    }

    #[test]
    fn unknown_blocks_kept_and_scripts_stripped_v1_vector() {
        let content = "<!-- wp:gallery --><figure><img src=\"x.png\"></figure><!-- /wp:gallery -->"
            .to_string()
            + "<script>alert(\"xss\")</script>"
            + "<!-- wp:paragraph --><p onclick=\"evil()\">Safe</p><!-- /wp:paragraph -->";
        let html = render_content(&content);
        assert!(
            html.contains("<figure><img src=\"x.png\"></figure>"),
            "{html}"
        );
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("onclick"), "{html}");
        assert!(html.contains("<p>Safe</p>"), "{html}");
    }

    #[test]
    fn classic_content_returned_as_stored() {
        assert_eq!(render_content("<p>Classic</p>"), "<p>Classic</p>");
        assert_eq!(render_content("plain text"), "plain text");
    }

    #[test]
    fn excerpt_strips_block_comments_v1_vector() {
        let content = "<!-- wp:paragraph --><p>Five vinyl stickers for your laptop. Ships nowhere.</p><!-- /wp:paragraph -->"
            .to_string()
            + "<!-- wp:paragraph --><p>Second paragraph.</p><!-- /wp:paragraph -->";
        assert_eq!(
            plain_text_excerpt(&content, 200),
            "Five vinyl stickers for your laptop. Ships nowhere. Second paragraph."
        );
        assert_eq!(plain_text_excerpt("plain text", 200), "plain text");
        assert_eq!(
            plain_text_excerpt(&"x".repeat(250), 10),
            format!("{}…", "x".repeat(10))
        );
    }

    #[test]
    fn parse_edges_and_safety() {
        let content = "<!-- wp:paragraph --><p>Hi</p><!-- /wp:paragraph -->";
        let (blocks, has) = parse_blocks(content);
        assert!(has);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].name, "paragraph");
        assert_eq!(render_content(content), "<p>Hi</p>");
        let (blocks, _) = parse_blocks("<!-- wp:paragraph --><p>Open");
        assert_eq!(blocks[0].inner, "<p>Open");
        let b = Block::new("heading", json!({"level": 9}), "<h2>T</h2>", false);
        assert_eq!(render_block(&b), "<h2>T</h2>");
        let b = Block::new("heading", json!({"level": 0}), "T", false);
        assert_eq!(render_block(&b), "<h2>T</h2>");
        assert_eq!(render_block(&Block::new("image", json!({}), "", true)), "");
        assert_eq!(render_block(&Block::new("spacer", json!({}), "", true)), "");
        assert_eq!(
            render_block(&Block::new("list", json!({}), "<li>a</li>", false)),
            "<ul><li>a</li></ul>"
        );
        assert!(
            strip_dangerous_html("<a href=\"javascript:alert(1)\">x</a>").contains("href=\"#\"")
        );
        assert!(!strip_dangerous_html("<p ONCLICK=\"x()\">y</p>").contains("ONCLICK"));
        assert_eq!(
            escape_html("A & B <\"x\">"),
            "A &amp; B &lt;&quot;x&quot;&gt;"
        );
    }
}
