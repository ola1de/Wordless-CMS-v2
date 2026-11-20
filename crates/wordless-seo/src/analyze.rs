//! Deterministic SEO analysis (port of v1 `src/modules/seo/analyze.ts`
//! + `types.ts`).
//!
//! Every number is computed from the site's own stored content — no
//! rankings, traffic or search-engine data is invented. Score formula
//! (documented in v1 `types.ts`): each check contributes pass = 1,
//! warn = 0.5, fail = 0; score = round(100 * sum / checks). A post
//! with no checks at all (impossible in practice — the word-count
//! check always runs) scores 0. Readability is a Flesch reading-ease
//! ESTIMATE (syllables counted by a vowel-group heuristic) and is
//! labelled as such everywhere it is shown. Headings: the post title
//! renders as the page's `<h1>`, so content `<h1>` tags are flagged,
//! not required.
//!
//! Pure over its inputs except [`analyze_post_seo`], which loads the
//! post + its SEO fields from D1.

use serde::{Deserialize, Serialize};
use wordless_cms::posts::get_post_by_id;
use wordless_db::Db;
use wordless_kernel::error::{not_found, WordlessError};

use wordless_seo_core::meta::get_post_seo;

/// One check result (v1 `SeoCheck`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeoCheck {
    pub id: String,
    pub label: String,
    pub status: CheckStatus,
    /// What was measured, in plain language (never invented).
    pub detail: String,
}

/// v1 `CheckStatus`: `pass` | `warn` | `fail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    fn score(self) -> f64 {
        match self {
            CheckStatus::Pass => 1.0,
            CheckStatus::Warn => 0.5,
            CheckStatus::Fail => 0.0,
        }
    }
}

/// Deterministic SEO analysis of one post (v1 `SeoAnalysis`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeoAnalysis {
    pub post_id: i64,
    pub score: i64,
    pub checks: Vec<SeoCheck>,
    pub word_count: i64,
    /// Flesch reading-ease estimate, 0–100 (higher = easier to read).
    pub reading_ease: Option<f64>,
    pub internal_links: i64,
    pub external_links: i64,
    pub images: i64,
    pub images_missing_alt: i64,
}

/// Input to the pure scorer (v1 `analyzeSeoContent` argument).
#[derive(Debug, Clone, Default)]
pub struct AnalyzeSeoInput {
    pub title: String,
    pub slug: String,
    pub content: String,
    pub excerpt: String,
    pub seo_title: String,
    pub seo_description: String,
    pub focus_keyword: String,
    pub canonical: String,
    pub site_url: String,
}

/// JS `String.length` counts UTF-16 code units; count the same way so
/// the 30–70 / 50–160 title/description ranges behave identically.
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Strip Gutenberg block comments + HTML tags, decode common entities.
/// (v1 `textFromContent`.)
pub fn text_from_content(content: &str) -> String {
    // `<!-- ... -->` (non-greedy, unterminated comments left as-is).
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find("<!--") {
        if let Some(end) = rest[start + 4..].find("-->") {
            out.push_str(&rest[..start]);
            out.push(' ');
            rest = &rest[start + 4 + end + 3..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    // `<...>` tags (a `<` with no later `>` is left as-is).
    let mut no_tags = String::with_capacity(out.len());
    let mut tail = out.as_str();
    while let Some(start) = tail.find('<') {
        if let Some(end) = tail[start..].find('>') {
            no_tags.push_str(&tail[..start]);
            no_tags.push(' ');
            tail = &tail[start + end + 1..];
        } else {
            break;
        }
    }
    no_tags.push_str(tail);
    // Entities, in v1's order.
    let decoded = no_tags
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    // Collapse whitespace + trim (v1 `/\s+/g` → `" "` then `.trim()`).
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn words_of(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_ascii_alphanumeric()))
        .collect()
}

/// Vowel-group syllable heuristic (estimate only).
pub fn count_syllables(word: &str) -> i64 {
    let clean: String = word
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase())
        .collect();
    if clean.is_empty() {
        return 0;
    }
    if clean.len() <= 3 {
        return 1;
    }
    let without_silent_e = clean.strip_suffix('e').unwrap_or(&clean);
    let mut groups = 0;
    let mut in_vowel = false;
    for c in without_silent_e.chars() {
        let is_vowel = matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y');
        if is_vowel && !in_vowel {
            groups += 1;
        }
        in_vowel = is_vowel;
    }
    groups.max(1)
}

/// Flesch reading ease, 0–100, or `None` when there is too little text.
pub fn flesch_reading_ease(text: &str) -> Option<f64> {
    let sentences: Vec<&str> = text
        .split(|c| c == '.' || c == '!' || c == '?')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let words = words_of(text);
    if sentences.is_empty() || words.len() < 10 {
        return None;
    }
    let syllables: i64 = words.iter().map(|w| count_syllables(w)).sum();
    let ease = 206.835
        - 1.015 * (words.len() as f64 / sentences.len() as f64)
        - 84.6 * (syllables as f64 / words.len() as f64);
    // v1: round to 1 decimal, then clamp to 0–100.
    Some(((ease * 10.0).round() / 10.0).clamp(0.0, 100.0))
}

struct ContentFacts {
    word_count: i64,
    reading_ease: Option<f64>,
    headings: Vec<(i64, String)>,
    images: Vec<Option<String>>,
    internal_links: i64,
    external_links: i64,
    first_paragraph: String,
}

fn ascii_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Find `needle` (ASCII, case-insensitive) in `haystack` from `from`.
fn find_ci(haystack: &str, from: usize, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < from + n.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| {
        h[i..i + n.len()]
            .iter()
            .zip(n.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

fn parse_content(content: &str, site_url: &str) -> ContentFacts {
    let plain_text = text_from_content(content);
    let words = words_of(&plain_text);

    // Headings: `<h[1-6] ...>...</hN>` (case-insensitive, non-greedy).
    let mut headings: Vec<(i64, String)> = Vec::new();
    let mut pos = 0;
    while let Some(h) = find_ci(content, pos, "<h") {
        let after = h + 2;
        let level_byte = content.as_bytes().get(after).copied();
        let Some(level_byte) = level_byte else { break };
        if !(b'1'..=b'6').contains(&level_byte) {
            pos = after;
            continue;
        }
        let level = (level_byte - b'0') as i64;
        let Some(gt) = content[after..].find('>') else {
            break;
        };
        let inner_start = after + gt + 1;
        let close = format!("</h{level}>");
        let Some(close_at) = find_ci(content, inner_start, &close) else {
            // Unclosed heading: v1's regex skips it and keeps scanning.
            pos = inner_start;
            continue;
        };
        let inner = &content[inner_start..close_at];
        headings.push((level, text_from_content(inner)));
        pos = close_at + close.len();
    }

    // Images: `<img ...>`; alt from `alt="..."` / `alt='...'` (unquoted
    // alt is no match, exactly like v1's regex).
    let mut images: Vec<Option<String>> = Vec::new();
    let mut pos = 0;
    while let Some(img) = find_ci(content, pos, "<img") {
        let after = img + 4;
        let next = content.as_bytes().get(after).copied();
        // `\b` after `img`: the next char must not be a word char.
        if matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == b'_') {
            pos = after;
            continue;
        }
        let Some(gt) = content[after..].find('>') else {
            break;
        };
        let tag = &content[img..after + gt];
        let alt = find_attr(tag, "alt");
        images.push(alt);
        pos = after + gt + 1;
    }

    // Links: `<a ... href="..." / '...'>`; `#` / `mailto:` / `tel:` /
    // empty skipped; relative `/...` (not `//`) internal; otherwise
    // the href host is compared with the site host.
    let site_host = url_host(site_url);
    let mut internal_links = 0i64;
    let mut external_links = 0i64;
    let mut pos = 0;
    while let Some(a) = find_ci(content, pos, "<a") {
        let after = a + 2;
        let next = content.as_bytes().get(after).copied();
        if matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == b'_') {
            pos = after;
            continue;
        }
        let Some(gt) = content[after..].find('>') else {
            break;
        };
        let tag = &content[a..after + gt];
        pos = after + gt + 1;
        let Some(href) = find_attr(tag, "href") else {
            continue;
        };
        let href = href.trim();
        if href.is_empty()
            || href.starts_with('#')
            || href.starts_with("mailto:")
            || href.starts_with("tel:")
        {
            continue;
        }
        if href.starts_with('/') && !href.starts_with("//") {
            internal_links += 1;
            continue;
        }
        if is_internal_href(href, &site_host) {
            internal_links += 1;
        } else {
            external_links += 1;
        }
    }

    // First paragraph: first `<p>...</p>` text, else the first
    // sentence of the plain text.
    let first_paragraph = {
        let mut found: Option<String> = None;
        if let Some(p) = find_ci(content, 0, "<p") {
            let after = p + 2;
            let next = content.as_bytes().get(after).copied();
            let boundary = !matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == b'_');
            if boundary {
                if let Some(gt) = content[after..].find('>') {
                    let inner_start = after + gt + 1;
                    if let Some(close_at) = find_ci(content, inner_start, "</p>") {
                        found = Some(text_from_content(&content[inner_start..close_at]));
                    }
                }
            }
        }
        match found {
            Some(p) => p,
            None => first_sentence(&plain_text),
        }
    };

    ContentFacts {
        word_count: words.len() as i64,
        reading_ease: flesch_reading_ease(&plain_text),
        headings,
        images,
        internal_links,
        external_links,
        first_paragraph,
    }
}

/// `alt="..."` / `href='...'` inside a tag (case-insensitive name,
/// quoted value only — v1's regex behaviour, including matching
/// inside e.g. `data-alt=`).
fn find_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search_from = 0;
    loop {
        let at = find_ci(&lower, search_from, name)?;
        let after_name = at + name.len();
        if lower.as_bytes().get(after_name) != Some(&b'=') {
            search_from = after_name;
            continue;
        }
        let mut i = after_name + 1;
        let bytes = lower.as_bytes();
        if i >= bytes.len() {
            return None;
        }
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        return Some(tag[start..i].to_string());
    }
}

/// Host (with port, as JS `URL.host`) of an absolute URL, or `""`
/// when the string has no `scheme://host` shape.
fn url_host(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return String::new();
    };
    let rest = &url[scheme_end + 3..];
    rest.split(['/', '?', '#']).next().unwrap_or("").to_string()
}

/// v1 link classification for a non-skipped, non-`/...` href: resolve
/// against the site URL and compare hosts (`new URL(href, siteUrl)`).
fn is_internal_href(href: &str, site_host: &str) -> bool {
    if href.starts_with("//") {
        // Protocol-relative: host is the part after `//`.
        let host = href[2..].split(['/', '?', '#']).next().unwrap_or("");
        return !site_host.is_empty() && ascii_eq(host, site_host);
    }
    // A scheme (`http://`, `javascript:`, `data:`, ...) — the host is
    // whatever follows `://`, or "" for opaque schemes (v1 then
    // counts it external unless the site host is also "").
    if let Some(colon) = href.find(':') {
        let before = &href[..colon];
        if !before.is_empty()
            && before
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && before
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
        {
            let host = url_host(href);
            return !site_host.is_empty() && !host.is_empty() && ascii_eq(&host, site_host);
        }
    }
    // Bare relative reference resolves against the site URL → internal.
    true
}

/// First sentence of plain text: up to and including the first
/// `.`/`!`/`?` that is followed by whitespace (v1's lookbehind split).
fn first_sentence(text: &str) -> String {
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if (b == b'.' || b == b'!' || b == b'?')
            && bytes.get(i + 1).is_some_and(|c| c.is_ascii_whitespace())
        {
            return text[..=i].to_string();
        }
    }
    text.to_string()
}

fn check(id: &str, label: &str, status: CheckStatus, detail: String) -> SeoCheck {
    SeoCheck {
        id: id.to_string(),
        label: label.to_string(),
        status,
        detail,
    }
}

fn contains_keyword(haystack: &str, keyword: &str) -> bool {
    // Slugs use hyphens ("ridge-trail") where text uses spaces.
    let norm = |s: &str| s.to_lowercase().replace('-', " ");
    norm(haystack).contains(&norm(keyword))
}

/// Analyse one post's SEO from its stored fields + content.
/// `site_url` is the site's base URL (for internal/external link
/// classification). Pure over its inputs — no DB access.
pub fn analyze_seo_content(input: AnalyzeSeoInput) -> SeoAnalysis {
    let facts = parse_content(&input.content, &input.site_url);
    let effective_title = if !input.seo_title.is_empty() {
        input.seo_title.clone()
    } else {
        input.title.clone()
    };
    let effective_description = if !input.seo_description.is_empty() {
        input.seo_description.clone()
    } else {
        input.excerpt.clone()
    };
    let mut checks: Vec<SeoCheck> = Vec::new();

    // 1. Title (effective: SEO title when set, else the post title).
    if effective_title.trim().is_empty() {
        checks.push(check(
            "title",
            "Title",
            CheckStatus::Fail,
            "The post has no title at all.".to_string(),
        ));
    } else if (30..=70).contains(&js_len(&effective_title)) {
        checks.push(check(
            "title",
            "Title",
            CheckStatus::Pass,
            format!(
                "Title is {} characters (30–70 is the target range).",
                js_len(&effective_title)
            ),
        ));
    } else {
        checks.push(check(
            "title",
            "Title",
            CheckStatus::Warn,
            format!(
                "Title is {} characters — aim for 30–70 so it fits search results.",
                js_len(&effective_title)
            ),
        ));
    }

    // 2. Meta description (effective: SEO description, else excerpt).
    if effective_description.trim().is_empty() {
        checks.push(check(
            "description",
            "Meta description",
            CheckStatus::Fail,
            "No meta description and no excerpt to fall back on.".to_string(),
        ));
    } else if (50..=160).contains(&js_len(&effective_description)) {
        checks.push(check(
            "description",
            "Meta description",
            CheckStatus::Pass,
            format!(
                "Description is {} characters (50–160 is the target range).",
                js_len(&effective_description)
            ),
        ));
    } else {
        checks.push(check(
            "description",
            "Meta description",
            CheckStatus::Warn,
            format!(
                "Description is {} characters — aim for 50–160 so it fits search results.",
                js_len(&effective_description)
            ),
        ));
    }

    // 3–6. Focus keyword (only when one is set).
    let keyword = input.focus_keyword.trim().to_string();
    if keyword.is_empty() {
        checks.push(check(
            "keyword",
            "Focus keyword",
            CheckStatus::Warn,
            "No focus keyword set — set one to check placement.".to_string(),
        ));
    } else {
        let in_title = contains_keyword(&effective_title, &keyword);
        checks.push(check(
            "keyword-title",
            "Keyword in title",
            if in_title {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            if in_title {
                "The focus keyword appears in the title.".to_string()
            } else {
                "The focus keyword does not appear in the title.".to_string()
            },
        ));
        let in_intro = contains_keyword(&facts.first_paragraph, &keyword);
        checks.push(check(
            "keyword-intro",
            "Keyword in first paragraph",
            if in_intro {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            if in_intro {
                "The focus keyword appears in the first paragraph.".to_string()
            } else {
                "The focus keyword does not appear in the first paragraph.".to_string()
            },
        ));
        let in_slug = contains_keyword(&input.slug, &keyword);
        checks.push(check(
            "keyword-slug",
            "Keyword in slug",
            if in_slug {
                CheckStatus::Pass
            } else {
                CheckStatus::Warn
            },
            if in_slug {
                "The focus keyword appears in the slug.".to_string()
            } else {
                "The focus keyword does not appear in the slug.".to_string()
            },
        ));
        let in_heading = facts
            .headings
            .iter()
            .any(|(_, text)| contains_keyword(text, &keyword));
        checks.push(check(
            "keyword-heading",
            "Keyword in a heading",
            if in_heading {
                CheckStatus::Pass
            } else {
                CheckStatus::Warn
            },
            if in_heading {
                "The focus keyword appears in a heading.".to_string()
            } else {
                "The focus keyword does not appear in any heading.".to_string()
            },
        ));
    }

    // 7. Heading structure.
    let h1_count = facts.headings.iter().filter(|(l, _)| *l == 1).count();
    let skipped = facts
        .headings
        .iter()
        .enumerate()
        .any(|(i, (level, _))| i > 0 && *level > facts.headings[i - 1].0 + 1);
    if h1_count > 0 {
        checks.push(check(
            "headings",
            "Heading structure",
            CheckStatus::Fail,
            format!(
                "Content has {h1_count} <h1> heading(s) — the post title is already the page <h1>."
            ),
        ));
    } else if skipped {
        checks.push(check(
            "headings",
            "Heading structure",
            CheckStatus::Warn,
            "Heading levels skip (e.g. H2 straight to H4) — keep levels in order.".to_string(),
        ));
    } else {
        checks.push(check(
            "headings",
            "Heading structure",
            CheckStatus::Pass,
            if facts.headings.is_empty() {
                "No headings in the content — fine for a short post.".to_string()
            } else {
                "Headings are in order, no <h1> in the content (the title provides it).".to_string()
            },
        ));
    }

    // 8. Image alt coverage.
    let missing_alt = facts
        .images
        .iter()
        .filter(|alt| alt.as_ref().map(|a| a.trim().is_empty()).unwrap_or(true))
        .count();
    if facts.images.is_empty() {
        checks.push(check(
            "images",
            "Image alt text",
            CheckStatus::Pass,
            "No images in the content.".to_string(),
        ));
    } else if missing_alt == 0 {
        checks.push(check(
            "images",
            "Image alt text",
            CheckStatus::Pass,
            format!("All {} image(s) have alt text.", facts.images.len()),
        ));
    } else {
        checks.push(check(
            "images",
            "Image alt text",
            CheckStatus::Fail,
            format!(
                "{missing_alt} of {} image(s) are missing alt text.",
                facts.images.len()
            ),
        ));
    }

    // 9. Internal links.
    checks.push(check(
        "internal-links",
        "Internal links",
        if facts.internal_links >= 1 {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        if facts.internal_links >= 1 {
            format!(
                "{} internal link(s) and {} external link(s).",
                facts.internal_links, facts.external_links
            )
        } else {
            format!(
                "No internal links found ({} external link(s)) — link to related content on this site.",
                facts.external_links
            )
        },
    ));

    // 10. Word count.
    if facts.word_count >= 300 {
        checks.push(check(
            "word-count",
            "Word count",
            CheckStatus::Pass,
            format!("{} words.", facts.word_count),
        ));
    } else if facts.word_count >= 100 {
        checks.push(check(
            "word-count",
            "Word count",
            CheckStatus::Warn,
            format!(
                "Only {} words — 300+ gives search engines more to work with.",
                facts.word_count
            ),
        ));
    } else {
        checks.push(check(
            "word-count",
            "Word count",
            CheckStatus::Fail,
            format!(
                "Only {} words — this is very thin content.",
                facts.word_count
            ),
        ));
    }

    // 11. Readability (Flesch estimate).
    match facts.reading_ease {
        None => checks.push(check(
            "readability",
            "Readability (estimate)",
            CheckStatus::Warn,
            "Too little text to estimate readability.".to_string(),
        )),
        Some(ease) if ease >= 60.0 => checks.push(check(
            "readability",
            "Readability (estimate)",
            CheckStatus::Pass,
            format!("Flesch reading-ease estimate {ease} (60+ is easy to read)."),
        )),
        Some(ease) if ease >= 30.0 => checks.push(check(
            "readability",
            "Readability (estimate)",
            CheckStatus::Warn,
            format!("Flesch reading-ease estimate {ease} — 60+ is easier to read."),
        )),
        Some(ease) => checks.push(check(
            "readability",
            "Readability (estimate)",
            CheckStatus::Fail,
            format!("Flesch reading-ease estimate {ease} — this is hard to read."),
        )),
    }

    // 12. Canonical.
    if !input.canonical.trim().is_empty() {
        checks.push(check(
            "canonical",
            "Canonical URL",
            CheckStatus::Pass,
            "A canonical URL is set.".to_string(),
        ));
    } else {
        checks.push(check(
            "canonical",
            "Canonical URL",
            CheckStatus::Warn,
            "No canonical URL set — the post URL is used as the canonical.".to_string(),
        ));
    }

    let sum: f64 = checks.iter().map(|c| c.status.score()).sum();
    let score = if checks.is_empty() {
        0
    } else {
        (100.0 * sum / checks.len() as f64).round() as i64
    };
    SeoAnalysis {
        post_id: 0,
        score,
        checks,
        word_count: facts.word_count,
        reading_ease: facts.reading_ease,
        internal_links: facts.internal_links,
        external_links: facts.external_links,
        images: facts.images.len() as i64,
        images_missing_alt: missing_alt as i64,
    }
}

/// Analyse a stored post by ID (loads the post + its SEO fields).
/// 404 when the post does not exist.
pub async fn analyze_post_seo(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    site_url: &str,
) -> Result<SeoAnalysis, WordlessError> {
    let post = get_post_by_id(db, site_id, post_id)
        .await
        .map_err(WordlessError::from)?;
    let Some(post) = post else {
        return Err(not_found(&format!("Post {post_id} not found.")));
    };
    let seo = get_post_seo(db, site_id, post_id).await?;
    let mut analysis = analyze_seo_content(AnalyzeSeoInput {
        title: post.title,
        slug: post.slug,
        content: post.content,
        excerpt: post.excerpt,
        seo_title: seo.title,
        seo_description: seo.description,
        focus_keyword: seo.focus_keyword,
        canonical: seo.canonical,
        site_url: site_url.to_string(),
    });
    analysis.post_id = post_id;
    Ok(analysis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordless_seo_core::meta::{set_post_seo, PostSeoInput};
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use wordless_cms::posts::{create_post, CreatePostInput};

    /// ~370 words of short, plain sentences about the ridge trail
    /// (v1 `seo.test.ts` `goodContent()`).
    fn good_content() -> String {
        let intro = "The ridge trail is the pack's favourite route. ".to_string()
            + &"It winds through pine forest and open grass. ".repeat(12);
        let body = "The pack moves as one along the ridge trail. ".to_string()
            + "Each wolf watches the one ahead. "
            + &"The trail stays clear because all walk it each day. ".repeat(12);
        let rest = "When the sun climbs high, the pack rests by the stream. ".to_string()
            + "The water is cold and clear. "
            + &"The shade gives all a place to rest. ".repeat(12);
        format!(
            "<!-- wp:paragraph --><p>{intro}</p><!-- /wp:paragraph --><!-- wp:heading {{\"level\":2}} --><h2 class=\"wp-block-heading\">How the pack travels the ridge trail</h2><!-- /wp:heading --><!-- wp:paragraph --><p>{body}</p><!-- /wp:paragraph --><!-- wp:paragraph --><p>{rest}</p><!-- /wp:paragraph --><!-- wp:paragraph --><p>Read the <a href=\"/trail-guide\">trail guide</a> and our <a href=\"https://example.org/wolves\">friends elsewhere</a>.</p><!-- /wp:paragraph --><!-- wp:image --><figure class=\"wp-block-image\"><img src=\"/media/x.png\" alt=\"A wolf on the ridge trail\"/></figure><!-- /wp:image -->"
        )
    }

    fn make_post(db: &SqliteDb, title: &str, content: &str, excerpt: &str) -> i64 {
        block_on(async {
            create_post(
                db,
                1,
                CreatePostInput {
                    title: title.to_string(),
                    content: Some(content.to_string()),
                    excerpt: Some(excerpt.to_string()),
                    status: Some("publish".to_string()),
                    author_id: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .id
        })
    }

    #[test]
    fn scores_a_well_built_post_100() {
        let db = SqliteDb::new();
        let id = make_post(
            &db,
            "Walking the Ridge Trail at Dawn",
            &good_content(),
            "A plain walk along the ridge trail at dawn, with the pack.",
        );
        block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                description: Some(
                    "Walk the ridge trail at dawn with the pack — route, rest stops and what to bring."
                        .to_string(),
                ),
                focus_keyword: Some("ridge trail".to_string()),
                canonical: Some(
                    "https://example.test/walking-the-ridge-trail-at-dawn".to_string(),
                ),
                ..Default::default()
            },
        ))
        .unwrap();
        let analysis = block_on(analyze_post_seo(&db, 1, id, "http://localhost:8787")).unwrap();
        assert_eq!(analysis.score, 100);
        assert!(analysis.word_count >= 300);
        assert_eq!(analysis.internal_links, 1);
        assert_eq!(analysis.external_links, 1);
        assert_eq!(analysis.images, 1);
        assert_eq!(analysis.images_missing_alt, 0);
        assert!(analysis.reading_ease.unwrap_or(0.0) >= 60.0);
        assert!(analysis
            .checks
            .iter()
            .all(|c| c.status == CheckStatus::Pass));
    }

    #[test]
    fn scores_an_empty_post_50_with_the_expected_failures() {
        let db = SqliteDb::new();
        let id = make_post(&db, "x", "", "");
        let analysis = block_on(analyze_post_seo(&db, 1, id, "http://localhost:8787")).unwrap();
        assert_eq!(analysis.score, 50);
        let by_id: std::collections::HashMap<&str, CheckStatus> = analysis
            .checks
            .iter()
            .map(|c| (c.id.as_str(), c.status))
            .collect();
        assert_eq!(by_id["description"], CheckStatus::Fail);
        assert_eq!(by_id["word-count"], CheckStatus::Fail);
        assert_eq!(by_id["keyword"], CheckStatus::Warn);
        assert_eq!(by_id["headings"], CheckStatus::Pass);
        assert_eq!(analysis.reading_ease, None);
    }

    #[test]
    fn flags_missing_alt_h1s_and_skipped_heading_levels() {
        let db = SqliteDb::new();
        let id = make_post(
            &db,
            "Alt test post title here",
            "<h1>Bad H1</h1><h2>Section</h2><h4>Skipped</h4><img src=\"/x.png\"><img src=\"/y.png\" alt=\"\">",
            "An excerpt that is long enough to pass the description check.",
        );
        let analysis = block_on(analyze_post_seo(&db, 1, id, "http://localhost:8787")).unwrap();
        let by_id: std::collections::HashMap<&str, &SeoCheck> =
            analysis.checks.iter().map(|c| (c.id.as_str(), c)).collect();
        assert_eq!(by_id["images"].status, CheckStatus::Fail);
        assert_eq!(analysis.images_missing_alt, 2);
        assert_eq!(by_id["headings"].status, CheckStatus::Fail);
    }

    #[test]
    fn flags_a_keyword_missing_from_title_and_intro() {
        let result = analyze_seo_content(AnalyzeSeoInput {
            title: "Walking the Ridge Trail at Dawn".to_string(),
            slug: "walking-the-ridge-trail-at-dawn".to_string(),
            content: "<p>Some plain text about walking.</p>".repeat(30),
            excerpt: String::new(),
            seo_title: String::new(),
            seo_description: "A description that is long enough to pass the check here."
                .to_string(),
            focus_keyword: "volcano".to_string(),
            canonical: String::new(),
            site_url: "http://localhost:8787".to_string(),
        });
        let by_id: std::collections::HashMap<&str, CheckStatus> = result
            .checks
            .iter()
            .map(|c| (c.id.as_str(), c.status))
            .collect();
        assert_eq!(by_id["keyword-title"], CheckStatus::Fail);
        assert_eq!(by_id["keyword-intro"], CheckStatus::Fail);
        assert_eq!(by_id["keyword-slug"], CheckStatus::Warn);
    }

    #[test]
    fn estimates_readability_and_strips_block_comments() {
        assert_eq!(
            flesch_reading_ease(&"The cat sat on the mat. ".repeat(20)),
            Some(100.0)
        );
        assert_eq!(flesch_reading_ease("Short."), None);
        assert_eq!(
            text_from_content(
                "<!-- wp:paragraph --><p>Hello &amp; goodbye</p><!-- /wp:paragraph -->"
            ),
            "Hello & goodbye"
        );
    }
}
