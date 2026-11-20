//! Block-pattern rendering helpers (P2) — port of v1
//! `src/modules/themes/patterns.ts` + `src/modules/themes/pattern-php.ts`.
//!
//! A pattern is a reusable block fragment stored as a file under
//! `patterns/<name>.php` (or `.html`). The leading PHP header comment is
//! stripped and the block HTML is rendered. **No PHP is ever executed**:
//! inline PHP snippets are evaluated by the allowlisted literal evaluator
//! below (the same 12 i18n/escaping calls v1 allows, string literals
//! only); anything outside the allowlist makes the whole pattern refuse
//! (the renderer warns and renders nothing for that pattern).
//!
//! Limits (honest, same as v1):
//! - Only file-based patterns a theme ships are resolved
//!   (`patterns/<name>.php`, falling back to `.html`). Patterns registered
//!   by plugins or by `theme.json`'s `patterns` list without a file are
//!   not rendered.
//! - Pattern nesting is bounded (depth 5, shared with template parts)
//!   and a pattern that references itself renders once and warns — that
//!   bookkeeping lives in the template renderer (`wordless-render`),
//!   which calls `find_pattern_file` / `extract_pattern_html` /
//!   `evaluate_pattern_php` per pattern block.

use wordless_kernel::error::{bad_request, WordlessError};

/// HTML-escape a value (port of v1 `escapeHtml` in `render/theme.ts`).
pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// `twentytwentyfive/template-query-loop` → `template-query-loop.php`.
pub fn pattern_file_name(slug: &str) -> Result<String, WordlessError> {
    let base = slug.rsplit('/').next().unwrap_or("");
    let clean: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if clean.is_empty() {
        return Err(bad_request(&format!(
            "Pattern slug \"{slug}\" has no usable name."
        )));
    }
    Ok(format!("{clean}.php"))
}

/// Extract the block HTML from a pattern file's source: strip the leading
/// PHP header block and return the rest, which may still contain inline
/// PHP snippets — those are evaluated by `evaluate_pattern_php`
/// (allowlisted i18n/escaping calls only). Returns `None` when the
/// remaining content is empty.
pub fn extract_pattern_html(source: &str) -> Option<String> {
    let mut rest = source;
    // Leading PHP header: /^\s*<\?php[\s\S]*?\?>\s*/
    let trimmed_start = source.trim_start_matches(char::is_whitespace);
    if let Some(after_open) = trimmed_start.strip_prefix("<?php") {
        if let Some(close) = after_open.find("?>") {
            rest = after_open[close + 2..].trim_start_matches(char::is_whitespace);
        }
    }
    let html = rest.trim();
    if html.is_empty() {
        None
    } else {
        Some(html.to_string())
    }
}

/// Find a pattern file in a theme's files map (relative path → content).
/// Tries `patterns/<name>.php` then `patterns/<name>.html`.
/// `files` is a slice of (relative path, content) pairs.
pub fn find_pattern_file(
    files: &[(String, String)],
    slug: &str,
) -> Result<Option<(String, String)>, WordlessError> {
    let file_name = pattern_file_name(slug)?;
    let base = file_name.strip_suffix(".php").unwrap_or(&file_name);
    for ext in [".php", ".html"] {
        let path = format!("patterns/{base}{ext}");
        if let Some((_, source)) = files.iter().find(|(p, _)| p == &path) {
            return Ok(Some((path, source.clone())));
        }
    }
    Ok(None)
}

// ------------------------------------------------------------------ kses

/// Tags `wp_kses_post` keeps, with their allowlisted attributes.
const KSES_ALLOWED_TAGS: &[(&str, &[&str])] = &[
    ("a", &["href", "title", "rel", "target", "class"]),
    ("br", &[]),
    ("p", &["class"]),
    ("span", &["class"]),
    ("strong", &["class"]),
    ("em", &["class"]),
    ("b", &["class"]),
    ("i", &["class"]),
    ("code", &["class"]),
    ("ul", &["class"]),
    ("ol", &["class"]),
    ("li", &["class"]),
    ("h1", &["class"]),
    ("h2", &["class"]),
    ("h3", &["class"]),
    ("h4", &["class"]),
    ("h5", &["class"]),
    ("h6", &["class"]),
    ("small", &["class"]),
    ("sub", &["class"]),
    ("sup", &["class"]),
    ("blockquote", &["class"]),
    ("cite", &["class"]),
    ("mark", &["class"]),
    ("time", &["class", "datetime"]),
];

fn kses_allowed(name: &str) -> Option<&'static [&'static str]> {
    KSES_ALLOWED_TAGS
        .iter()
        .find(|(tag, _)| *tag == name)
        .map(|(_, attrs)| *attrs)
}

/// Minimal wp_kses_post: keep allowlisted tags with allowlisted
/// attributes, drop everything else (script/style/iframe content is
/// dropped whole; other unknown tags are unwrapped, keeping their text).
pub fn kses_post(html: &str) -> String {
    let drop_content = ["script", "style", "iframe", "object", "embed", "form"];
    let mut out = String::new();
    let mut i = 0;
    while i < html.len() {
        let Some(lt_rel) = html[i..].find('<') else {
            out.push_str(&escape_html(&html[i..]));
            break;
        };
        let lt = i + lt_rel;
        out.push_str(&escape_html(&html[i..lt]));
        let Some(gt_rel) = html[lt..].find('>') else {
            out.push_str(&escape_html(&html[lt..]));
            break;
        };
        let gt = lt + gt_rel;
        let raw_tag = html[lt + 1..gt].trim();
        let is_close = raw_tag.starts_with('/');
        // /^\/?([a-zA-Z][a-zA-Z0-9]*)/
        let name_start = if is_close { 1 } else { 0 };
        let name_chars: String = raw_tag[name_start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let name_valid = raw_tag[name_start..]
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false);
        let name = if name_valid {
            name_chars.to_lowercase()
        } else {
            String::new()
        };
        if name.is_empty() || name == "!--" {
            // Comment or malformed tag: drop it.
            i = gt + 1;
            continue;
        }
        if drop_content.contains(&name.as_str()) {
            // Drop the element and everything up to its close tag.
            let rest = &html[gt + 1..];
            let close = find_close_tag(rest, &name);
            i = match close {
                Some(end) => gt + 1 + end,
                None => html.len(),
            };
            continue;
        }
        let Some(allowed) = kses_allowed(&name) else {
            // Unknown tag: unwrap (drop the tag, keep the text).
            i = gt + 1;
            continue;
        };
        if is_close {
            out.push_str(&format!("</{name}>"));
            i = gt + 1;
            continue;
        }
        // Keep the tag with allowlisted attributes only.
        let mut attrs: Vec<String> = Vec::new();
        for (attr_name, value) in parse_tag_attrs(raw_tag) {
            let attr_lower = attr_name.to_lowercase();
            if !allowed.contains(&attr_lower.as_str()) {
                continue;
            }
            if attr_lower == "href" && value.trim_start().to_lowercase().starts_with("javascript:")
            {
                continue;
            }
            attrs.push(format!("{attr_lower}=\"{}\"", escape_html(&value)));
        }
        let self_close = raw_tag.ends_with('/') || name == "br";
        out.push_str(&format!(
            "<{name}{}{}>",
            if attrs.is_empty() {
                String::new()
            } else {
                format!(" {}", attrs.join(" "))
            },
            if self_close { " /" } else { "" }
        ));
        i = gt + 1;
    }
    out
}

/// Find `</name\s*>` (case-insensitive) in `html`; return the index just
/// past the match, or `None`.
fn find_close_tag(html: &str, name: &str) -> Option<usize> {
    let lower = html.to_lowercase();
    let needle = format!("</{name}");
    let mut start = 0;
    while let Some(pos) = lower[start..].find(&needle) {
        let idx = start + pos + needle.len();
        let mut j = idx;
        while j < lower.len()
            && (lower.as_bytes()[j] == b' '
                || lower.as_bytes()[j] == b'\t'
                || lower.as_bytes()[j] == b'\n'
                || lower.as_bytes()[j] == b'\r')
        {
            j += 1;
        }
        if j < lower.len() && lower.as_bytes()[j] == b'>' {
            return Some(j + 1);
        }
        start = idx;
    }
    None
}

/// Parse `name="value"` / `name='value'` attributes from a raw tag
/// interior (v1's `/([a-zA-Z][a-zA-Z0-9-]*)\s*=\s*("[^"]*"|'[^']*')/g`).
fn parse_tag_attrs(raw_tag: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let chars: Vec<char> = raw_tag.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !(chars[i].is_ascii_alphabetic()) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '-') {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        while i < chars.len()
            && (chars[i] == ' ' || chars[i] == '\t' || chars[i] == '\n' || chars[i] == '\r')
        {
            i += 1;
        }
        if i >= chars.len() || chars[i] != '=' {
            continue;
        }
        i += 1;
        while i < chars.len()
            && (chars[i] == ' ' || chars[i] == '\t' || chars[i] == '\n' || chars[i] == '\r')
        {
            i += 1;
        }
        if i >= chars.len() || (chars[i] != '"' && chars[i] != '\'') {
            continue;
        }
        let quote = chars[i];
        i += 1;
        let value_start = i;
        while i < chars.len() && chars[i] != quote {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let value: String = chars[value_start..i].iter().collect();
        attrs.push((name, value));
        i += 1;
    }
    attrs
}

// ---------------------------------------------------------------- tokenizer

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Int(i64),
    Punct(char),
}

fn tokenize(src: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r' {
            i += 1;
            continue;
        }
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            // /* … */ comment.
            let mut j = i + 2;
            let mut closed = false;
            while j + 1 < chars.len() {
                if chars[j] == '*' && chars[j + 1] == '/' {
                    closed = true;
                    j += 2;
                    break;
                }
                j += 1;
            }
            if !closed {
                return None;
            }
            i = j;
            continue;
        }
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            let mut value = String::new();
            let mut j = i + 1;
            let mut closed = false;
            while j < chars.len() {
                let c = chars[j];
                if c == '\\' && j + 1 < chars.len() {
                    let next = chars[j + 1];
                    if next == ch || next == '\\' {
                        value.push(next);
                        j += 2;
                        continue;
                    }
                    value.push(c);
                    j += 1;
                    continue;
                }
                if c == ch {
                    closed = true;
                    break;
                }
                // No interpolation: a $ inside a double-quoted string refuses.
                if ch == '"' && c == '$' {
                    return None;
                }
                value.push(c);
                j += 1;
            }
            if !closed {
                return None;
            }
            tokens.push(Token::Str(value));
            i = j + 1;
            continue;
        }
        if ch.is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let num: String = chars[i..j].iter().collect();
            tokens.push(Token::Int(num.parse::<i64>().unwrap_or(0)));
            i = j;
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let ident: String = chars[i..j].iter().collect();
            tokens.push(Token::Ident(ident));
            i = j;
            continue;
        }
        if "(),;.".contains(ch) {
            tokens.push(Token::Punct(ch));
            i += 1;
            continue;
        }
        return None;
    }
    Some(tokens)
}

// ------------------------------------------------------------------- parser

struct ExprValue {
    text: String,
    /// True when the value should be output (an `_e` call, or `printf`).
    output: bool,
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    on_asset_uri: &'a mut dyn FnMut(),
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect_punct(&mut self, value: char) -> bool {
        if matches!(self.peek(), Some(Token::Punct(p)) if *p == value) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Parse a whole PHP segment (statements until the tokens run out).
    fn parse_segment(&mut self) -> Option<String> {
        let mut out = String::new();
        while self.pos < self.tokens.len() {
            out.push_str(&self.parse_statement()?);
        }
        Some(out)
    }

    fn parse_statement(&mut self) -> Option<String> {
        let mut echo_it = false;
        if matches!(self.peek(), Some(Token::Ident(id)) if id == "echo") {
            echo_it = true;
            self.pos += 1;
        }
        let value = self.parse_expr()?;
        // The trailing semicolon is required between statements, but a short
        // echo tag (`<?= expr ?>`) usually omits it — allow it at the end.
        if !self.expect_punct(';') && self.pos < self.tokens.len() {
            return None;
        }
        // A bare call to an _e function echoes; a bare __() call does not.
        if value.output || echo_it {
            Some(value.text)
        } else {
            Some(String::new())
        }
    }

    fn parse_expr(&mut self) -> Option<ExprValue> {
        let mut left = self.parse_term()?;
        while matches!(self.peek(), Some(Token::Punct('.'))) {
            self.pos += 1;
            let right = self.parse_term()?;
            left = ExprValue {
                text: format!("{}{}", left.text, right.text),
                output: left.output || right.output,
            };
        }
        Some(left)
    }

    fn parse_term(&mut self) -> Option<ExprValue> {
        let t = self.next()?;
        match t {
            Token::Str(s) => Some(ExprValue {
                text: s,
                output: false,
            }),
            Token::Int(n) => Some(ExprValue {
                text: n.to_string(),
                output: false,
            }),
            Token::Ident(name) => {
                if name == "echo" {
                    return None; // echo only starts a statement
                }
                if !self.expect_punct('(') {
                    return None;
                }
                let mut args: Vec<ExprValue> = Vec::new();
                if !matches!(self.peek(), Some(Token::Punct(')'))) {
                    loop {
                        let arg = self.parse_expr()?;
                        args.push(arg);
                        if self.expect_punct(',') {
                            continue;
                        }
                        break;
                    }
                }
                if !self.expect_punct(')') {
                    return None;
                }
                self.call_function(&name, &args)
            }
            Token::Punct(_) => None,
        }
    }

    fn call_function(&mut self, name: &str, args: &[ExprValue]) -> Option<ExprValue> {
        let first = args.first().map(|a| a.text.clone()).unwrap_or_default();
        match name {
            "__" | "_x" => Some(ExprValue {
                text: first,
                output: false,
            }),
            "_e" => Some(ExprValue {
                text: escape_html(&first),
                output: true,
            }),
            "esc_html__" | "esc_html_x" => Some(ExprValue {
                text: escape_html(&first),
                output: false,
            }),
            "esc_html_e" => Some(ExprValue {
                text: escape_html(&first),
                output: true,
            }),
            "esc_attr__" | "esc_attr_x" => Some(ExprValue {
                text: escape_html(&first),
                output: false,
            }),
            "esc_attr_e" => Some(ExprValue {
                text: escape_html(&first),
                output: true,
            }),
            "esc_url" => {
                let lower = first.trim_start().to_lowercase();
                if lower.starts_with("javascript:") || lower.starts_with("data:") {
                    return Some(ExprValue {
                        text: String::new(),
                        output: false,
                    });
                }
                Some(ExprValue {
                    text: escape_html(&first),
                    output: false,
                })
            }
            "get_template_directory_uri" => {
                (self.on_asset_uri)();
                Some(ExprValue {
                    text: String::new(),
                    output: false,
                })
            }
            "printf" => {
                let mut out = String::new();
                let mut arg_index = 1;
                let chars: Vec<char> = first.chars().collect();
                let mut i = 0;
                while i < chars.len() {
                    let c = chars[i];
                    if c == '%' && i + 1 < chars.len() {
                        let spec = chars[i + 1];
                        if spec == '%' {
                            out.push('%');
                            i += 2;
                        } else if spec == 's' || spec == 'd' {
                            let arg = args.get(arg_index)?;
                            arg_index += 1;
                            if spec == 'd' {
                                let num: f64 = arg.text.trim().parse().unwrap_or(f64::NAN);
                                let n = if num.is_nan() { 0.0 } else { num };
                                out.push_str(&format_js_number(n));
                            } else {
                                out.push_str(&arg.text);
                            }
                            i += 2;
                        } else {
                            return None;
                        }
                    } else {
                        out.push(c);
                        i += 1;
                    }
                }
                Some(ExprValue {
                    text: out,
                    output: true,
                })
            }
            "wp_kses_post" => Some(ExprValue {
                text: kses_post(&first),
                output: false,
            }),
            _ => None,
        }
    }
}

/// Format a number the way JS `String(Number(x))` does for the values
/// `printf %d` can produce (integers print without a decimal point).
fn format_js_number(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Evaluate the inline PHP segments in pattern HTML and splice the
/// results in. Returns `None` when any segment uses a construct outside
/// the allowlist — the caller refuses the whole pattern (never a partial
/// render, never an execution).
///
/// `on_asset_uri` is called at most once per evaluation, when a pattern
/// uses `get_template_directory_uri()` (which returns `""` — Wordless
/// serves theme files from R2 with no public asset URL yet).
pub fn evaluate_pattern_php(html: &str, on_asset_uri: &mut dyn FnMut()) -> Option<String> {
    let mut notified = false;
    let mut notify = || {
        if !notified {
            notified = true;
            on_asset_uri();
        }
    };
    let mut out = String::new();
    let mut i = 0;
    while i < html.len() {
        let Some(open_rel) = html[i..].find("<?") else {
            out.push_str(&html[i..]);
            break;
        };
        let open = i + open_rel;
        let rest = &html[open + 2..];
        let (is_short_echo, body_start) = if let Some(after) = rest.strip_prefix("php") {
            (false, after)
        } else if let Some(after) = rest.strip_prefix('=') {
            (true, after)
        } else {
            // Not a PHP segment (e.g. <?xml) — leave it literal.
            out.push_str(&html[i..open + 2]);
            i = open + 2;
            continue;
        };
        let Some(close_rel) = body_start.find("?>") else {
            // Unterminated: no regex match in v1 either — rest is literal.
            out.push_str(&html[i..]);
            break;
        };
        let body = &body_start[..close_rel];
        out.push_str(&html[i..open]);
        let segment = if is_short_echo {
            format!("echo {body}")
        } else {
            body.to_string()
        };
        let tokens = tokenize(&segment)?;
        let mut parser = Parser {
            tokens,
            pos: 0,
            on_asset_uri: &mut notify,
        };
        let value = parser.parse_segment()?;
        out.push_str(&value);
        // Absolute end: open + 2 (for "<?") + offset of the body start
        // within rest (3 for "<?php", 1 for "<?= ") + close_rel + 2 ("?>").
        let body_offset = if is_short_echo { 1 } else { 3 };
        i = open + 2 + body_offset + close_rel + 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn eval(html: &str) -> Option<String> {
        evaluate_pattern_php(html, &mut || {})
    }

    #[test]
    fn maps_namespaced_slug_to_file() {
        assert_eq!(
            pattern_file_name("twentytwentyfive/template-query-loop").unwrap(),
            "template-query-loop.php"
        );
        assert_eq!(pattern_file_name("plain-slug").unwrap(), "plain-slug.php");
        assert!(pattern_file_name("///").is_err());
    }

    #[test]
    fn finds_php_first_then_html() {
        let files = vec![
            (
                "patterns/a.html".to_string(),
                "<!-- wp:paragraph --><p>html</p><!-- /wp:paragraph -->".to_string(),
            ),
            (
                "patterns/b.php".to_string(),
                "<!-- wp:paragraph --><p>php</p><!-- /wp:paragraph -->".to_string(),
            ),
        ];
        assert_eq!(
            find_pattern_file(&files, "theme/a").unwrap().unwrap().0,
            "patterns/a.html"
        );
        assert_eq!(
            find_pattern_file(&files, "theme/b").unwrap().unwrap().0,
            "patterns/b.php"
        );
        assert!(find_pattern_file(&files, "theme/missing")
            .unwrap()
            .is_none());
    }

    const LOOP_PATTERN: &str = r#"<?php
/**
 * Title: Template Query Loop
 * Slug: testtheme/template-query-loop
 * Categories: query
 * Block Types: core/query
 */
?>
<!-- wp:query --><!-- wp:post-template --><!-- wp:post-title {"level":2,"isLink":true} /--><!-- wp:post-excerpt /--><!-- /wp:post-template --><!-- /wp:query -->"#;

    #[test]
    fn strips_leading_php_header_and_keeps_block_html() {
        let html = extract_pattern_html(LOOP_PATTERN).unwrap();
        assert!(html.contains("wp:query"));
        assert!(!html.contains("<?php"));
    }

    #[test]
    fn passes_pure_html_pattern_files_through() {
        assert!(
            extract_pattern_html("<!-- wp:paragraph --><p>Hi</p><!-- /wp:paragraph -->")
                .unwrap()
                .contains("wp:paragraph")
        );
    }

    #[test]
    fn keeps_inline_php_snippets_for_evaluator() {
        let source = "<?php /** Title: X */ ?>\n<!-- wp:paragraph --><p><?php esc_html_e( 'Hello', 'theme' ); ?></p><!-- /wp:paragraph -->";
        let html = extract_pattern_html(source).unwrap();
        assert!(html.contains("esc_html_e"));
        assert!(!html.contains("Title: X"));
    }

    #[test]
    fn returns_none_for_empty_pattern() {
        assert!(extract_pattern_html("<?php /** Title: Empty */ ?>\n").is_none());
    }

    #[test]
    fn echoes_esc_html_e_literals_escaped() {
        assert_eq!(
            eval("<p><?php esc_html_e( 'Blog', 'twentytwentyfive' ); ?></p>"),
            Some("<p>Blog</p>".to_string())
        );
        assert_eq!(
            eval("<p title=\"<?php esc_attr_e( 'A \"quoted\" title', 't' ); ?>\">x</p>"),
            Some("<p title=\"A &quot;quoted&quot; title\">x</p>".to_string())
        );
    }

    #[test]
    fn evaluates_i18n_calls_as_values() {
        assert_eq!(
            eval("<?php echo esc_html_x( 'Search', 'label', 't' ); ?>"),
            Some("Search".to_string())
        );
        assert_eq!(
            eval("<?php echo __( 'Hello & goodbye', 't' ); ?>"),
            Some("Hello & goodbye".to_string())
        );
        assert_eq!(
            eval("<?php echo esc_html__( 'A <b>bold</b> claim', 't' ); ?>"),
            Some("A &lt;b&gt;bold&lt;/b&gt; claim".to_string())
        );
    }

    #[test]
    fn evaluates_printf_with_percent_s_and_percent_percent() {
        assert_eq!(
            eval("<?php printf( esc_html__( 'Designed with %s.', 't' ), '<a href=\"https://wordpress.org\">WordPress</a>' ); ?>"),
            Some("Designed with <a href=\"https://wordpress.org\">WordPress</a>.".to_string())
        );
        assert_eq!(
            eval("<?php printf( '100%% sure %s', 'yes' ); ?>"),
            Some("100% sure yes".to_string())
        );
    }

    #[test]
    fn evaluates_wp_kses_post() {
        assert_eq!(
            eval("<?php echo wp_kses_post( _x( 'Got questions? <br><a href=\"#\">Ask us</a>', 't' ) ); ?>"),
            Some("Got questions? <br /><a href=\"#\">Ask us</a>".to_string())
        );
        assert_eq!(
            eval("<?php echo wp_kses_post( 'Safe <script>alert(1)</script> text' ); ?>"),
            Some("Safe  text".to_string())
        );
    }

    #[test]
    fn evaluates_template_directory_uri_as_empty_and_notifies_once() {
        let notified = Cell::new(0);
        let out = evaluate_pattern_php(
            "<img src=\"<?php echo esc_url( get_template_directory_uri() ); ?>/assets/a.webp\"><img src=\"<?php echo esc_url( get_template_directory_uri() ); ?>/assets/b.webp\">",
            &mut || notified.set(notified.get() + 1),
        );
        assert_eq!(
            out,
            Some("<img src=\"/assets/a.webp\"><img src=\"/assets/b.webp\">".to_string())
        );
        assert_eq!(notified.get(), 1);
    }

    #[test]
    fn evaluates_concatenation_and_multiple_statements() {
        assert_eq!(
            eval("<?php echo esc_url( get_template_directory_uri() . '/assets/x.webp' ); ?>"),
            Some("/assets/x.webp".to_string())
        );
        assert_eq!(
            eval("<?php esc_html_e( 'One', 't' ); ?>-<?php esc_html_e( 'Two', 't' ); ?>"),
            Some("One-Two".to_string())
        );
    }

    #[test]
    fn supports_short_echo_tags() {
        assert_eq!(
            eval("<?= esc_html__( 'Hi', 't' ) ?>"),
            Some("Hi".to_string())
        );
    }

    #[test]
    fn skips_translator_comments_inside_printf_calls() {
        assert_eq!(
            eval("<?php printf( /* translators: %s is the brand. */ esc_html__( 'Made by %s', 't' ), 'Us' ); ?>"),
            Some("Made by Us".to_string())
        );
    }

    #[test]
    fn refuses_disallowed_constructs() {
        let cases: &[&str] = &[
            "<?php echo $user_input; ?>",
            "<?php echo get_option( 'blogname' ); ?>",
            "<?php system( 'id' ); ?>",
            "<?php include 'evil.php'; ?>",
            "<?php eval( '1+1' ); ?>",
            "<?php echo $post->post_title; ?>",
            "<?php echo \"hi $name\"; ?>",
            "<?php printf( '%1$s', 'x' ); ?>",
            "<?php echo 'oops; ?>",
            "<?php if ( true ) { echo 'x'; } ?>",
        ];
        for snippet in cases {
            assert_eq!(eval(snippet), None, "should refuse: {snippet}");
        }
    }

    #[test]
    fn refuses_javascript_urls_from_esc_url() {
        assert_eq!(
            eval("<?php echo esc_url( 'javascript:alert(1)' ); ?>"),
            Some(String::new())
        );
    }

    #[test]
    fn kses_keeps_allowlisted_tags_and_drops_rest() {
        assert_eq!(
            kses_post("<p class=\"x\">A <strong>bold</strong> <a href=\"https://example.com\" onclick=\"evil()\">link</a></p>"),
            "<p class=\"x\">A <strong>bold</strong> <a href=\"https://example.com\">link</a></p>"
        );
        assert_eq!(kses_post("<div><p>kept text</p></div>"), "<p>kept text</p>");
        assert_eq!(
            kses_post("<a href=\"javascript:alert(1)\">x</a>"),
            "<a>x</a>"
        );
    }
}
