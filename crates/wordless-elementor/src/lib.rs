//! WORDLESS v2 Elementor content renderer (P3).
//!
//! Port of the v1 module `src/modules/elementor/elementor.ts`
//! (Milestone 11) in the v1.1.0 TypeScript repo (read-only reference).
//! WordPress pages built with Elementor store their layout in the
//! `_elementor_data` postmeta key: a JSON array of elements
//! (sections → columns → widgets). This crate reads that JSON and
//! renders it as plain Wordless HTML — no Elementor code runs, no
//! Elementor assets are loaded, and nothing here executes PHP.
//!
//! Scope, stated plainly (same as v1):
//! - This renders Elementor *content* on the public site. The
//!   Elementor drag-and-drop editor is a separate JavaScript
//!   application and is NOT included: pages are edited in Wordless's
//!   own block editor.
//! - Supported widgets ([`SUPPORTED_ELEMENTOR_WIDGETS`]): heading,
//!   text-editor, image, button, spacer, divider, video, icon-list,
//!   image-box, google-maps. Unknown widgets render nothing; their
//!   types are collected in
//!   [`ElementorRenderResult::unsupported`] and listed so the gap is
//!   visible, never silent.
//! - Icon glyphs are not rendered (Elementor icons are a Font Awesome
//!   font Wordless does not bundle); icon-list items render as a plain
//!   list with their text.
//! - Video embeds only allow YouTube, Vimeo and Dailymotion embeds and
//!   self-hosted `<video>` files. Anything else renders nothing.
//! - All text is escaped; all URLs must be http(s) or site-relative.
//!   text-editor HTML goes through the same `strip_dangerous_html`
//!   the block renderer uses (scripts, event handlers and
//!   `javascript:` links removed).
//!
//! # Public API
//!
//! - [`ELEMENTOR_META_KEY`] — the `wp_postmeta` key (`"_elementor_data"`).
//! - [`SUPPORTED_ELEMENTOR_WIDGETS`] — the ten renderable widget types.
//! - [`render_elementor_data`] — render parsed `_elementor_data`
//!   JSON (`&serde_json::Value`, expected to be an array) to an HTML
//!   string. Non-array data renders as `""`.
//! - [`render_elementor_data_detailed`] — same render, plus the list
//!   of widget types seen but not rendered
//!   ([`ElementorRenderResult`]).
//! - [`has_elementor_data`] — true when a `_elementor_data` meta
//!   value is non-empty valid JSON holding a non-empty elements array
//!   (the v1 `getElementorData` presence rule, without the DB read:
//!   callers fetch the `wp_postmeta` row themselves and pass the
//!   `meta_value` string).
//!
//! Pure logic — no `Db`, no Worker types, cargo-testable on native
//! targets. HTML escaping and the dangerous-HTML strip are reused
//! from `wordless-kernel::html` (moved there from `wordless-render`
//! in P3 so this crate does not depend on the render crate), so this
//! crate adds no second sanitizer.
//!
//! # WASM size
//!
//! This crate is hand-rolled string matching only (no `regex` crate)
//! and depends only on `wordless-kernel` plus `serde_json`. It adds
//! no new dependency to the worker graph.

use serde_json::Value;
use wordless_kernel::html::{escape_html, strip_dangerous_html};

/// The `wp_postmeta` key Elementor stores its layout under (v1 `ELEMENTOR_META_KEY`).
pub const ELEMENTOR_META_KEY: &str = "_elementor_data";

/// The widgets this renderer knows how to render (v1 `SUPPORTED_ELEMENTOR_WIDGETS`).
pub const SUPPORTED_ELEMENTOR_WIDGETS: &[&str] = &[
    "heading",
    "text-editor",
    "image",
    "button",
    "spacer",
    "divider",
    "video",
    "icon-list",
    "image-box",
    "google-maps",
];

/// P11 (Visual Builder) — the builder-native widget types, one per
/// shared admin element (`admin/src/components/elements.tsx`). They
/// are `wordless-` prefixed so they can never collide with a real
/// Elementor widget type, and they render through this same renderer
/// so the builder preview and the public page produce identical HTML.
///
/// Mapping (26 admin exports → 19 types; `button` already exists
/// above, several exports share one type):
/// * `Button` → `button` (above); `Input` → `wordless-input`;
///   `Select` → `wordless-select`; `Toggle` → `wordless-toggle`;
///   `Tabs` → `wordless-tabs`; `Modal` → `wordless-modal`;
///   `Drawer` → `wordless-drawer`; `ToastProvider` / `useToast` /
///   `Toast` → `wordless-toast`; `Table` → `wordless-table`;
///   `Badge` / `Tag` → `wordless-badge`; `Avatar` / `AvatarStack` →
///   `wordless-avatar`; `Breadcrumbs` → `wordless-breadcrumbs`;
///   `Stepper` → `wordless-stepper`; `EmptyState` →
///   `wordless-empty-state`; `Skeleton` → `wordless-skeleton`;
///   `Progress` / `ProgressRange` → `wordless-progress`;
///   `useCommandPaletteHotkey` / `CommandPalette` →
///   `wordless-search`; `Card` → `wordless-card`; `Alert` →
///   `wordless-alert`; `StatCard` → `wordless-stat`.
pub const WORDLESS_ELEMENT_WIDGETS: &[&str] = &[
    "wordless-input",
    "wordless-select",
    "wordless-toggle",
    "wordless-tabs",
    "wordless-modal",
    "wordless-drawer",
    "wordless-toast",
    "wordless-table",
    "wordless-badge",
    "wordless-avatar",
    "wordless-breadcrumbs",
    "wordless-stepper",
    "wordless-empty-state",
    "wordless-skeleton",
    "wordless-progress",
    "wordless-search",
    "wordless-card",
    "wordless-alert",
    "wordless-stat",
];

/// True when `widget_type` is a widget this renderer renders — a core
/// Elementor widget or a P11 builder-native element widget.
pub fn is_supported_widget(widget_type: &str) -> bool {
    SUPPORTED_ELEMENTOR_WIDGETS.contains(&widget_type)
        || WORDLESS_ELEMENT_WIDGETS.contains(&widget_type)
}

/// Result of [`render_elementor_data_detailed`]: the rendered HTML
/// plus the widget types seen but not rendered (unknown or
/// unrenderable), each listed once, in first-seen order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementorRenderResult {
    pub html: String,
    /// Widget types seen but not rendered (unknown or unrenderable).
    pub unsupported: Vec<String>,
}

static NULL_VALUE: Value = Value::Null;

/// A settings object, or the JSON null when the value is not an object
/// (v1 `asSettings`: non-objects and arrays become `{}`).
fn as_settings(value: Option<&Value>) -> &Value {
    match value {
        Some(v) if v.is_object() => v,
        _ => &NULL_VALUE,
    }
}

fn setting<'a>(settings: &'a Value, key: &str) -> Option<&'a Value> {
    settings.as_object().and_then(|m| m.get(key))
}

fn as_str(value: Option<&Value>) -> &str {
    value.and_then(|v| v.as_str()).unwrap_or("")
}

/// v1 `asNumber`: a finite JSON number, or a finite number parsed from
/// a non-empty string, else `fallback`.
fn as_number(value: Option<&Value>, fallback: f64) -> f64 {
    match value {
        Some(Value::Number(n)) => n.as_f64().filter(|f| f.is_finite()).unwrap_or(fallback),
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                return fallback;
            }
            t.parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .unwrap_or(fallback)
        }
        _ => fallback,
    }
}

/// Only http(s) and site-relative URLs are ever emitted (v1 `safeUrl`).
/// Returns the trimmed URL, or `""` when it is not emittable.
fn safe_url(value: Option<&Value>) -> String {
    let url = match value.and_then(|v| v.as_str()) {
        Some(s) => s.trim(),
        None => return String::new(),
    };
    if url.is_empty() {
        return String::new();
    }
    if url.starts_with('/') {
        return url.to_string();
    }
    let Some(colon) = url.find(':') else {
        return String::new();
    };
    let scheme = &url[..colon];
    if !scheme.chars().all(|c| c.is_ascii_alphabetic())
        || !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        || url[colon + 1..].is_empty()
    {
        return String::new();
    }
    url.to_string()
}

fn align_style(settings: &Value) -> String {
    match as_str(setting(settings, "align")) {
        "center" | "right" | "left" => {
            format!("text-align:{};", as_str(setting(settings, "align")))
        }
        _ => String::new(),
    }
}

/// v1 `linkAttrs`: `(href, extra)` — `extra` holds the `target`/`rel`
/// attributes (with leading space) for external / nofollow links.
fn link_attrs(link: Option<&Value>) -> (String, String) {
    let settings = as_settings(link);
    let href = safe_url(setting(settings, "url"));
    if href.is_empty() {
        return (String::new(), String::new());
    }
    let is_external = setting(settings, "is_external")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let nofollow = setting(settings, "nofollow")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let extra = if is_external && nofollow {
        r#" target="_blank" rel="noopener nofollow""#.to_string()
    } else if is_external {
        r#" target="_blank" rel="noopener""#.to_string()
    } else if nofollow {
        r#" rel="nofollow""#.to_string()
    } else {
        String::new()
    };
    (href, extra)
}

fn image_url(settings: &Value) -> (String, String) {
    let image = as_settings(setting(settings, "image"));
    (
        safe_url(setting(image, "url")),
        as_str(setting(image, "alt")).to_string(),
    )
}

/// Minimal URL decomposition, hand-rolled (no `regex` crate, no URL
/// parser in scope): scheme, lowercased host, path, raw query.
struct UrlParts {
    host: String,
    path: String,
    query: String,
}

fn parse_url(url: &str) -> Option<UrlParts> {
    let scheme_end = url.find("://")?;
    let scheme = &url[..scheme_end];
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return None;
    }
    let rest = &url[scheme_end + 3..];
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    let hostport = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    let host = match hostport.find(':') {
        Some(i) => &hostport[..i],
        None => hostport,
    }
    .to_ascii_lowercase();
    let mut remainder = &rest[auth_end..];
    let mut path = String::from("/");
    if remainder.starts_with('/') {
        let end = remainder.find(['?', '#']).unwrap_or(remainder.len());
        path = remainder[..end].to_string();
        remainder = &remainder[end..];
    }
    let mut query = String::new();
    if let Some(q) = remainder.strip_prefix('?') {
        let end = q.find('#').unwrap_or(q.len());
        query = q[..end].to_string();
    }
    Some(UrlParts { host, path, query })
}

fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let (k, v) = match pair.find('=') {
            Some(i) => (&pair[..i], &pair[i + 1..]),
            None => (pair, ""),
        };
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

fn you_tube_id(url: &str) -> String {
    let Some(parts) = parse_url(url) else {
        return String::new();
    };
    if parts.host == "youtu.be" {
        return path_segments(&parts.path)
            .first()
            .copied()
            .unwrap_or("")
            .to_string();
    }
    if parts.host.ends_with("youtube.com") {
        if let Some(v) = query_param(&parts.query, "v") {
            if !v.is_empty() {
                return v;
            }
        }
        let segs = path_segments(&parts.path);
        if segs.first().copied() == Some("embed") || segs.first().copied() == Some("shorts") {
            return segs.get(1).copied().unwrap_or("").to_string();
        }
    }
    String::new()
}

fn vimeo_id(url: &str) -> String {
    let Some(parts) = parse_url(url) else {
        return String::new();
    };
    if !parts.host.ends_with("vimeo.com") {
        return String::new();
    }
    let last = path_segments(&parts.path).last().copied().unwrap_or("");
    if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
        last.to_string()
    } else {
        String::new()
    }
}

fn dailymotion_id(url: &str) -> String {
    let Some(parts) = parse_url(url) else {
        return String::new();
    };
    if !parts.host.ends_with("dailymotion.com") {
        return String::new();
    }
    let Some(idx) = parts.path.find("/video/") else {
        return String::new();
    };
    let rest = &parts.path[idx + 7..];
    rest.split('/').next().unwrap_or("").to_string()
}

/// `encodeURIComponent` (v1 google-maps address encoding): leaves
/// `A–Z a–z 0–9 - _ . ! ~ * ' ( )` unescaped, percent-encodes every
/// other UTF-8 byte as uppercase `%XX`.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Format a finite JSON number the way JavaScript prints it for the
/// small clamped values used here (`40` not `40.0`).
fn js_num(n: f64) -> String {
    format!("{n}")
}

/// A settings value as a list of strings (P11 element widgets:
/// table headers/rows, tabs, breadcrumbs, stepper steps). Non-array
/// values and non-string items are dropped; at most `max` items are
/// kept so a hostile layout cannot force an unbounded render.
fn as_str_list(value: Option<&Value>, max: usize) -> Vec<String> {
    let Some(arr) = value.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .take(max)
        .collect()
}

/// A settings value as a list of setting objects (P11: select options,
/// tab panels, breadcrumb items, stepper steps). Non-object items are
/// dropped; at most `max` items are kept.
fn as_obj_list<'a>(value: Option<&'a Value>, max: usize) -> Vec<&'a Value> {
    let Some(arr) = value.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter().filter(|v| v.is_object()).take(max).collect()
}

/// A tone value clamped to an allow-list (P11: badge/alert/toast).
/// Anything else falls back to `default`.
fn tone(value: Option<&Value>, allowed: &[&str], default: &str) -> String {
    let t = as_str(value);
    if allowed.contains(&t) {
        t.to_string()
    } else {
        default.to_string()
    }
}

/// Initials for an avatar: the first letter of the first two words of
/// a name ("Ada Lovelace" → "AL", "Ada" → "A"). Empty name → "?".
fn initials(name: &str) -> String {
    let mut out = String::new();
    for word in name.split_whitespace().take(2) {
        if let Some(c) = word.chars().next() {
            out.push(c.to_ascii_uppercase());
        }
    }
    if out.is_empty() {
        out.push('?');
    }
    out
}

#[allow(clippy::too_many_lines)]
fn render_widget(widget_type: &str, settings: &Value) -> String {
    match widget_type {
        "heading" => {
            let title = as_str(setting(settings, "title"));
            if title.is_empty() {
                return String::new();
            }
            let size = as_str(setting(settings, "header_size"));
            let level = if ["h1", "h2", "h3", "h4", "h5", "h6"].contains(&size) {
                size
            } else {
                "h2"
            };
            let style = align_style(settings);
            if style.is_empty() {
                format!("<{level}>{}</{level}>", escape_html(title))
            } else {
                format!(
                    "<{level} style=\"{style}\">{}</{level}>",
                    escape_html(title)
                )
            }
        }
        "text-editor" => {
            let editor = as_str(setting(settings, "editor"));
            if editor.trim().is_empty() {
                return String::new();
            }
            strip_dangerous_html(editor)
        }
        "image" => {
            let (url, alt) = image_url(settings);
            if url.is_empty() {
                return String::new();
            }
            let caption = as_str(setting(settings, "caption"));
            let (href, extra) = link_attrs(setting(settings, "link"));
            let img = format!(
                "<img src=\"{}\" alt=\"{}\" loading=\"lazy\">",
                escape_html(&url),
                escape_html(&alt)
            );
            let inner = if href.is_empty() {
                img
            } else {
                format!("<a href=\"{}\"{}>{}</a>", escape_html(&href), extra, img)
            };
            let figcaption = if caption.is_empty() {
                String::new()
            } else {
                format!("<figcaption>{}</figcaption>", escape_html(caption))
            };
            format!("<figure class=\"elementor-widget-image\">{inner}{figcaption}</figure>")
        }
        "button" => {
            let text = as_str(setting(settings, "text"));
            if text.is_empty() {
                return String::new();
            }
            let (href, extra) = link_attrs(setting(settings, "link"));
            let style = align_style(settings);
            let inner = if href.is_empty() {
                format!(
                    "<span class=\"elementor-button\">{}</span>",
                    escape_html(text)
                )
            } else {
                format!(
                    "<a class=\"elementor-button\" href=\"{}\"{}>{}</a>",
                    escape_html(&href),
                    extra,
                    escape_html(text)
                )
            };
            if style.is_empty() {
                format!("<div class=\"elementor-widget-button\">{inner}</div>")
            } else {
                format!("<div class=\"elementor-widget-button\" style=\"{style}\">{inner}</div>")
            }
        }
        "spacer" => {
            let space = as_settings(setting(settings, "space"));
            let size = as_number(setting(space, "size"), 50.0);
            format!(
                "<div class=\"elementor-spacer\" style=\"height:{}px\" aria-hidden=\"true\"></div>",
                js_num(size.clamp(0.0, 2000.0))
            )
        }
        "divider" => "<hr class=\"elementor-divider\">".to_string(),
        "video" => {
            let vtype = as_str(setting(settings, "video_type"));
            let vtype = if vtype.is_empty() { "youtube" } else { vtype };
            match vtype {
                "youtube" => {
                    let id = you_tube_id(as_str(setting(settings, "youtube_url")));
                    if id.is_empty() {
                        return String::new();
                    }
                    format!(
                        "<div class=\"elementor-video\"><iframe src=\"https://www.youtube.com/embed/{}\" title=\"YouTube video\" frameborder=\"0\" allowfullscreen loading=\"lazy\"></iframe></div>",
                        escape_html(&id)
                    )
                }
                "vimeo" => {
                    let id = vimeo_id(as_str(setting(settings, "vimeo_url")));
                    if id.is_empty() {
                        return String::new();
                    }
                    format!(
                        "<div class=\"elementor-video\"><iframe src=\"https://player.vimeo.com/video/{}\" title=\"Vimeo video\" frameborder=\"0\" allowfullscreen loading=\"lazy\"></iframe></div>",
                        escape_html(&id)
                    )
                }
                "dailymotion" => {
                    let id = dailymotion_id(as_str(setting(settings, "dailymotion_url")));
                    if id.is_empty() {
                        return String::new();
                    }
                    format!(
                        "<div class=\"elementor-video\"><iframe src=\"https://www.dailymotion.com/embed/video/{}\" title=\"Dailymotion video\" frameborder=\"0\" allowfullscreen loading=\"lazy\"></iframe></div>",
                        escape_html(&id)
                    )
                }
                "hosted" => {
                    let hosted = as_settings(setting(settings, "hosted_url"));
                    let hosted_url = setting(hosted, "url");
                    let url = match hosted_url {
                        Some(v) if !v.is_null() => safe_url(Some(v)),
                        _ => safe_url(setting(settings, "external_url")),
                    };
                    if url.is_empty() {
                        return String::new();
                    }
                    format!(
                        "<div class=\"elementor-video\"><video controls preload=\"none\" src=\"{}\"></video></div>",
                        escape_html(&url)
                    )
                }
                _ => String::new(),
            }
        }
        "icon-list" => {
            let items = setting(settings, "icon_list")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let texts: Vec<&str> = items
                .iter()
                .map(|item| as_str(setting(as_settings(Some(item)), "text")))
                .filter(|t| !t.is_empty())
                .collect();
            if texts.is_empty() {
                return String::new();
            }
            let lis: String = texts
                .iter()
                .map(|t| format!("<li>{}</li>", escape_html(t)))
                .collect();
            format!("<ul class=\"elementor-icon-list\">{lis}</ul>")
        }
        "image-box" => {
            let (url, alt) = image_url(settings);
            let title = as_str(setting(settings, "title_text"));
            let description = as_str(setting(settings, "description_text"));
            if url.is_empty() && title.is_empty() && description.is_empty() {
                return String::new();
            }
            let (href, extra) = link_attrs(setting(settings, "link"));
            let mut inner = String::new();
            if !url.is_empty() {
                inner.push_str(&format!(
                    "<img src=\"{}\" alt=\"{}\" loading=\"lazy\">",
                    escape_html(&url),
                    escape_html(&alt)
                ));
            }
            if !title.is_empty() {
                inner.push_str(&format!("<h3>{}</h3>", escape_html(title)));
            }
            if !description.is_empty() {
                inner.push_str(&format!("<p>{}</p>", escape_html(description)));
            }
            let boxed = format!("<div class=\"elementor-image-box\">{inner}</div>");
            if href.is_empty() {
                boxed
            } else {
                format!("<a href=\"{}\"{}>{}</a>", escape_html(&href), extra, boxed)
            }
        }
        "google-maps" => {
            let address = as_str(setting(settings, "address"));
            if address.is_empty() {
                return String::new();
            }
            let zoom = as_number(
                setting(as_settings(setting(settings, "zoom")), "size"),
                10.0,
            );
            let height = as_number(
                setting(as_settings(setting(settings, "height")), "size"),
                400.0,
            );
            let zoom_clamped = (zoom.round() as i64).clamp(1, 21);
            let height_clamped = js_num(height.clamp(100.0, 1200.0));
            let src = format!(
                "https://maps.google.com/maps?q={}&z={}&output=embed",
                encode_uri_component(address),
                zoom_clamped
            );
            format!(
                "<div class=\"elementor-google-maps\"><iframe src=\"{}\" style=\"height:{}px;width:100%;border:0\" loading=\"lazy\" title=\"Map: {}\"></iframe></div>",
                escape_html(&src),
                height_clamped,
                escape_html(address)
            )
        }
        // ------------------------------------------------ P11 builder
        // elements (wordless-* types, one per shared admin element).
        // Every value is escaped; nothing here executes script, opens
        // a form action, or links anywhere the author did not set.
        "wordless-input" => {
            let label = as_str(setting(settings, "label"));
            let placeholder = as_str(setting(settings, "placeholder"));
            let input_type = match as_str(setting(settings, "inputType")) {
                "email" | "tel" | "number" => as_str(setting(settings, "inputType")),
                _ => "text",
            };
            let required = setting(settings, "required")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if label.is_empty() && placeholder.is_empty() {
                return String::new();
            }
            let req = if required { " required" } else { "" };
            let lab = if label.is_empty() {
                String::new()
            } else {
                format!("<label>{}</label>", escape_html(label))
            };
            format!(
                "<div class=\"wl-field\">{lab}<input type=\"{input_type}\" placeholder=\"{}\"{}></div>",
                escape_html(placeholder),
                req
            )
        }
        "wordless-select" => {
            let label = as_str(setting(settings, "label"));
            let options = as_obj_list(setting(settings, "options"), 50);
            if label.is_empty() && options.is_empty() {
                return String::new();
            }
            let opts: String = options
                .iter()
                .map(|o| {
                    let ol = as_str(setting(as_settings(Some(o)), "label"));
                    let ov = as_str(setting(as_settings(Some(o)), "value"));
                    let value = if ov.is_empty() { ol } else { ov };
                    format!(
                        "<option value=\"{}\">{}</option>",
                        escape_html(value),
                        escape_html(if ol.is_empty() { value } else { ol })
                    )
                })
                .collect();
            let lab = if label.is_empty() {
                String::new()
            } else {
                format!("<label>{}</label>", escape_html(label))
            };
            format!("<div class=\"wl-field\">{lab}<select>{opts}</select></div>")
        }
        "wordless-toggle" => {
            let label = as_str(setting(settings, "label"));
            if label.is_empty() {
                return String::new();
            }
            let checked = setting(settings, "checked")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let chk = if checked { " checked" } else { "" };
            format!(
                "<div class=\"wl-field\"><label><input type=\"checkbox\"{chk}> {}</label></div>",
                escape_html(label)
            )
        }
        "wordless-tabs" => {
            let tabs = as_obj_list(setting(settings, "tabs"), 12);
            let panels: Vec<(String, String)> = tabs
                .iter()
                .map(|t| {
                    let s = as_settings(Some(t));
                    (
                        as_str(setting(s, "title")).to_string(),
                        as_str(setting(s, "content")).to_string(),
                    )
                })
                .filter(|(t, _)| !t.is_empty())
                .collect();
            if panels.is_empty() {
                return String::new();
            }
            let heads: String = panels
                .iter()
                .enumerate()
                .map(|(i, (t, _))| {
                    let active = if i == 0 { " aria-selected=\"true\"" } else { "" };
                    format!("<button role=\"tab\"{active}>{}</button>", escape_html(t))
                })
                .collect();
            let first = panels
                .first()
                .map(|(_, c)| escape_html(c))
                .unwrap_or_default();
            format!(
                "<div class=\"wl-tabs\" role=\"tablist\">{heads}</div><div class=\"wl-tab-panel\">{first}</div>"
            )
        }
        "wordless-modal" => {
            let button_text = as_str(setting(settings, "buttonText"));
            let title = as_str(setting(settings, "title"));
            let content = as_str(setting(settings, "content"));
            if button_text.is_empty() && title.is_empty() && content.is_empty() {
                return String::new();
            }
            format!(
                "<div class=\"wl-modal\"><button type=\"button\">{}</button><div class=\"wl-modal-body\" role=\"dialog\"><h3>{}</h3><p>{}</p></div></div>",
                escape_html(if button_text.is_empty() { "Open" } else { button_text }),
                escape_html(title),
                escape_html(content)
            )
        }
        "wordless-drawer" => {
            let button_text = as_str(setting(settings, "buttonText"));
            let title = as_str(setting(settings, "title"));
            let content = as_str(setting(settings, "content"));
            if button_text.is_empty() && title.is_empty() && content.is_empty() {
                return String::new();
            }
            format!(
                "<div class=\"wl-drawer\"><button type=\"button\">{}</button><aside><h3>{}</h3><p>{}</p></aside></div>",
                escape_html(if button_text.is_empty() { "Open panel" } else { button_text }),
                escape_html(title),
                escape_html(content)
            )
        }
        "wordless-toast" => {
            let message = as_str(setting(settings, "message"));
            if message.is_empty() {
                return String::new();
            }
            let tone = tone(
                setting(settings, "tone"),
                &["info", "ok", "warn", "err"],
                "info",
            );
            format!(
                "<div class=\"wl-toast wl-toast--{tone}\" role=\"status\">{}</div>",
                escape_html(message)
            )
        }
        "wordless-table" => {
            let headers = as_str_list(setting(settings, "headers"), 20);
            // Rows are arrays of strings, not objects: read the array
            // directly (as_obj_list would drop every row).
            let rows: Vec<&Value> = setting(settings, "rows")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().take(50).collect())
                .unwrap_or_default();
            if headers.is_empty() && rows.is_empty() {
                return String::new();
            }
            let head = if headers.is_empty() {
                String::new()
            } else {
                let ths: String = headers
                    .iter()
                    .map(|h| format!("<th>{}</th>", escape_html(h)))
                    .collect();
                format!("<thead><tr>{ths}</tr></thead>")
            };
            let body: String = rows
                .iter()
                .map(|r| {
                    let cells = as_str_list(Some(r), 20);
                    let tds: String = cells
                        .iter()
                        .map(|c| format!("<td>{}</td>", escape_html(c)))
                        .collect();
                    format!("<tr>{tds}</tr>")
                })
                .collect();
            format!("<table class=\"wl-table\">{head}<tbody>{body}</tbody></table>")
        }
        "wordless-badge" => {
            let text = as_str(setting(settings, "text"));
            if text.is_empty() {
                return String::new();
            }
            let tone = tone(
                setting(settings, "tone"),
                &["teal", "amber", "red", "blue", "grey"],
                "teal",
            );
            format!(
                "<span class=\"wl-badge wl-badge--{tone}\">{}</span>",
                escape_html(text)
            )
        }
        "wordless-avatar" => {
            let name = as_str(setting(settings, "name"));
            let subtitle = as_str(setting(settings, "subtitle"));
            if name.is_empty() {
                return String::new();
            }
            let sub = if subtitle.is_empty() {
                String::new()
            } else {
                format!("<span>{}</span>", escape_html(subtitle))
            };
            format!(
                "<div class=\"wl-avatar\"><span class=\"wl-avatar-circle\" aria-hidden=\"true\">{}</span><strong>{}</strong>{sub}</div>",
                escape_html(&initials(name)),
                escape_html(name)
            )
        }
        "wordless-breadcrumbs" => {
            let items = as_obj_list(setting(settings, "items"), 12);
            let crumbs: Vec<(String, String)> = items
                .iter()
                .map(|i| {
                    let s = as_settings(Some(i));
                    (
                        as_str(setting(s, "label")).to_string(),
                        safe_url(setting(s, "href")),
                    )
                })
                .filter(|(l, _)| !l.is_empty())
                .collect();
            if crumbs.is_empty() {
                return String::new();
            }
            let lis: String = crumbs
                .iter()
                .enumerate()
                .map(|(i, (label, href))| {
                    let last = i + 1 == crumbs.len();
                    if last || href.is_empty() {
                        format!(
                            "<li aria-current=\"page\">{}</li>",
                            escape_html(label)
                        )
                    } else {
                        format!(
                            "<li><a href=\"{}\">{}</a></li>",
                            escape_html(href),
                            escape_html(label)
                        )
                    }
                })
                .collect();
            format!("<nav class=\"wl-breadcrumbs\" aria-label=\"Breadcrumb\"><ol>{lis}</ol></nav>")
        }
        "wordless-stepper" => {
            let steps = as_obj_list(setting(settings, "steps"), 12);
            let items: Vec<(String, String)> = steps
                .iter()
                .map(|s| {
                    let st = as_settings(Some(s));
                    (
                        as_str(setting(st, "label")).to_string(),
                        match as_str(setting(st, "state")) {
                            "done" | "current" => as_str(setting(st, "state")).to_string(),
                            _ => "todo".to_string(),
                        },
                    )
                })
                .filter(|(l, _)| !l.is_empty())
                .collect();
            if items.is_empty() {
                return String::new();
            }
            let lis: String = items
                .iter()
                .map(|(label, state)| {
                    format!(
                        "<li class=\"wl-step wl-step--{state}\">{}</li>",
                        escape_html(label)
                    )
                })
                .collect();
            format!("<ol class=\"wl-stepper\">{lis}</ol>")
        }
        "wordless-empty-state" => {
            let title = as_str(setting(settings, "title"));
            let text = as_str(setting(settings, "text"));
            let button_text = as_str(setting(settings, "buttonText"));
            let (href, extra) = link_attrs(setting(settings, "buttonLink"));
            if title.is_empty() && text.is_empty() {
                return String::new();
            }
            let btn = if button_text.is_empty() {
                String::new()
            } else if href.is_empty() {
                format!(
                    "<span class=\"elementor-button\">{}</span>",
                    escape_html(button_text)
                )
            } else {
                format!(
                    "<a class=\"elementor-button\" href=\"{href}\"{extra}>{}</a>",
                    escape_html(button_text)
                )
            };
            format!(
                "<div class=\"wl-empty-state\"><h3>{}</h3><p>{}</p>{btn}</div>",
                escape_html(title),
                escape_html(text)
            )
        }
        "wordless-skeleton" => {
            let lines = as_number(setting(settings, "lines"), 3.0).clamp(1.0, 8.0) as usize;
            let bars: String = (0..lines)
                .map(|_| "<div class=\"wl-skeleton-bar\" aria-hidden=\"true\"></div>".to_string())
                .collect();
            format!("<div class=\"wl-skeleton\" aria-hidden=\"true\">{bars}</div>")
        }
        "wordless-progress" => {
            let label = as_str(setting(settings, "label"));
            // No value set at all (and no label) is a blank widget:
            // render nothing. A value of 0 that the author set is a
            // real state (not started) and renders.
            if label.is_empty() && setting(settings, "value").is_none() {
                return String::new();
            }
            let value = as_number(setting(settings, "value"), 0.0).clamp(0.0, 100.0);
            let lab = if label.is_empty() {
                String::new()
            } else {
                format!("<span>{}</span>", escape_html(label))
            };
            format!(
                "<div class=\"wl-progress\">{lab}<div role=\"progressbar\" aria-valuenow=\"{}\" aria-valuemin=\"0\" aria-valuemax=\"100\"><div style=\"width:{}%\"></div></div></div>",
                js_num(value),
                js_num(value)
            )
        }
        "wordless-search" => {
            let placeholder = as_str(setting(settings, "placeholder"));
            format!(
                "<div class=\"wl-search\" role=\"search\"><input type=\"search\" placeholder=\"{}\" aria-label=\"Search\"></div>",
                escape_html(if placeholder.is_empty() {
                    "Search…"
                } else {
                    placeholder
                })
            )
        }
        "wordless-card" => {
            let title = as_str(setting(settings, "title"));
            let text = as_str(setting(settings, "text"));
            let (url, alt) = image_url(settings);
            let button_text = as_str(setting(settings, "buttonText"));
            let (href, extra) = link_attrs(setting(settings, "buttonLink"));
            if title.is_empty() && text.is_empty() && url.is_empty() {
                return String::new();
            }
            let img = if url.is_empty() {
                String::new()
            } else {
                format!(
                    "<img src=\"{}\" alt=\"{}\" loading=\"lazy\">",
                    escape_html(&url),
                    escape_html(&alt)
                )
            };
            let btn = if button_text.is_empty() {
                String::new()
            } else if href.is_empty() {
                format!(
                    "<span class=\"elementor-button\">{}</span>",
                    escape_html(button_text)
                )
            } else {
                format!(
                    "<a class=\"elementor-button\" href=\"{href}\"{extra}>{}</a>",
                    escape_html(button_text)
                )
            };
            format!(
                "<article class=\"wl-card\">{img}<h3>{}</h3><p>{}</p>{btn}</article>",
                escape_html(title),
                escape_html(text)
            )
        }
        "wordless-alert" => {
            let title = as_str(setting(settings, "title"));
            let text = as_str(setting(settings, "text"));
            if title.is_empty() && text.is_empty() {
                return String::new();
            }
            let tone = tone(
                setting(settings, "tone"),
                &["ok", "warn", "err", "info"],
                "info",
            );
            format!(
                "<div class=\"wl-alert wl-alert--{tone}\" role=\"alert\"><strong>{}</strong><span>{}</span></div>",
                escape_html(title),
                escape_html(text)
            )
        }
        "wordless-stat" => {
            let value = as_str(setting(settings, "value"));
            let label = as_str(setting(settings, "label"));
            if value.is_empty() && label.is_empty() {
                return String::new();
            }
            format!(
                "<div class=\"wl-stat\"><strong>{}</strong><span>{}</span></div>",
                escape_html(value),
                escape_html(label)
            )
        }
        _ => String::new(),
    }
}

fn render_element(el: &Value, unsupported: &mut Vec<String>, depth: usize) -> String {
    if depth > 10 {
        return String::new();
    }
    if !el.is_object() {
        return String::new();
    }
    let settings = as_settings(el.get("settings"));
    let children: Vec<&Value> = el
        .get("elements")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    // Children are rendered for every element type, exactly as in v1
    // (a widget's children are rendered and then discarded, but an
    // unknown widget nested inside one is still recorded).
    let child_html: String = children
        .iter()
        .map(|c| render_element(c, unsupported, depth + 1))
        .collect();
    match as_str(el.get("elType")) {
        "section" => format!("<section class=\"elementor-section\">{child_html}</section>"),
        "column" => format!("<div class=\"elementor-column\">{child_html}</div>"),
        "widget" => {
            let widget_type = as_str(el.get("widgetType"));
            if is_supported_widget(widget_type) {
                render_widget(widget_type, settings)
            } else {
                if !widget_type.is_empty() && !unsupported.iter().any(|u| u == widget_type) {
                    unsupported.push(widget_type.to_string());
                }
                String::new()
            }
        }
        _ => child_html,
    }
}

/// Render parsed `_elementor_data` (the elements array) to HTML
/// (v1 `renderElementorData`, HTML half). Non-array data renders as
/// `""`. Unknown widgets render nothing; use
/// [`render_elementor_data_detailed`] to also learn which widget types
/// were skipped.
pub fn render_elementor_data(data: &Value) -> String {
    render_elementor_data_detailed(data).html
}

/// Render parsed `_elementor_data` to HTML and report the widget types
/// seen but not rendered (v1 `renderElementorData` in full).
pub fn render_elementor_data_detailed(data: &Value) -> ElementorRenderResult {
    let mut unsupported: Vec<String> = Vec::new();
    let Some(arr) = data.as_array() else {
        return ElementorRenderResult {
            html: String::new(),
            unsupported,
        };
    };
    let html: String = arr
        .iter()
        .map(|el| render_element(el, &mut unsupported, 0))
        .collect();
    ElementorRenderResult { html, unsupported }
}

/// True when a post carries Elementor data: the `_elementor_data`
/// meta value is non-empty valid JSON holding a non-empty elements
/// array (the v1 `getElementorData` presence rule, without the DB
/// read — pass the `wp_postmeta.meta_value` string). `None`, blank,
/// invalid JSON, a non-array, or `[]` all return `false`.
pub fn has_elementor_data(meta_value: Option<&str>) -> bool {
    let Some(raw) = meta_value else {
        return false;
    };
    if raw.trim().is_empty() {
        return false;
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(arr)) => !arr.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!([
            {
                "id": "sec1",
                "elType": "section",
                "settings": { "layout": "boxed" },
                "elements": [
                    {
                        "id": "col1",
                        "elType": "column",
                        "settings": { "_column_size": 100 },
                        "elements": [
                            { "id": "w1", "elType": "widget", "widgetType": "heading", "settings": { "title": "Welcome to Wordless", "header_size": "h1", "align": "center" } },
                            { "id": "w2", "elType": "widget", "widgetType": "text-editor", "settings": { "editor": "<p>Built with <strong>Elementor</strong>, rendered by Wordless.</p>" } },
                            { "id": "w3", "elType": "widget", "widgetType": "image", "settings": { "image": { "url": "https://example.test/hero.jpg", "alt": "Hero" }, "caption": "Our hero" } },
                            { "id": "w4", "elType": "widget", "widgetType": "button", "settings": { "text": "Get started", "link": { "url": "https://example.test/start", "is_external": true }, "align": "center" } },
                            { "id": "w5", "elType": "widget", "widgetType": "spacer", "settings": { "space": { "size": 40 } } },
                            { "id": "w6", "elType": "widget", "widgetType": "divider", "settings": {} }
                        ]
                    }
                ]
            },
            {
                "id": "sec2",
                "elType": "section",
                "settings": {},
                "elements": [
                    {
                        "id": "col2",
                        "elType": "column",
                        "settings": {},
                        "elements": [
                            { "id": "w7", "elType": "widget", "widgetType": "fancy-slider", "settings": { "slides": [] } },
                            { "id": "w8", "elType": "widget", "widgetType": "video", "settings": { "video_type": "youtube", "youtube_url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ" } },
                            { "id": "w9", "elType": "widget", "widgetType": "icon-list", "settings": { "icon_list": [{ "text": "Fast" }, { "text": "Simple" }] } },
                            { "id": "w10", "elType": "widget", "widgetType": "image-box", "settings": { "image": { "url": "https://example.test/box.jpg" }, "title_text": "Box title", "description_text": "Box text" } },
                            { "id": "w11", "elType": "widget", "widgetType": "google-maps", "settings": { "address": "Lagos, Nigeria", "zoom": { "size": 12 }, "height": { "size": 300 } } }
                        ]
                    }
                ]
            }
        ])
    }

    #[test]
    fn renders_nested_section_column_heading_text_image_button() {
        let result = render_elementor_data_detailed(&fixture());
        assert!(result
            .html
            .contains("<section class=\"elementor-section\">"));
        assert!(result.html.contains("<div class=\"elementor-column\">"));
        assert!(result
            .html
            .contains("<h1 style=\"text-align:center;\">Welcome to Wordless</h1>"));
        assert!(result
            .html
            .contains("<p>Built with <strong>Elementor</strong>, rendered by Wordless.</p>"));
        assert!(result
            .html
            .contains("<img src=\"https://example.test/hero.jpg\" alt=\"Hero\" loading=\"lazy\">"));
        assert!(result.html.contains("<figcaption>Our hero</figcaption>"));
        assert!(result.html.contains(
            "<a class=\"elementor-button\" href=\"https://example.test/start\" target=\"_blank\" rel=\"noopener\">Get started</a>"
        ));
        assert!(result.html.contains("height:40px"));
        assert!(result.html.contains("<hr class=\"elementor-divider\">"));
        assert_eq!(result.unsupported, vec!["fancy-slider".to_string()]);
        // The plain String API renders the same HTML.
        assert_eq!(render_elementor_data(&fixture()), result.html);
    }

    #[test]
    fn renders_video_icon_list_image_box_and_google_maps() {
        let result = render_elementor_data_detailed(&fixture());
        assert!(result
            .html
            .contains("https://www.youtube.com/embed/dQw4w9WgXcQ"));
        assert!(result
            .html
            .contains("<ul class=\"elementor-icon-list\"><li>Fast</li><li>Simple</li></ul>"));
        assert!(result.html.contains("Box title"));
        assert!(result.html.contains("Box text"));
        assert!(result
            .html
            .contains("https://maps.google.com/maps?q=Lagos%2C%20Nigeria"));
    }

    #[test]
    fn renders_nothing_for_unknown_widgets_and_lists_them_once() {
        let data = json!([
            { "id": "x", "elType": "widget", "widgetType": "mega-menu", "settings": {} },
            { "id": "y", "elType": "widget", "widgetType": "mega-menu", "settings": {} }
        ]);
        let result = render_elementor_data_detailed(&data);
        assert_eq!(result.html, "");
        assert_eq!(result.unsupported, vec!["mega-menu".to_string()]);
        assert_eq!(render_elementor_data(&data), "");
    }

    #[test]
    fn returns_empty_html_for_non_array_data() {
        assert_eq!(render_elementor_data(&Value::Null), "");
        assert_eq!(render_elementor_data(&json!({})), "");
        assert_eq!(render_elementor_data(&json!([])), "");
        assert_eq!(
            render_elementor_data_detailed(&Value::Null),
            ElementorRenderResult {
                html: String::new(),
                unsupported: vec![],
            }
        );
    }

    #[test]
    fn escapes_widget_text_and_rejects_javascript_links() {
        let data = json!([
            { "id": "a", "elType": "widget", "widgetType": "heading", "settings": { "title": "<script>alert(1)</script> Hi" } },
            { "id": "b", "elType": "widget", "widgetType": "button", "settings": { "text": "Click", "link": { "url": "javascript:alert(1)" } } }
        ]);
        let html = render_elementor_data(&data);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("javascript:"));
        assert!(html.contains("<span class=\"elementor-button\">Click</span>"));
    }

    #[test]
    fn strips_scripts_from_text_editor_html() {
        let data = json!([
            { "id": "a", "elType": "widget", "widgetType": "text-editor", "settings": { "editor": "<p onclick=\"evil()\">Safe</p><script>bad()</script>" } }
        ]);
        let html = render_elementor_data(&data);
        assert!(html.contains("<p>Safe</p>"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("onclick"));
    }

    #[test]
    fn supports_the_documented_widget_list() {
        let mut got = SUPPORTED_ELEMENTOR_WIDGETS.to_vec();
        got.sort_unstable();
        let mut expected = vec![
            "button",
            "divider",
            "google-maps",
            "heading",
            "icon-list",
            "image",
            "image-box",
            "spacer",
            "text-editor",
            "video",
        ];
        expected.sort_unstable();
        assert_eq!(got, expected);
    }

    // P11 — the builder-native element widgets: one test per type
    // (render + escape + empty), plus the combined-list invariants.
    #[test]
    fn wordless_element_widgets_render() {
        let cases: Vec<(&str, Value, &str)> = vec![
            (
                "wordless-input",
                json!({ "label": "Name", "placeholder": "Your name", "required": true }),
                "<label>Name</label>",
            ),
            (
                "wordless-select",
                json!({ "label": "Pick", "options": [{ "label": "One", "value": "1" }] }),
                "<option value=\"1\">One</option>",
            ),
            (
                "wordless-toggle",
                json!({ "label": "Subscribe", "checked": true }),
                "type=\"checkbox\" checked",
            ),
            (
                "wordless-tabs",
                json!({ "tabs": [{ "title": "A", "content": "First" }, { "title": "B", "content": "Second" }] }),
                "role=\"tab\"",
            ),
            (
                "wordless-modal",
                json!({ "buttonText": "Open", "title": "Hello", "content": "Body text" }),
                "role=\"dialog\"",
            ),
            (
                "wordless-drawer",
                json!({ "buttonText": "Panel", "title": "Side", "content": "Side text" }),
                "<aside>",
            ),
            (
                "wordless-toast",
                json!({ "message": "Saved", "tone": "ok" }),
                "wl-toast--ok",
            ),
            (
                "wordless-table",
                json!({ "headers": ["A", "B"], "rows": [["1", "2"]] }),
                "<th>A</th>",
            ),
            (
                "wordless-badge",
                json!({ "text": "New", "tone": "amber" }),
                "wl-badge--amber",
            ),
            (
                "wordless-avatar",
                json!({ "name": "Ada Lovelace", "subtitle": "Author" }),
                "AL",
            ),
            (
                "wordless-breadcrumbs",
                json!({ "items": [{ "label": "Home", "href": "/" }, { "label": "Page" }] }),
                "aria-current=\"page\"",
            ),
            (
                "wordless-stepper",
                json!({ "steps": [{ "label": "One", "state": "done" }, { "label": "Two", "state": "current" }] }),
                "wl-step--current",
            ),
            (
                "wordless-empty-state",
                json!({ "title": "Nothing here", "text": "Try again", "buttonText": "Retry" }),
                "wl-empty-state",
            ),
            ("wordless-skeleton", json!({ "lines": 2 }), "wl-skeleton-bar"),
            (
                "wordless-progress",
                json!({ "label": "Done", "value": 75 }),
                "aria-valuenow=\"75\"",
            ),
            (
                "wordless-search",
                json!({ "placeholder": "Find…" }),
                "role=\"search\"",
            ),
            (
                "wordless-card",
                json!({ "title": "Card", "text": "Body", "buttonText": "Go" }),
                "wl-card",
            ),
            (
                "wordless-alert",
                json!({ "title": "Heads up", "text": "Read this", "tone": "warn" }),
                "role=\"alert\"",
            ),
            (
                "wordless-stat",
                json!({ "value": "1,204", "label": "Readers" }),
                "wl-stat",
            ),
        ];
        assert_eq!(cases.len(), WORDLESS_ELEMENT_WIDGETS.len());
        for (widget_type, settings, needle) in cases {
            let html = render_widget(widget_type, &settings);
            assert!(
                html.contains(needle),
                "{widget_type} should render {needle:?}, got {html:?}"
            );
            assert!(
                is_supported_widget(widget_type),
                "{widget_type} must be in the supported list"
            );
        }
    }

    #[test]
    fn wordless_element_widgets_escape_and_empty() {
        // Script in a setting never reaches the page unescaped.
        let html = render_widget(
            "wordless-badge",
            &json!({ "text": "<script>alert(1)</script>" }),
        );
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        // A javascript: link is never emitted.
        let html = render_widget(
            "wordless-breadcrumbs",
            &json!({ "items": [{ "label": "Evil", "href": "javascript:alert(1)" }] }),
        );
        assert!(!html.contains("javascript:"));
        // Empty settings render nothing — except the two types whose
        // whole point is a default placeholder: `wordless-search`
        // (default placeholder) and `wordless-skeleton` (default 3
        // bars). Both are deliberate placements, not blank widgets.
        for widget_type in WORDLESS_ELEMENT_WIDGETS {
            let html = render_widget(widget_type, &json!({}));
            match *widget_type {
                "wordless-search" => {
                    assert!(html.contains("role=\"search\""), "search renders by default");
                }
                "wordless-skeleton" => {
                    assert_eq!(
                        html.matches("wl-skeleton-bar").count(),
                        3,
                        "skeleton renders 3 bars by default"
                    );
                }
                _ => {
                    assert!(
                        html.is_empty(),
                        "{widget_type} with empty settings should render nothing, got {html:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn combined_widget_lists_have_no_overlap_or_duplicates() {
        let mut all: Vec<&str> = SUPPORTED_ELEMENTOR_WIDGETS
            .iter()
            .chain(WORDLESS_ELEMENT_WIDGETS.iter())
            .copied()
            .collect();
        assert_eq!(all.len(), 29);
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 29);
    }

    #[test]
    fn wordless_unknown_widget_is_unsupported_not_rendered() {
        let data = json!([
            { "id": "a", "elType": "widget", "widgetType": "wordless-nope", "settings": {} },
            { "id": "b", "elType": "widget", "widgetType": "wordless-badge", "settings": { "text": "Hi" } },
        ]);
        let result = render_elementor_data_detailed(&data);
        assert!(result.html.contains("wl-badge"));
        assert_eq!(result.unsupported, vec!["wordless-nope".to_string()]);
    }

    #[test]
    fn renders_hosted_video_and_ignores_unknown_video_types() {
        let data = json!([
            { "id": "a", "elType": "widget", "widgetType": "video", "settings": { "video_type": "hosted", "hosted_url": { "url": "https://example.test/clip.mp4" } } },
            { "id": "b", "elType": "widget", "widgetType": "video", "settings": { "video_type": "twitch", "youtube_url": "https://www.youtube.com/watch?v=x" } }
        ]);
        let html = render_elementor_data(&data);
        assert!(html
            .contains("<video controls preload=\"none\" src=\"https://example.test/clip.mp4\">"));
        assert!(!html.contains("youtube.com/embed"));
    }

    #[test]
    fn has_elementor_data_presence_rule() {
        assert!(!has_elementor_data(None));
        assert!(!has_elementor_data(Some("")));
        assert!(!has_elementor_data(Some("   ")));
        assert!(!has_elementor_data(Some("not json at all")));
        assert!(!has_elementor_data(Some("{}")));
        assert!(!has_elementor_data(Some("[]")));
        assert!(!has_elementor_data(Some("null")));
        assert!(has_elementor_data(Some("[{\"elType\":\"section\"}]")));
        assert!(has_elementor_data(Some(
            "  [{\"elType\":\"section\",\"elements\":[]}]  "
        )));
    }

    #[test]
    fn video_id_extraction_variants() {
        // youtu.be short link, /embed/ and /shorts/ paths, Vimeo, Dailymotion.
        let data = json!([
            { "id": "a", "elType": "widget", "widgetType": "video", "settings": { "video_type": "youtube", "youtube_url": "https://youtu.be/dQw4w9WgXcQ" } },
            { "id": "b", "elType": "widget", "widgetType": "video", "settings": { "video_type": "youtube", "youtube_url": "https://www.youtube.com/embed/abc123" } },
            { "id": "c", "elType": "widget", "widgetType": "video", "settings": { "video_type": "youtube", "youtube_url": "https://www.youtube.com/shorts/xyz789" } },
            { "id": "d", "elType": "widget", "widgetType": "video", "settings": { "video_type": "vimeo", "vimeo_url": "https://vimeo.com/123456789" } },
            { "id": "e", "elType": "widget", "widgetType": "video", "settings": { "video_type": "dailymotion", "dailymotion_url": "https://www.dailymotion.com/video/x8abcde" } },
            { "id": "f", "elType": "widget", "widgetType": "video", "settings": { "video_type": "vimeo", "vimeo_url": "https://vimeo.com/channels/staffpicks/not-a-number" } }
        ]);
        let html = render_elementor_data(&data);
        assert!(html.contains("https://www.youtube.com/embed/dQw4w9WgXcQ"));
        assert!(html.contains("https://www.youtube.com/embed/abc123"));
        assert!(html.contains("https://www.youtube.com/embed/xyz789"));
        assert!(html.contains("https://player.vimeo.com/video/123456789"));
        assert!(html.contains("https://www.dailymotion.com/embed/video/x8abcde"));
        assert!(!html.contains("not-a-number"));
    }

    #[test]
    fn safe_url_rules_and_spacer_and_maps_clamps() {
        // Site-relative URL kept; javascript:/data: rejected; spacer and
        // maps values clamp to the v1 ranges (spacer 0–2000, zoom 1–21,
        // maps height 100–1200).
        let data = json!([
            { "id": "a", "elType": "widget", "widgetType": "image", "settings": { "image": { "url": "/media/hero.jpg", "alt": "Local" } } },
            { "id": "b", "elType": "widget", "widgetType": "image", "settings": { "image": { "url": "data:image/png;base64,AAAA" } } },
            { "id": "c", "elType": "widget", "widgetType": "spacer", "settings": { "space": { "size": -5 } } },
            { "id": "d", "elType": "widget", "widgetType": "spacer", "settings": { "space": { "size": 99999 } } },
            { "id": "e", "elType": "widget", "widgetType": "google-maps", "settings": { "address": "Abuja", "zoom": { "size": 99 }, "height": { "size": 5 } } }
        ]);
        let html = render_elementor_data(&data);
        assert!(html.contains("src=\"/media/hero.jpg\""));
        assert!(!html.contains("data:image"));
        assert!(html.contains("height:0px"));
        assert!(html.contains("height:2000px"));
        assert!(html.contains("z=21"));
        assert!(html.contains("output=embed"));
        assert!(html.contains("height:100px"));
    }

    #[test]
    fn depth_limit_stops_deeply_nested_elements() {
        // 12 nested sections: the widget at depth 11 never renders.
        let mut el = json!({ "id": "leaf", "elType": "widget", "widgetType": "heading", "settings": { "title": "Too deep" } });
        for i in 0..12 {
            el = json!({ "id": format!("s{i}"), "elType": "section", "settings": {}, "elements": [el] });
        }
        let html = render_elementor_data(&json!([el]));
        assert!(!html.contains("Too deep"));
    }

    #[test]
    fn button_and_image_link_variants() {
        // nofollow-only link, and an image link without external flags.
        let data = json!([
            { "id": "a", "elType": "widget", "widgetType": "button", "settings": { "text": "Docs", "link": { "url": "https://example.test/docs", "nofollow": true } } },
            { "id": "b", "elType": "widget", "widgetType": "image", "settings": { "image": { "url": "https://example.test/p.png", "alt": "" }, "link": { "url": "https://example.test/p", "is_external": true, "nofollow": true } } }
        ]);
        let html = render_elementor_data(&data);
        assert!(html.contains("rel=\"nofollow\">Docs</a>"));
        assert!(!html.contains("target=\"_blank\" rel=\"nofollow\">Docs"));
        assert!(html.contains("target=\"_blank\" rel=\"noopener nofollow\""));
    }
}
