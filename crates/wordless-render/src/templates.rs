//! Block-theme template engine (P2).
//!
//! Port of v1 `src/modules/themes/templates.ts` (template hierarchy,
//! stack-based tree parser, template-block renderer) plus the
//! pattern support from `src/modules/themes/patterns.ts` and the
//! allowlisted inline-PHP evaluator from
//! `src/modules/themes/pattern-php.ts`.
//!
//! Supported template blocks (v1): template-part, site-title,
//! site-tagline, navigation, query, post-template, post-title,
//! post-content, post-excerpt, post-date, post-terms, query-title,
//! query-pagination, search, group, columns, column, pattern, and
//! every content block `blocks.rs` already knows (paragraph, heading,
//! list, quote, image, code, separator, spacer).
//! `post-featured-image` renders nothing — Wordless has no
//! featured-image assignment yet. Unknown blocks render their inner
//! HTML unchanged, same policy as M2.
//!
//! The theme's files are supplied as a `BTreeMap<path, content>`
//! (relative paths like `templates/home.html`, `parts/header.html`,
//! `patterns/foo.php`) by the coordinator, which loads them through
//! `wordless-themes` (`loadActiveTheme`). This crate does not read
//! R2 itself.
//!
//! Pattern PHP: a literal evaluator, not a PHP runtime — no variable
//! lookup, no function definition, no control flow, no file access,
//! no `eval`. Anything outside the allowlist makes the whole pattern
//! refuse (warning, renders nothing).

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blocks::{escape_html, parse_blocks, render_block, Block};

/// Template kinds, in hierarchy order (v1 `TemplateKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateKind {
    Home,
    Single,
    Page,
    Archive,
    Search,
    #[serde(rename = "404")]
    NotFound,
    Index,
}

impl TemplateKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TemplateKind::Home => "home",
            TemplateKind::Single => "single",
            TemplateKind::Page => "page",
            TemplateKind::Archive => "archive",
            TemplateKind::Search => "search",
            TemplateKind::NotFound => "404",
            TemplateKind::Index => "index",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateLink {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplatePostData {
    pub title: String,
    pub date: String,
    pub excerpt: String,
    /// Already-rendered post content HTML (via `render_content`).
    pub content_html: String,
    pub url: String,
    pub comment_count: i64,
    pub terms: Vec<TemplateTerm>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateTerm {
    pub name: String,
    pub url: String,
    pub taxonomy: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateContext {
    pub site_name: String,
    pub description: String,
    pub nav: Vec<TemplateLink>,
    pub kind: TemplateKind,
    /// Page `<title>` text (without the site name suffix).
    pub title: String,
    pub post: Option<TemplatePostData>,
    /// Post ID for the rendered post (M7: SEO lookup); `None` for lists.
    pub post_id: Option<i64>,
    pub posts: Option<Vec<TemplatePostData>>,
    pub page: i64,
    pub total_pages: i64,
    /// Base URL for pagination links (path + existing query, without `page=`).
    pub base_url: String,
    /// Search query, when kind is `search`.
    pub query: Option<String>,
    /// Archive heading, when kind is `archive`.
    pub archive_title: Option<String>,
}

impl TemplateContext {
    pub fn new(
        site_name: &str,
        description: &str,
        nav: Vec<TemplateLink>,
        kind: TemplateKind,
    ) -> Self {
        Self {
            site_name: site_name.to_string(),
            description: description.to_string(),
            nav,
            kind,
            title: String::new(),
            post: None,
            post_id: None,
            posts: None,
            page: 1,
            total_pages: 1,
            base_url: "/".to_string(),
            query: None,
            archive_title: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateBlock {
    pub name: String,
    pub attrs: Value,
    /// Raw inner HTML between the open/close comments (leaf blocks).
    pub inner_html: String,
    pub children: Vec<TemplateBlock>,
    pub self_closing: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedTemplate {
    pub html: String,
    pub warnings: Vec<String>,
}

fn template_candidates(kind: TemplateKind) -> &'static [&'static str] {
    match kind {
        TemplateKind::Home => &["home.html", "index.html"],
        TemplateKind::Single => &["single.html", "singular.html", "index.html"],
        TemplateKind::Page => &["page.html", "singular.html", "index.html"],
        TemplateKind::Archive => &["archive.html", "index.html"],
        TemplateKind::Search => &["search.html", "archive.html", "index.html"],
        TemplateKind::NotFound => &["404.html", "index.html"],
        TemplateKind::Index => &["index.html"],
    }
}

/// First template that exists for this kind, or `None` when none do.
/// Port of v1 `resolveTemplateName`.
pub fn resolve_template_name(kind: TemplateKind, available: &HashSet<String>) -> Option<String> {
    for name in template_candidates(kind) {
        if available.contains(*name) {
            return Some(name.to_string());
        }
    }
    None
}

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

fn is_name_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}

/// Scan one `<!-- wp:... -->` / `<!-- /wp:... -->` token.
/// Returns `(is_close, name, attrs_json, self_closing, token_end)`.
/// Mirrors v1 `TOKEN_RE`:
/// `<!--\s*(\/?)wp:([a-z0-9][a-z0-9-]*(?:\/[a-z0-9][a-z0-9-]*)?)\s*(\{[^>]*\})?\s*(\/?)-->`
fn scan_token(s: &str, start: usize) -> Option<(bool, String, Option<String>, bool, usize)> {
    let bytes = s.as_bytes();
    let mut i = start + 4;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut is_close = false;
    if i < bytes.len() && bytes[i] == b'/' {
        is_close = true;
        i += 1;
    }
    if !s[i..].starts_with("wp:") {
        return None;
    }
    i += 3;
    let name_start = i;
    while i < bytes.len() && is_name_char(bytes[i] as char) {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    if i < bytes.len() && bytes[i] == b'/' {
        i += 1;
        let seg_start = i;
        while i < bytes.len() && is_name_char(bytes[i] as char) {
            i += 1;
        }
        if i == seg_start {
            return None;
        }
    }
    let name = s[name_start..i].to_string();
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut attrs: Option<String> = None;
    if i < bytes.len() && bytes[i] == b'{' {
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
        Some((is_close, name, attrs, self_closing, i + 3))
    } else {
        None
    }
}

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

/// Stack-based tree parser for block templates. Handles nesting
/// (group > paragraph, query > post-template) that the flat
/// post-content parser cannot. Unmatched closes are ignored;
/// unclosed opens keep the rest. Port of v1 `parseTemplateBlocks`.
///
/// Implementation: a stack of frames; a block is attached to its
/// parent when its close token arrives (or at end-of-input for
/// unclosed blocks). Raw text between tokens goes to the innermost
/// open block's `inner_html`, or becomes a `freeform` child at the
/// root — exactly like v1.
pub fn parse_template_blocks(html: &str) -> Vec<TemplateBlock> {
    // Frames: (block being built). The bottom frame is a pseudo-root
    // whose children are the result.
    let mut frames: Vec<TemplateBlock> = vec![TemplateBlock {
        name: String::new(),
        attrs: Value::Object(Default::default()),
        inner_html: String::new(),
        children: Vec::new(),
        self_closing: false,
    }];
    let mut last_index = 0usize;
    let mut i = 0usize;
    while let Some(rel) = html[i..].find("<!--") {
        let at = i + rel;
        let Some((is_close, name, attrs_raw, self_closing, token_end)) = scan_token(html, at)
        else {
            i = at + 4;
            continue;
        };
        let before = &html[last_index..at];
        if !before.trim().is_empty() {
            if frames.len() > 1 {
                frames.last_mut().unwrap().inner_html.push_str(before);
            } else {
                frames[0].children.push(TemplateBlock {
                    name: "freeform".to_string(),
                    attrs: Value::Object(Default::default()),
                    inner_html: before.to_string(),
                    children: Vec::new(),
                    self_closing: false,
                });
            }
        }
        last_index = token_end;
        i = token_end;
        if is_close {
            // Pop until the matching open block (tolerates a missing
            // open); every popped block is attached to its parent.
            while frames.len() > 1 {
                let popped = frames.pop().unwrap();
                let matched = popped.name == name;
                frames.last_mut().unwrap().children.push(popped);
                if matched {
                    break;
                }
            }
            continue;
        }
        let block = TemplateBlock {
            name,
            attrs: parse_attrs(attrs_raw.as_deref()),
            inner_html: String::new(),
            children: Vec::new(),
            self_closing,
        };
        if self_closing {
            frames.last_mut().unwrap().children.push(block);
        } else {
            frames.push(block);
        }
    }
    let tail = &html[last_index..];
    if !tail.trim().is_empty() {
        if frames.len() > 1 {
            frames.last_mut().unwrap().inner_html.push_str(tail);
        } else {
            frames[0].children.push(TemplateBlock {
                name: "freeform".to_string(),
                attrs: Value::Object(Default::default()),
                inner_html: tail.to_string(),
                children: Vec::new(),
                self_closing: false,
            });
        }
    }
    // Attach any still-open blocks to their parents, innermost first.
    while frames.len() > 1 {
        let block = frames.pop().unwrap();
        frames.last_mut().unwrap().children.push(block);
    }
    frames.pop().unwrap().children
}

fn pagination_html(ctx: &TemplateContext) -> String {
    if ctx.total_pages <= 1 {
        return String::new();
    }
    let sep = if ctx.base_url.contains('?') { "&" } else { "?" };
    let prev = if ctx.page > 1 {
        format!(
            "<a href=\"{}\">&larr; Newer</a>",
            escape_html(&format!("{}{}page={}", ctx.base_url, sep, ctx.page - 1))
        )
    } else {
        "<span></span>".to_string()
    };
    let next = if ctx.page < ctx.total_pages {
        format!(
            "<a href=\"{}\">Older &rarr;</a>",
            escape_html(&format!("{}{}page={}", ctx.base_url, sep, ctx.page + 1))
        )
    } else {
        "<span></span>".to_string()
    };
    format!("<nav class=\"pagination\" aria-label=\"Pagination\">{prev}{next}</nav>")
}

fn resolve_preset_vars(s: &str) -> String {
    // `var:preset|spacing|60` -> `var(--wp--preset--spacing--60)`;
    // `var:preset|custom|slug` -> `var(--wp--custom--slug)`.
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(idx) = rest.find("var:preset|") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + "var:preset|".len()..];
        let mut parts = after.splitn(3, '|');
        let kind = parts.next().unwrap_or("");
        let slug = parts.next().unwrap_or("");
        let tail = parts.next().unwrap_or("");
        // slug ends at the first char that is not [a-z0-9-].
        let slug_end = slug
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            .unwrap_or(slug.len());
        let slug = &slug[..slug_end];
        if kind.is_empty() || slug.is_empty() {
            out.push_str("var:preset|");
            rest = after;
            continue;
        }
        if kind == "custom" {
            out.push_str(&format!("var(--wp--custom--{slug})"));
        } else {
            out.push_str(&format!("var(--wp--preset--{kind}--{slug})"));
        }
        // `tail` starts right after the slug; the remainder of `after`
        // after kind|slug is `tail` plus whatever followed the slug.
        let consumed = "var:preset|".len() + kind.len() + 1 + slug_end;
        let _ = tail;
        rest = &rest[idx + consumed..];
    }
    out.push_str(rest);
    out
}

fn inline_style_from_attrs(attrs: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(style) = attrs.get("style").and_then(|v| v.as_object()) {
        if let Some(spacing) = style.get("spacing").and_then(|v| v.as_object()) {
            for key in ["padding", "margin"] {
                match spacing.get(key) {
                    Some(Value::String(v)) => parts.push(format!("{key}: {v}")),
                    Some(Value::Object(obj)) => {
                        for side in ["top", "right", "bottom", "left"] {
                            if let Some(Value::String(v)) = obj.get(side) {
                                parts.push(format!("{key}-{side}: {v}"));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(ty) = style.get("typography").and_then(|v| v.as_object()) {
            if let Some(Value::String(v)) = ty.get("fontSize") {
                parts.push(format!("font-size: {v}"));
            }
            if let Some(Value::String(v)) = ty.get("lineHeight") {
                parts.push(format!("line-height: {v}"));
            }
        }
        if let Some(color) = style.get("color").and_then(|v| v.as_object()) {
            if let Some(Value::String(v)) = color.get("background") {
                parts.push(format!("background-color: {v}"));
            }
            if let Some(Value::String(v)) = color.get("text") {
                parts.push(format!("color: {v}"));
            }
        }
    }
    parts
        .iter()
        .map(|p| resolve_preset_vars(p))
        .collect::<Vec<_>>()
        .join("; ")
}

fn block_class(attrs: &Value, base: &str) -> String {
    let mut classes = vec![base.to_string()];
    let str_attr = |k: &str| attrs.get(k).and_then(|v| v.as_str()).unwrap_or("");
    if !str_attr("className").is_empty() {
        classes.push(str_attr("className").to_string());
    }
    if !str_attr("align").is_empty() {
        classes.push(format!("align{}", str_attr("align")));
    }
    if !str_attr("backgroundColor").is_empty() {
        classes.push(format!(
            "has-{}-background-color",
            str_attr("backgroundColor")
        ));
    }
    if !str_attr("textColor").is_empty() {
        classes.push(format!("has-{}-color", str_attr("textColor")));
    }
    if !str_attr("fontSize").is_empty() {
        classes.push(format!("has-{}-font-size", str_attr("fontSize")));
    }
    if !str_attr("fontFamily").is_empty() {
        classes.push(format!("has-{}-font-family", str_attr("fontFamily")));
    }
    classes.join(" ")
}

struct RenderState<'a> {
    files: &'a BTreeMap<String, String>,
    depth: usize,
    warnings: Vec<String>,
    active_patterns: HashSet<String>,
    asset_uri_warned: bool,
}

fn to_content_block(block: &TemplateBlock) -> Block {
    Block::new(
        &block.name,
        block.attrs.clone(),
        &block.inner_html,
        block.self_closing,
    )
}

fn render_blocks(
    blocks: &[TemplateBlock],
    ctx: &TemplateContext,
    state: &mut RenderState,
) -> String {
    blocks
        .iter()
        .map(|b| render_template_block(b, ctx, state))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_template_block(
    block: &TemplateBlock,
    ctx: &TemplateContext,
    state: &mut RenderState,
) -> String {
    let attrs = &block.attrs;
    let attr_str = |k: &str| attrs.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let attr_level = |default: i64| -> i64 {
        match attrs.get("level").and_then(|v| v.as_f64()) {
            Some(l) if (1.0..=6.0).contains(&l) => l.floor() as i64,
            _ => default,
        }
    };
    match block.name.as_str() {
        "freeform" => block.inner_html.clone(),
        "template-part" => {
            let slug = attr_str("slug");
            if slug.is_empty() {
                return String::new();
            }
            if state.depth >= 5 {
                state
                    .warnings
                    .push(format!("Template part \"{slug}\" skipped: nesting too deep."));
                return String::new();
            }
            let Some(part) = state.files.get(&format!("parts/{slug}.html")) else {
                state
                    .warnings
                    .push(format!("Template part \"{slug}\" not found in the theme."));
                return String::new();
            };
            let inner = parse_template_blocks(part);
            state.depth += 1;
            let out = render_blocks(&inner, ctx, state);
            state.depth -= 1;
            out
        }
        "site-title" => {
            let level = attr_level(1);
            format!(
                "<h{level} class=\"wp-block-site-title\"><a href=\"/\">{}</a></h{level}>",
                escape_html(&ctx.site_name)
            )
        }
        "site-tagline" => {
            if ctx.description.is_empty() {
                String::new()
            } else {
                format!(
                    "<p class=\"wp-block-site-tagline\">{}</p>",
                    escape_html(&ctx.description)
                )
            }
        }
        "navigation" => {
            if ctx.nav.is_empty() {
                return String::new();
            }
            let items = ctx
                .nav
                .iter()
                .map(|item| {
                    format!(
                        "<li><a href=\"{}\">{}</a></li>",
                        escape_html(&item.url),
                        escape_html(&item.title)
                    )
                })
                .collect::<Vec<_>>()
                .join("");
            format!("<nav class=\"wp-block-navigation\" aria-label=\"Primary\"><ul>{items}</ul></nav>")
        }
        "query" => {
            let posts = ctx.posts.clone().unwrap_or_default();
            let template = block.children.iter().find(|c| c.name == "post-template");
            let Some(template) = template else {
                return render_blocks(&block.children, ctx, state);
            };
            if posts.is_empty() {
                return "<p>No posts found.</p>".to_string();
            }
            posts
                .iter()
                .map(|post| {
                    let mut post_ctx = ctx.clone();
                    post_ctx.post = Some(post.clone());
                    format!(
                        "<article class=\"post-card\">{}</article>",
                        render_blocks(&template.children, &post_ctx, state)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "post-template" => render_blocks(&block.children, ctx, state),
        "post-title" => {
            let Some(post) = &ctx.post else {
                return String::new();
            };
            let level = attr_level(1);
            let text = escape_html(if post.title.is_empty() { "(untitled)" } else { &post.title });
            let inner = if attrs.get("isLink").and_then(|v| v.as_bool()) == Some(true) {
                format!("<a href=\"{}\">{text}</a>", escape_html(&post.url))
            } else {
                text
            };
            format!("<h{level} class=\"wp-block-post-title\">{inner}</h{level}>")
        }
        "post-content" => ctx.post.as_ref().map(|p| p.content_html.clone()).unwrap_or_default(),
        "post-excerpt" => match &ctx.post {
            Some(post) if !post.excerpt.is_empty() => {
                format!("<p class=\"wp-block-post-excerpt\">{}</p>", escape_html(&post.excerpt))
            }
            _ => String::new(),
        },
        "post-date" => match &ctx.post {
            Some(post) => format!(
                "<time class=\"wp-block-post-date\" datetime=\"{}\">{}</time>",
                escape_html(&post.date),
                escape_html(&post.date.chars().take(10).collect::<String>())
            ),
            None => String::new(),
        },
        "post-terms" => {
            let Some(post) = &ctx.post else {
                return String::new();
            };
            let taxonomy = if attr_str("term").is_empty() { "category" } else { attr_str("term") };
            let separator = if attrs.get("separator").is_some() {
                attr_str("separator")
            } else {
                ", "
            };
            let terms: Vec<&TemplateTerm> =
                post.terms.iter().filter(|t| t.taxonomy == taxonomy).collect();
            if terms.is_empty() {
                return String::new();
            }
            let joined = terms
                .iter()
                .map(|t| format!("<a href=\"{}\">{}</a>", escape_html(&t.url), escape_html(&t.name)))
                .collect::<Vec<_>>()
                .join(&escape_html(separator));
            format!("<div class=\"wp-block-post-terms\">{joined}</div>")
        }
        "post-featured-image" => {
            state.warnings.push(
                "post-featured-image renders nothing: featured images are not assigned yet."
                    .to_string(),
            );
            String::new()
        }
        "query-title" => {
            let ty = attr_str("type");
            let text = if ty == "search" {
                match &ctx.query {
                    Some(q) => format!("Search results for “{q}”"),
                    None => ctx.title.clone(),
                }
            } else if ty == "archive" {
                ctx.archive_title.clone().unwrap_or_else(|| ctx.title.clone())
            } else {
                ctx.title.clone()
            };
            format!("<h1 class=\"wp-block-query-title\">{}</h1>", escape_html(&text))
        }
        "query-pagination" => pagination_html(ctx),
        "search" => format!(
            "<form role=\"search\" method=\"get\" action=\"/search\" class=\"wp-block-search\"><label>Search <input type=\"search\" name=\"q\" value=\"{}\"></label> <button type=\"submit\">Search</button></form>",
            escape_html(ctx.query.as_deref().unwrap_or(""))
        ),
        "pattern" => {
            let slug = attr_str("slug");
            if slug.is_empty() {
                state.warnings.push("Pattern block without a slug renders nothing.".to_string());
                return String::new();
            }
            if state.depth >= 5 {
                state
                    .warnings
                    .push(format!("Pattern \"{slug}\" skipped: nesting too deep."));
                return String::new();
            }
            if state.active_patterns.contains(slug) {
                state
                    .warnings
                    .push(format!("Pattern \"{slug}\" skipped: it references itself (cycle)."));
                return String::new();
            }
            let Some(found) = find_pattern_file(state.files, slug) else {
                state.warnings.push(format!(
                    "Pattern \"{slug}\" not found in the theme (patterns/{}.php).",
                    slug.split('/').next_back().unwrap_or("")
                ));
                return String::new();
            };
            let Some(html) = extract_pattern_html(&found) else {
                state.warnings.push(format!("Pattern \"{slug}\" is empty."));
                return String::new();
            };
            let (evaluated, used_asset_uri) = evaluate_pattern_php(&html);
            if used_asset_uri && !state.asset_uri_warned {
                state.asset_uri_warned = true;
                state.warnings.push(
                    "Theme asset URLs (get_template_directory_uri) are not served yet — image sources built from them are empty."
                        .to_string(),
                );
            }
            let Some(evaluated) = evaluated else {
                state.warnings.push(format!(
                    "Pattern \"{slug}\" not rendered: its file contains PHP outside the allowlisted i18n/escaping calls (never executed)."
                ));
                return String::new();
            };
            state.active_patterns.insert(slug.to_string());
            state.depth += 1;
            let inner = parse_template_blocks(&evaluated);
            let out = render_blocks(&inner, ctx, state);
            state.depth -= 1;
            state.active_patterns.remove(slug);
            out
        }
        "group" | "columns" | "column" => {
            let tag = {
                let t = attr_str("tagName");
                if !t.is_empty()
                    && t.chars().next().unwrap().is_ascii_lowercase()
                    && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                {
                    t.to_string()
                } else {
                    "div".to_string()
                }
            };
            let style = inline_style_from_attrs(attrs);
            let cls = block_class(attrs, &format!("wp-block-{}", block.name));
            let literal = strip_block_comments(&block.inner_html);
            let literal = strip_leading_tag(&literal, &tag);
            let literal = strip_trailing_tag(&literal, &tag);
            let mut inner = render_blocks(&block.children, ctx, state);
            if !literal.trim().is_empty() {
                inner.push_str(&literal);
            }
            format!(
                "<{tag} class=\"{}\"{}>{inner}</{tag}>",
                escape_html(&cls),
                if style.is_empty() {
                    String::new()
                } else {
                    format!(" style=\"{}\"", escape_html(&style))
                }
            )
        }
        _ => {
            if !block.children.is_empty() {
                let inner = render_blocks(&block.children, ctx, state);
                return render_block(&Block::new(&block.name, block.attrs.clone(), &inner, false));
            }
            let (blocks, _) = parse_blocks(&block.inner_html);
            if !blocks.is_empty() && block.inner_html.contains("<!--") {
                return blocks.iter().map(render_block).collect::<Vec<_>>().join("\n");
            }
            render_block(&to_content_block(block))
        }
    }
}

/// Remove `<!-- wp:... -->` / `<!-- /wp:... -->` comments only
/// (v1's group literal strips `/<!--\s*\/?wp:[^>]*-->/g` — other
/// HTML comments are kept).
fn strip_block_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        let after = &rest[start + 4..];
        let trimmed = after.trim_start();
        let is_wp = trimmed.starts_with("wp:") || trimmed.starts_with("/wp:");
        match after.find("-->") {
            Some(end) if is_wp => {
                out.push_str(&rest[..start]);
                rest = &after[end + 3..];
            }
            Some(_) => {
                out.push_str(&rest[..start + 4]);
                rest = after;
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

fn strip_leading_tag(s: &str, tag: &str) -> String {
    let trimmed = s.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    let open = format!("<{tag}");
    if lower.starts_with(&open) {
        let after = &trimmed[open.len()..];
        let next = after.chars().next();
        if matches!(
            next,
            Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some('>') | Some('/')
        ) {
            if let Some(gt) = after.find('>') {
                return after[gt + 1..].to_string();
            }
        }
    }
    s.to_string()
}

fn strip_trailing_tag(s: &str, tag: &str) -> String {
    let trimmed_end = s.trim_end();
    let close = format!("</{tag}>");
    if trimmed_end.to_ascii_lowercase().ends_with(&close) {
        let cut = trimmed_end.len() - close.len();
        return trimmed_end[..cut].to_string();
    }
    s.to_string()
}

/// Render a full template file into page body HTML.
/// `files` maps relative paths (`templates/*.html`, `parts/*.html`)
/// to content. Port of v1 `renderTemplateFile`.
pub fn render_template_file(
    name: &str,
    files: &BTreeMap<String, String>,
    ctx: &TemplateContext,
) -> RenderedTemplate {
    let Some(source) = files.get(&format!("templates/{name}")) else {
        return RenderedTemplate {
            html: String::new(),
            warnings: vec![format!("Template \"{name}\" not found in the theme.")],
        };
    };
    let mut state = RenderState {
        files,
        depth: 0,
        warnings: Vec::new(),
        active_patterns: HashSet::new(),
        asset_uri_warned: false,
    };
    let blocks = parse_template_blocks(source);
    let html = render_blocks(&blocks, ctx, &mut state);
    RenderedTemplate {
        html,
        warnings: state.warnings,
    }
}

/// Wrap rendered template body in a full HTML document.
/// Port of v1 `themedDocument`.
pub fn themed_document(
    title: &str,
    site_name: &str,
    description: &str,
    stylesheet_href: &str,
    body: &str,
    extra_body: Option<&str>,
    head_extra: Option<&str>,
) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{} — {}</title>\n<meta name=\"description\" content=\"{}\">\n{}\n<link rel=\"stylesheet\" href=\"{}\">\n</head>\n<body>\n{}\n{}\n</body>\n</html>",
        escape_html(title),
        escape_html(site_name),
        escape_html(description),
        head_extra.unwrap_or(""),
        escape_html(stylesheet_href),
        body,
        extra_body.unwrap_or("")
    )
}

// ---------------------------------------------------------------- patterns
//
// Pattern resolution + the allowlisted inline-PHP evaluator live in
// `wordless-themes` (`patterns.rs`) — this crate uses its public
// functions and does not re-implement theme logic. The wrappers
// below only adapt shapes: the theme crate works over
// `Vec<(path, content)>` files and returns `Result`s, while the
// template renderer works over a `BTreeMap` and reports pattern
// problems as warnings (never a failed page).
//
// One deliberate adaptation: v1 `findPatternFile` throws for a slug
// with no usable name; here that becomes "not found" (warning).
// The observable page outcome is the same — v1's `finishPage`
// catches a theme error and falls back to the M2 layout, and a
// missing pattern renders nothing with a warning either way.

/// `twentytwentyfive/template-query-loop` → `template-query-loop.php`.
/// Returns `None` when the slug has no usable name.
pub fn pattern_file_name(slug: &str) -> Option<String> {
    wordless_themes::patterns::pattern_file_name(slug).ok()
}

/// Extract the block HTML from a pattern file's source: strip the
/// leading PHP header block and return the rest, which may still
/// contain inline PHP snippets (evaluated by
/// [`evaluate_pattern_php`]). Returns `None` when the remaining
/// content is empty.
pub fn extract_pattern_html(source: &str) -> Option<String> {
    wordless_themes::patterns::extract_pattern_html(source)
}

/// Find a pattern file in a theme's files map. Tries
/// `patterns/<name>.php` then `patterns/<name>.html`. Returns the
/// file's source.
pub fn find_pattern_file(files: &BTreeMap<String, String>, slug: &str) -> Option<String> {
    let flat: Vec<(String, String)> = files.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    match wordless_themes::patterns::find_pattern_file(&flat, slug) {
        Ok(Some((_path, source))) => Some(source),
        _ => None,
    }
}

/// Minimal `wp_kses_post`: keep allowlisted tags with allowlisted
/// attributes, drop everything else.
pub fn kses_post(html: &str) -> String {
    wordless_themes::patterns::kses_post(html)
}

/// Evaluate the inline PHP segments in pattern HTML and splice the
/// results in. Returns `(Some(rendered), asset_uri_used)` on success,
/// or `(None, asset_uri_used)` when any segment uses a construct
/// outside the allowlist — the caller refuses the whole pattern
/// (never a partial render, never an execution).
pub fn evaluate_pattern_php(html: &str) -> (Option<String>, bool) {
    let mut asset_uri_used = false;
    let out = wordless_themes::patterns::evaluate_pattern_php(html, &mut || {
        asset_uri_used = true;
    });
    (out, asset_uri_used)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx_home() -> TemplateContext {
        let mut ctx = TemplateContext::new(
            "Wordless",
            "A site",
            vec![TemplateLink {
                title: "Home".to_string(),
                url: "/".to_string(),
            }],
            TemplateKind::Home,
        );
        ctx.title = "Home".to_string();
        ctx.posts = Some(vec![TemplatePostData {
            title: "Hello".to_string(),
            date: "2026-11-19T00:00:00.000Z".to_string(),
            excerpt: "Intro.".to_string(),
            content_html: "<p>Body</p>".to_string(),
            url: "/hello".to_string(),
            comment_count: 0,
            terms: vec![],
        }]);
        ctx.page = 1;
        ctx.total_pages = 1;
        ctx.base_url = "/".to_string();
        ctx
    }

    #[test]
    fn hierarchy_resolution_matches_v1() {
        let avail: HashSet<String> = ["index.html".to_string(), "single.html".to_string()]
            .into_iter()
            .collect();
        assert_eq!(
            resolve_template_name(TemplateKind::Home, &avail),
            Some("index.html".to_string())
        );
        assert_eq!(
            resolve_template_name(TemplateKind::Single, &avail),
            Some("single.html".to_string())
        );
        assert_eq!(
            resolve_template_name(TemplateKind::Page, &avail),
            Some("index.html".to_string())
        );
        assert_eq!(
            resolve_template_name(TemplateKind::Search, &avail),
            Some("index.html".to_string())
        );
        assert_eq!(
            resolve_template_name(TemplateKind::NotFound, &avail),
            Some("index.html".to_string())
        );
        let empty: HashSet<String> = HashSet::new();
        assert_eq!(resolve_template_name(TemplateKind::Home, &empty), None);
        let home: HashSet<String> = ["home.html".to_string()].into_iter().collect();
        assert_eq!(
            resolve_template_name(TemplateKind::Home, &home),
            Some("home.html".to_string())
        );
    }

    #[test]
    fn renders_default_theme_home_template() {
        // v1 default theme templates/home.html (templates.ts + default-theme.ts).
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        files.insert(
            "parts/header.html".to_string(),
            "<!-- wp:group {\"tagName\":\"header\",\"className\":\"site-header\"} -->\n<header class=\"wp-block-group site-header\">\n<!-- wp:site-title /-->\n<!-- wp:site-tagline /-->\n<!-- wp:navigation /-->\n</header>\n<!-- /wp:group -->".to_string(),
        );
        files.insert(
            "parts/footer.html".to_string(),
            "<!-- wp:group {\"tagName\":\"footer\",\"className\":\"site-footer\"} -->\n<footer class=\"wp-block-group site-footer\">\n<!-- wp:paragraph --><p>Powered by <a href=\"/admin\">Wordless</a></p><!-- /wp:paragraph -->\n</footer>\n<!-- /wp:group -->".to_string(),
        );
        files.insert(
            "templates/home.html".to_string(),
            "<!-- wp:template-part {\"slug\":\"header\"} /-->\n\n<!-- wp:group {\"tagName\":\"main\"} -->\n<main class=\"wp-block-group\">\n<!-- wp:query -->\n<!-- wp:post-template -->\n<!-- wp:post-title {\"level\":2,\"isLink\":true} /-->\n<!-- wp:post-date /-->\n<!-- wp:post-excerpt /-->\n<!-- /wp:post-template -->\n<!-- wp:query-pagination /-->\n<!-- /wp:query -->\n</main>\n<!-- /wp:group -->\n\n<!-- wp:template-part {\"slug\":\"footer\"} /-->".to_string(),
        );
        let rendered = render_template_file("home.html", &files, &ctx_home());
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        assert!(
            rendered
                .html
                .contains("<h1 class=\"wp-block-site-title\"><a href=\"/\">Wordless</a></h1>"),
            "{}",
            rendered.html
        );
        assert!(
            rendered
                .html
                .contains("<h2 class=\"wp-block-post-title\"><a href=\"/hello\">Hello</a></h2>"),
            "{}",
            rendered.html
        );
        assert!(rendered.html.contains("<time class=\"wp-block-post-date\" datetime=\"2026-11-19T00:00:00.000Z\">2026-11-19</time>"), "{}", rendered.html);
        assert!(
            rendered
                .html
                .contains("<p class=\"wp-block-post-excerpt\">Intro.</p>"),
            "{}",
            rendered.html
        );
        assert!(rendered.html.contains("Powered by"), "{}", rendered.html);
    }

    #[test]
    fn template_parser_handles_nesting_and_missing_parts() {
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        files.insert(
            "templates/index.html".to_string(),
            "<!-- wp:template-part {\"slug\":\"nope\"} /--><!-- wp:group --><div><!-- wp:paragraph --><p>Hi</p><!-- /wp:paragraph --></div><!-- /wp:group --><!-- wp:query --><!-- wp:post-template --><!-- wp:post-title {\"level\":2} /--><!-- /wp:post-template --><!-- /wp:query -->".to_string(),
        );
        let rendered = render_template_file("index.html", &files, &ctx_home());
        assert!(rendered.html.contains("<p>Hi</p>"), "{}", rendered.html);
        assert!(
            rendered.warnings.iter().any(|w| w.contains("not found")),
            "{:?}",
            rendered.warnings
        );
        // Empty query renders the v1 "No posts found." line.
        let mut ctx = ctx_home();
        ctx.posts = Some(vec![]);
        let rendered = render_template_file("index.html", &files, &ctx);
        assert!(
            rendered.html.contains("No posts found."),
            "{}",
            rendered.html
        );
        // Missing template file warns.
        let rendered = render_template_file("single.html", &files, &ctx_home());
        assert!(rendered.html.is_empty());
        assert!(rendered.warnings.iter().any(|w| w.contains("not found")));
    }

    #[test]
    fn pattern_php_evaluator_matches_v1_allowlist() {
        let (out, used) = evaluate_pattern_php("<?= esc_html__( 'Hello', 'theme' ) ?>");
        assert_eq!(out.as_deref(), Some("Hello"));
        assert!(!used);
        let (out, _) = evaluate_pattern_php("<?php echo __( 'Hi' ) . ' there'; ?>");
        assert_eq!(out.as_deref(), Some("Hi there"));
        let (out, _) = evaluate_pattern_php("<?php printf( '%s has %d wolves', 'Pack', 3 ); ?>");
        assert_eq!(out.as_deref(), Some("Pack has 3 wolves"));
        let (out, used) =
            evaluate_pattern_php("<img src=\"<?= get_template_directory_uri() ?>/x.png\">");
        assert_eq!(out.as_deref(), Some("<img src=\"/x.png\">"));
        assert!(used);
        // Outside the allowlist: whole pattern refuses.
        let (out, _) = evaluate_pattern_php("<?php include 'x.php'; ?>");
        assert_eq!(out, None);
        let (out, _) = evaluate_pattern_php("<?php echo $var; ?>");
        assert_eq!(out, None);
        // wp_kses_post keeps allowlisted tags, drops scripts whole.
        assert_eq!(
            kses_post("<p class=\"a\" onclick=\"x()\">Hi</p><script>alert(1)</script><unknown>kept</unknown>"),
            "<p class=\"a\">Hi</p>kept"
        );
        assert_eq!(
            pattern_file_name("twentytwentyfive/template-query-loop"),
            Some("template-query-loop.php".to_string())
        );
        assert_eq!(pattern_file_name(""), None);
        assert_eq!(
            extract_pattern_html(
                "<?php /* header */ ?>\n<!-- wp:paragraph --><p>x</p><!-- /wp:paragraph -->"
            ),
            Some("<!-- wp:paragraph --><p>x</p><!-- /wp:paragraph -->".to_string())
        );
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        files.insert(
            "patterns/foo.php".to_string(),
            "<?php // Title: Foo ?>\n<p>Foo</p>".to_string(),
        );
        assert_eq!(
            find_pattern_file(&files, "theme/foo"),
            Some("<?php // Title: Foo ?>\n<p>Foo</p>".to_string())
        );
        // Style preset vars + block classes.
        assert_eq!(
            inline_style_from_attrs(
                &json!({"style": {"spacing": {"padding": "var:preset|spacing|60"}, "color": {"background": "#fff"}}})
            ),
            "padding: var(--wp--preset--spacing--60); background-color: #fff"
        );
        assert_eq!(
            block_class(
                &json!({"className": "x", "align": "wide"}),
                "wp-block-group"
            ),
            "wp-block-group x alignwide"
        );
        // themedDocument shape.
        let doc = themed_document(
            "Hello",
            "Wordless",
            "Desc",
            "/theme.css?v=abc",
            "<p>x</p>",
            None,
            None,
        );
        assert!(doc.contains("<title>Hello — Wordless</title>"));
        assert!(doc.contains("<link rel=\"stylesheet\" href=\"/theme.css?v=abc\">"));
    }
}
