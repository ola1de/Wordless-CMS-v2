//! Paginated XML sitemap (port of v1 `src/modules/seo/sitemap.ts`).
//!
//! One sitemap file holds at most 1,000 URLs. Up to 1,000 URLs the
//! single `/sitemap.xml` is a plain `<urlset>` (M2 behaviour, kept).
//! Above that, `/sitemap.xml` becomes a `<sitemapindex>` and each
//! chunk is served at `/sitemap.xml?page=N`. Total collection is
//! capped at 50,000 URLs (the sitemap protocol's per-file ceiling,
//! applied to the whole set).
//!
//! changefreq / priority formula (documented, computed from stored data):
//! - home: daily / 1.0
//! - post, updated < 7 days ago: daily / 0.9
//! - post, updated < 30 days ago: weekly / 0.8
//! - post, updated < 365 days ago: monthly / 0.6
//! - post, older: yearly / 0.5
//! - page: monthly / 0.7
//! - term archive: weekly / 0.5
//!
//! Image entries come from `wp:image {"id":N}` blocks in post content
//! and are only included when the attachment actually exists in the
//! media library. At most 10 images per URL.

use serde::{Deserialize, Serialize};
use wordless_cms::permalinks::{post_url, term_url};
use wordless_cms::posts::row_to_post;
use wordless_cms::posts::Post;
use wordless_cms::terms::{list_terms, ListTermsOptions};
use wordless_db::row::row_text;
use wordless_db::Db;
use wordless_kernel::error::WordlessError;
use wordless_kernel::time::{ms_from_iso, now_ms};
use wordless_media::service::get_media_record;
use wordless_seo_core::site::site_url;

pub const SITEMAP_PAGE_SIZE: i64 = 1000;
pub const SITEMAP_MAX_URLS: i64 = 50000;
const MAX_IMAGES_PER_URL: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SitemapImage {
    pub loc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SitemapUrl {
    pub loc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lastmod: Option<String>,
    pub changefreq: String,
    pub priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<SitemapImage>>,
}

/// XML escaping (same 5 replacements as v1 `escapeXml`; `'` → `&apos;`).
pub fn escape_xml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Parse a stored date to epoch ms. Stored dates are ISO-8601 UTC
/// text, but v1 also wrote the space form (`2026-11-19 00:00:00`);
/// that form is normalised to `T...Z` before parsing. Unparseable
/// dates are treated as infinitely old (v1 `ageDays` returns 9999).
fn parse_date_ms(iso: &str) -> Option<u64> {
    if let Some(ms) = ms_from_iso(iso) {
        return Some(ms);
    }
    let bytes = iso.as_bytes();
    if bytes.len() >= 19 && bytes[10] == b' ' {
        let mut norm = iso.to_string();
        norm.replace_range(10..11, "T");
        if !norm.ends_with('Z') {
            norm.push('Z');
        }
        return ms_from_iso(&norm);
    }
    None
}

fn age_days(iso: &str, now: u64) -> f64 {
    match parse_date_ms(iso) {
        Some(d) => ((now as i64 - d as i64).max(0) as f64) / 86_400_000.0,
        None => 9999.0,
    }
}

fn post_freq_priority(post: &Post, now: u64) -> (String, String) {
    if post.post_type == "page" {
        return ("monthly".to_string(), "0.7".to_string());
    }
    let modified = if !post.modified_gmt.is_empty() {
        post.modified_gmt.clone()
    } else if !post.modified.is_empty() {
        post.modified.clone()
    } else {
        post.date.clone()
    };
    let age = age_days(&modified, now);
    if age < 7.0 {
        ("daily".to_string(), "0.9".to_string())
    } else if age < 30.0 {
        ("weekly".to_string(), "0.8".to_string())
    } else if age < 365.0 {
        ("monthly".to_string(), "0.6".to_string())
    } else {
        ("yearly".to_string(), "0.5".to_string())
    }
}

async fn post_images(
    db: &dyn Db,
    site_id: i64,
    base: &str,
    post: &Post,
) -> Result<Vec<SitemapImage>, WordlessError> {
    let mut images: Vec<SitemapImage> = Vec::new();
    // v1: /<!--\s*wp:image\s+(\{[^}]*\})\s*-->/g
    let content = &post.content;
    let mut pos = 0;
    while images.len() < MAX_IMAGES_PER_URL {
        let Some(open) = content[pos..].find("<!--") else {
            break;
        };
        let mut i = pos + open + 4;
        while content
            .as_bytes()
            .get(i)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            i += 1;
        }
        if !content[i..].starts_with("wp:image") {
            pos = i;
            continue;
        }
        i += "wp:image".len();
        while content
            .as_bytes()
            .get(i)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            i += 1;
        }
        if !content[i..].starts_with('{') {
            pos = i;
            continue;
        }
        let Some(close_brace) = content[i..].find('}') else {
            break;
        };
        let attrs_src = &content[i..i + close_brace + 1];
        let mut j = i + close_brace + 1;
        while content
            .as_bytes()
            .get(j)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            j += 1;
        }
        if !content[j..].starts_with("-->") {
            pos = j;
            continue;
        }
        pos = j + 3;
        let id = match serde_json::from_str::<serde_json::Value>(attrs_src) {
            Ok(v) => v.get("id").and_then(|n| n.as_i64()).unwrap_or(0),
            Err(_) => continue,
        };
        if id <= 0 {
            continue;
        }
        let Some(record) = get_media_record(db, site_id, id).await? else {
            continue;
        };
        if record.url.is_empty() {
            continue;
        }
        images.push(SitemapImage {
            loc: if record.url.starts_with("http") {
                record.url.clone()
            } else {
                format!("{base}{}", record.url)
            },
            title: if record.title.is_empty() {
                None
            } else {
                Some(record.title.clone())
            },
        });
    }
    Ok(images)
}

/// Every URL the sitemap covers, in sitemap order (home, posts, terms).
pub async fn collect_sitemap_urls(db: &dyn Db, site_id: i64) -> Result<Vec<SitemapUrl>, WordlessError> {
    let base = site_url(db, site_id).await?;
    let now = now_ms();
    let mut urls: Vec<SitemapUrl> = vec![SitemapUrl {
        loc: format!("{base}/"),
        lastmod: None,
        changefreq: "daily".to_string(),
        priority: "1.0".to_string(),
        images: None,
    }];
    let rows = db
        .query_all(
            "SELECT * FROM wp_posts WHERE post_status = 'publish' AND post_type IN ('post', 'page') AND site_id = ? ORDER BY post_date DESC, ID DESC LIMIT ?",
            &[wordless_db::Param::Integer(site_id), wordless_db::Param::Integer(SITEMAP_MAX_URLS)],
        )
        .await
        .map_err(WordlessError::from)?;
    for row in &rows {
        let post = row_to_post(row);
        let (changefreq, priority) = post_freq_priority(&post, now);
        let modified = if !post.modified_gmt.is_empty() {
            post.modified_gmt.clone()
        } else if !post.modified.is_empty() {
            post.modified.clone()
        } else {
            post.date.clone()
        };
        let lastmod = if modified.is_empty() {
            None
        } else {
            Some(modified.chars().take(10).collect())
        };
        let mut entry = SitemapUrl {
            loc: format!("{base}{}", post_url(db, site_id, &post).await?),
            lastmod,
            changefreq,
            priority,
            images: None,
        };
        if post.post_type == "post" {
            let images = post_images(db, site_id, &base, &post).await?;
            if !images.is_empty() {
                entry.images = Some(images);
            }
        }
        urls.push(entry);
        if urls.len() as i64 >= SITEMAP_MAX_URLS {
            break;
        }
    }
    for taxonomy in ["category", "tag"] {
        let result = list_terms(
            db,
            site_id,
            taxonomy,
            ListTermsOptions {
                per_page: Some(100),
                ..Default::default()
            },
        )
        .await?;
        for term in &result.terms {
            if term.count == 0 {
                continue;
            }
            urls.push(SitemapUrl {
                loc: format!("{base}{}", term_url(&term.taxonomy, &term.slug)),
                lastmod: None,
                changefreq: "weekly".to_string(),
                priority: "0.5".to_string(),
                images: None,
            });
            if urls.len() as i64 >= SITEMAP_MAX_URLS {
                break;
            }
        }
        if urls.len() as i64 >= SITEMAP_MAX_URLS {
            break;
        }
    }
    Ok(urls)
}

fn url_xml(u: &SitemapUrl) -> String {
    let images = match &u.images {
        Some(imgs) if !imgs.is_empty() => imgs
            .iter()
            .map(|img| {
                format!(
                    "    <image:image><image:loc>{}</image:loc>{}</image:image>",
                    escape_xml(&img.loc),
                    img.title
                        .as_ref()
                        .map(|t| format!("<image:title>{}</image:title>", escape_xml(t)))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    format!(
        "  <url><loc>{}</loc>{}<changefreq>{}</changefreq><priority>{}</priority>{}</url>",
        escape_xml(&u.loc),
        u.lastmod
            .as_ref()
            .map(|d| format!("<lastmod>{}</lastmod>", escape_xml(d)))
            .unwrap_or_default(),
        u.changefreq,
        u.priority,
        if images.is_empty() {
            String::new()
        } else {
            format!("\n{images}")
        }
    )
}

fn urlset_xml(urls: &[SitemapUrl]) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" xmlns:image=\"http://www.google.com/schemas/sitemap-image/1.1\">\n{}\n</urlset>",
        urls.iter().map(url_xml).collect::<Vec<_>>().join("\n")
    )
}

/// Sitemap index: one `<sitemap>` per 1,000-URL chunk.
pub async fn sitemap_index_xml(
    db: &dyn Db,
    site_id: i64,
    precollected: Option<Vec<SitemapUrl>>,
) -> Result<String, WordlessError> {
    let base = site_url(db, site_id).await?;
    let urls = match precollected {
        Some(u) => u,
        None => collect_sitemap_urls(db, site_id).await?,
    };
    let pages = ((urls.len() as i64 + SITEMAP_PAGE_SIZE - 1) / SITEMAP_PAGE_SIZE).max(1);
    let mut entries: Vec<String> = Vec::new();
    for p in 1..=pages {
        let start = ((p - 1) * SITEMAP_PAGE_SIZE) as usize;
        let end = (p * SITEMAP_PAGE_SIZE).min(urls.len() as i64) as usize;
        let chunk = &urls[start..end];
        let lastmod = chunk.iter().filter_map(|u| u.lastmod.clone()).max();
        entries.push(format!(
            "  <sitemap><loc>{}</loc>{}</sitemap>",
            escape_xml(&format!("{base}/sitemap.xml?page={p}")),
            lastmod
                .map(|d| format!("<lastmod>{}</lastmod>", escape_xml(&d)))
                .unwrap_or_default()
        ));
    }
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n{}\n</sitemapindex>",
        entries.join("\n")
    ))
}

/// Sitemap for a page. `page = None` means "the whole sitemap": a
/// plain `<urlset>` when there are ≤ 1,000 URLs, otherwise the index.
/// `page = N` returns chunk N of the paginated sitemap (1-based,
/// clamped to 1..=totalPages).
pub async fn sitemap_xml(db: &dyn Db, site_id: i64, page: Option<i64>) -> Result<String, WordlessError> {
    let urls = collect_sitemap_urls(db, site_id).await?;
    match page {
        None => {
            if urls.len() as i64 <= SITEMAP_PAGE_SIZE {
                Ok(urlset_xml(&urls))
            } else {
                sitemap_index_xml(db, site_id, Some(urls)).await
            }
        }
        Some(p) => {
            let total_pages =
                ((urls.len() as i64 + SITEMAP_PAGE_SIZE - 1) / SITEMAP_PAGE_SIZE).max(1);
            let p = p.clamp(1, total_pages);
            let start = ((p - 1) * SITEMAP_PAGE_SIZE) as usize;
            let end = (p * SITEMAP_PAGE_SIZE).min(urls.len() as i64) as usize;
            Ok(urlset_xml(&urls[start..end]))
        }
    }
}

// ------------------------------------------------------- P10: image + video sitemaps

/// One video reference found in post content (P10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapVideo {
    /// URL of the page the video appears on (`<loc>`).
    pub page_loc: String,
    /// Player / content URL (the embed or file URL).
    pub content_loc: String,
    /// Title shown in search results (the post title).
    pub title: String,
    /// Description (post excerpt, else the title).
    pub description: String,
    /// Thumbnail URL — required by the video sitemap protocol, so a
    /// video without a thumbnail is NOT listed (see
    /// [`collect_sitemap_videos`]).
    pub thumbnail_loc: String,
}

/// The post's featured image URL, when one is set and exists.
async fn post_thumbnail(
    db: &dyn Db,
    site_id: i64,
    base: &str,
    post: &Post,
) -> Result<Option<String>, WordlessError> {
    let row = db
        .query_one(
            "SELECT meta_value FROM wp_postmeta WHERE post_id = ? AND meta_key = '_thumbnail_id' AND site_id = ? LIMIT 1",
            &[wordless_db::Param::Integer(post.id), wordless_db::Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    let Some(row) = row else { return Ok(None) };
    let id: i64 = row_text(&row, "meta_value").trim().parse().unwrap_or(0);
    if id <= 0 {
        return Ok(None);
    }
    let Some(record) = get_media_record(db, site_id, id).await? else {
        return Ok(None);
    };
    if record.url.is_empty() {
        return Ok(None);
    }
    Ok(Some(if record.url.starts_with("http") {
        record.url.clone()
    } else {
        format!("{base}{}", record.url)
    }))
}

/// Extract `src="..."` values from a tag fragment.
fn tag_srcs(fragment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = fragment;
    while let Some(pos) = rest.find("src=") {
        let after = &rest[pos + 4..];
        let quote = after.chars().next();
        let Some(q) = quote else { break };
        if q != '"' && q != '\'' {
            rest = after;
            continue;
        }
        let Some(end) = after[1..].find(q) else { break };
        let src = &after[1..1 + end];
        if !src.is_empty() {
            out.push(src.to_string());
        }
        rest = &after[1 + end + 1..];
    }
    out
}

/// Video references in one post's content: `wp:video` blocks (by
/// attachment id or `src`), `<video>` / `<source>` tags, and
/// YouTube / Vimeo iframe embeds. A reference is only a video
/// sitemap entry when the post also has a thumbnail (featured
/// image) — the protocol requires `thumbnail_loc`, and inventing
/// one would be fabrication.
pub async fn collect_sitemap_videos(db: &dyn Db, site_id: i64) -> Result<Vec<SitemapVideo>, WordlessError> {
    let base = site_url(db, site_id).await?;
    let rows = db
        .query_all(
            "SELECT * FROM wp_posts WHERE post_status = 'publish' AND post_type IN ('post', 'page') AND site_id = ? ORDER BY post_date DESC, ID DESC LIMIT ?",
            &[wordless_db::Param::Integer(site_id), wordless_db::Param::Integer(SITEMAP_MAX_URLS)],
        )
        .await
        .map_err(WordlessError::from)?;
    let mut videos: Vec<SitemapVideo> = Vec::new();
    for row in &rows {
        let post = row_to_post(row);
        let mut locs: Vec<String> = Vec::new();
        // wp:video blocks: {"id":N} or {"src":"..."}.
        let mut pos = 0;
        let content = post.content.clone();
        while let Some(open) = content[pos..].find("wp:video") {
            let start = pos + open;
            let Some(close) = content[start..].find("-->") else {
                break;
            };
            let block = &content[start..start + close];
            if let Some(attrs_start) = block.find('{') {
                if let Some(attrs_end) = block[attrs_start..].find('}') {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(
                        &block[attrs_start..attrs_start + attrs_end + 1],
                    ) {
                        if let Some(src) = v.get("src").and_then(|s| s.as_str()) {
                            if !src.is_empty() {
                                locs.push(src.to_string());
                            }
                        } else if let Some(id) = v.get("id").and_then(|n| n.as_i64()) {
                            if id > 0 {
                                if let Some(record) = get_media_record(db, site_id, id).await? {
                                    if !record.url.is_empty() {
                                        locs.push(if record.url.starts_with("http") {
                                            record.url.clone()
                                        } else {
                                            format!("{base}{}", record.url)
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            pos = start + close + 3;
        }
        // <video> / <source> tags.
        let mut vpos = 0;
        while let Some(open) = content[vpos..].find("<video") {
            let start = vpos + open;
            let Some(end) = content[start..].find('>') else {
                break;
            };
            for src in tag_srcs(&content[start..start + end]) {
                locs.push(src);
            }
            // <source> tags inside this <video> element.
            let close_tag = content[start..]
                .find("</video>")
                .map(|i| start + i)
                .unwrap_or(content.len());
            let mut spos = start + end;
            while let Some(sopen) = content[spos..close_tag].find("<source") {
                let sstart = spos + sopen;
                let Some(send) = content[sstart..].find('>') else {
                    break;
                };
                for src in tag_srcs(&content[sstart..sstart + send]) {
                    locs.push(src);
                }
                spos = sstart + send + 1;
            }
            vpos = start + end + 1;
        }
        // YouTube / Vimeo iframe embeds.
        let mut ipos = 0;
        while let Some(open) = content[ipos..].find("<iframe") {
            let start = ipos + open;
            let Some(end) = content[start..].find('>') else {
                break;
            };
            for src in tag_srcs(&content[start..start + end]) {
                if src.contains("youtube.com/embed/")
                    || src.contains("youtube-nocookie.com/embed/")
                    || src.contains("youtu.be/")
                    || src.contains("player.vimeo.com/video/")
                {
                    locs.push(src);
                }
            }
            ipos = start + end + 1;
        }
        if locs.is_empty() {
            continue;
        }
        let Some(thumbnail) = post_thumbnail(db, site_id, &base, &post).await? else {
            continue;
        };
        let description = if post.excerpt.is_empty() {
            post.title.clone()
        } else {
            post.excerpt.clone()
        };
        let page_url = format!("{base}{}", post_url(db, site_id, &post).await?);
        for loc in locs {
            videos.push(SitemapVideo {
                page_loc: page_url.clone(),
                content_loc: loc,
                title: if post.title.is_empty() {
                    "(untitled)".to_string()
                } else {
                    post.title.clone()
                },
                description: description.clone(),
                thumbnail_loc: thumbnail.clone(),
            });
        }
    }
    Ok(videos)
}

fn video_xml(v: &SitemapVideo) -> String {
    format!(
        "  <url><loc>{}</loc><video:video><video:thumbnail_loc>{}</video:thumbnail_loc><video:title>{}</video:title><video:description>{}</video:description><video:content_loc>{}</video:content_loc></video:video></url>",
        escape_xml(&v.page_loc),
        escape_xml(&v.thumbnail_loc),
        escape_xml(&v.title),
        escape_xml(&v.description),
        escape_xml(&v.content_loc)
    )
}

/// Dedicated image sitemap (`/sitemap-images.xml`): only URLs that
/// carry images, with their `image:image` entries.
pub async fn image_sitemap_xml(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let urls = collect_sitemap_urls(db, site_id).await?;
    let with_images: Vec<&SitemapUrl> = urls
        .iter()
        .filter(|u| u.images.as_ref().is_some_and(|i| !i.is_empty()))
        .collect();
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" xmlns:image=\"http://www.google.com/schemas/sitemap-image/1.1\">\n{}\n</urlset>",
        with_images.iter().map(|u| url_xml(u)).collect::<Vec<_>>().join("\n")
    ))
}

/// Dedicated video sitemap (`/sitemap-videos.xml`). Empty
/// `<urlset>` when no video has a thumbnail (see
/// [`collect_sitemap_videos`]) — valid XML, not an error.
pub async fn video_sitemap_xml(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let videos = collect_sitemap_videos(db, site_id).await?;
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" xmlns:video=\"http://www.google.com/schemas/sitemap-video/1.1\">\n{}\n</urlset>",
        videos.iter().map(video_xml).collect::<Vec<_>>().join("\n")
    ))
}

/// Sitemap index over ALL sitemap files (`/sitemap-index.xml`):
/// the paginated post sitemap chunks, the image sitemap, and the
/// video sitemap. The plain `/sitemap.xml` behaviour is unchanged
/// (v1 parity) — this index is additive.
pub async fn sitemap_index_all_xml(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let base = site_url(db, site_id).await?;
    let urls = collect_sitemap_urls(db, site_id).await?;
    let pages = ((urls.len() as i64 + SITEMAP_PAGE_SIZE - 1) / SITEMAP_PAGE_SIZE).max(1);
    let mut entries: Vec<String> = Vec::new();
    for p in 1..=pages {
        let start = ((p - 1) * SITEMAP_PAGE_SIZE) as usize;
        let end = (p * SITEMAP_PAGE_SIZE).min(urls.len() as i64) as usize;
        let chunk = &urls[start..end];
        let lastmod = chunk.iter().filter_map(|u| u.lastmod.clone()).max();
        entries.push(format!(
            "  <sitemap><loc>{}</loc>{}</sitemap>",
            escape_xml(&format!("{base}/sitemap.xml?page={p}")),
            lastmod
                .map(|d| format!("<lastmod>{}</lastmod>", escape_xml(&d)))
                .unwrap_or_default()
        ));
    }
    entries.push(format!(
        "  <sitemap><loc>{}/sitemap-images.xml</loc></sitemap>",
        escape_xml(&base)
    ));
    entries.push(format!(
        "  <sitemap><loc>{}/sitemap-videos.xml</loc></sitemap>",
        escape_xml(&base)
    ));
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n{}\n</sitemapindex>",
        entries.join("\n")
    ))
}

/// Sitemap inventory for the admin sitemap viewer: each file with
/// its URL and entry count.
pub async fn sitemap_overview(db: &dyn Db, site_id: i64) -> Result<Vec<(String, i64)>, WordlessError> {
    let base = site_url(db, site_id).await?;
    let urls = collect_sitemap_urls(db, site_id).await?;
    let image_count = urls
        .iter()
        .filter(|u| u.images.as_ref().is_some_and(|i| !i.is_empty()))
        .count() as i64;
    let video_count = collect_sitemap_videos(db, site_id).await?.len() as i64;
    let pages = ((urls.len() as i64 + SITEMAP_PAGE_SIZE - 1) / SITEMAP_PAGE_SIZE).max(1);
    let mut out = vec![(format!("{base}/sitemap-index.xml"), pages + 2)];
    for p in 1..=pages {
        let start = ((p - 1) * SITEMAP_PAGE_SIZE) as usize;
        let end = (p * SITEMAP_PAGE_SIZE).min(urls.len() as i64) as usize;
        out.push((format!("{base}/sitemap.xml?page={p}"), (end - start) as i64));
    }
    out.push((format!("{base}/sitemap-images.xml"), image_count));
    out.push((format!("{base}/sitemap-videos.xml"), video_count));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use wordless_cms::posts::{create_post, CreatePostInput};
    use wordless_db::Param;

    fn make_post(db: &SqliteDb, title: &str, slug: &str) -> i64 {
        block_on(async {
            create_post(
                db,
                1,
                CreatePostInput {
                    title: title.to_string(),
                    slug: Some(slug.to_string()),
                    content: Some("Body.".to_string()),
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

    fn insert_attachment(db: &SqliteDb, key: &str, title: &str) -> i64 {
        let id = block_on(db.insert_post(
            title,
            "",
            "",
            "inherit",
            "wolf-png",
            "attachment",
            "2026-11-19T00:00:00.000Z",
            1,
        ));
        block_on(async {
            db.exec(
                "INSERT INTO wp_postmeta (post_id, meta_key, meta_value) VALUES (?, '_wp_attached_file', ?)",
                &[Param::Integer(id), Param::Text(key.to_string())],
            )
            .await
            .unwrap();
            db.exec(
                "INSERT INTO wp_postmeta (post_id, meta_key, meta_value) VALUES (?, '_wp_attachment_metadata', ?)",
                &[
                    Param::Integer(id),
                    Param::Text(
                        serde_json::json!({"filename": "wolf.png", "size": 8, "alt": "A wolf"})
                            .to_string(),
                    ),
                ],
            )
            .await
            .unwrap();
        });
        id
    }

    #[test]
    fn serves_a_single_urlset_under_1000_urls() {
        let db = SqliteDb::new();
        make_post(&db, "Hello", "hello");
        let xml = block_on(sitemap_xml(&db, 1, None)).unwrap();
        assert!(xml.contains("<urlset"));
        assert!(!xml.contains("<sitemapindex"));
        assert!(xml.contains("<changefreq>daily</changefreq><priority>1.0</priority>"));
        assert!(xml.contains("<loc>http://localhost:8787/hello</loc>"));
    }

    #[test]
    fn paginates_at_1000_urls_with_a_1050_post_fixture() {
        let db = SqliteDb::new();
        // Bulk insert (the service path would take too long for 1,050 posts).
        let mut values: Vec<String> = Vec::new();
        for i in 1..=1050 {
            values.push(format!(
                "(1, '2026-11-19 00:00:00', '2026-11-19 00:00:00', 'Body {i}', 'Post {i}', '', 'publish', 'post-{i}', '2026-11-19 00:00:00', '2026-11-19 00:00:00', 0, 'post', '', 'open')"
            ));
        }
        block_on(async {
            db.exec(
                &format!(
                    "INSERT INTO wp_posts (post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt, post_status, post_name, post_modified, post_modified_gmt, post_parent, post_type, post_mime_type, comment_status) VALUES {}",
                    values.join(",")
                ),
                &[],
            )
            .await
            .unwrap();
        });
        let urls = block_on(collect_sitemap_urls(&db, 1)).unwrap();
        assert_eq!(urls.len(), 1051); // home + 1,050 posts
        let index = block_on(sitemap_index_xml(&db, 1, Some(urls))).unwrap();
        assert!(index.contains("<sitemapindex"));
        assert!(index.contains("/sitemap.xml?page=1"));
        assert!(index.contains("/sitemap.xml?page=2"));
        let page1 = block_on(sitemap_xml(&db, 1, Some(1))).unwrap();
        assert_eq!(page1.matches("<url>").count(), 1000);
        let page2 = block_on(sitemap_xml(&db, 1, Some(2))).unwrap();
        assert_eq!(page2.matches("<url>").count(), 51);
        let as_index = block_on(sitemap_xml(&db, 1, None)).unwrap();
        assert!(as_index.contains("<sitemapindex"));
    }

    #[test]
    fn includes_image_entries_only_for_attachments_that_exist() {
        let db = SqliteDb::new();
        let att = insert_attachment(&db, "wolf.png", "Wolf photo");
        block_on(async {
            create_post(
                &db, 1,
                CreatePostInput {
                    title: "With image".to_string(),
                    slug: Some("with-image".to_string()),
                    content: Some(format!(
                        "<!-- wp:image {{\"id\":{att}}} --><figure class=\"wp-block-image\"><img src=\"/media/wolf.png\" alt=\"A wolf\"/></figure><!-- /wp:image --><!-- wp:image {{\"id\":99999}} --><figure class=\"wp-block-image\"><img src=\"/media/nope.png\" alt=\"Missing\"/></figure><!-- /wp:image -->"
                    )),
                    status: Some("publish".to_string()),
                    author_id: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        });
        let xml = block_on(sitemap_xml(&db, 1, None)).unwrap();
        assert!(xml.contains("<image:image>"));
        assert!(xml.contains("/media/wolf.png"));
        assert!(xml.contains("<image:title>Wolf photo</image:title>"));
        assert!(!xml.contains("/media/nope.png"));
    }

    #[test]
    fn includes_term_archives_with_posts() {
        let db = SqliteDb::new();
        make_post(&db, "Hello", "hello");
        // A category with count 1, and a tag with count 0.
        block_on(async {
            db.exec(
                "INSERT INTO wp_terms (term_id, name, slug, term_group) VALUES (1, 'News', 'news', 0), (2, 'Empty', 'empty', 0)",
                &[],
            )
            .await
            .unwrap();
            db.exec(
                "INSERT INTO wp_term_taxonomy (term_taxonomy_id, term_id, taxonomy, description, parent, count) VALUES (1, 1, 'category', '', 0, 1), (2, 2, 'tag', '', 0, 0)",
                &[],
            )
            .await
            .unwrap();
        });
        let urls = block_on(collect_sitemap_urls(&db, 1)).unwrap();
        assert!(urls.iter().any(|u| u.loc.ends_with("/category/news")));
        assert!(!urls.iter().any(|u| u.loc.ends_with("/tag/empty")));
    }

    fn set_thumbnail(db: &SqliteDb, post_id: i64, att_id: i64) {
        block_on(async {
            db.exec(
                "INSERT INTO wp_postmeta (post_id, meta_key, meta_value) VALUES (?, '_thumbnail_id', ?)",
                &[Param::Integer(post_id), Param::Integer(att_id)],
            )
            .await
            .unwrap();
        });
    }

    #[test]
    fn dedicated_image_sitemap_lists_only_urls_with_images() {
        let db = SqliteDb::new();
        let att = insert_attachment(&db, "wolf.png", "Wolf photo");
        block_on(async {
            create_post(
                &db, 1,
                CreatePostInput {
                    title: "With image".to_string(),
                    slug: Some("with-image".to_string()),
                    content: Some(format!(
                        "<!-- wp:image {{\"id\":{att}}} --><figure class=\"wp-block-image\"><img src=\"/media/wolf.png\" alt=\"A wolf\"/></figure><!-- /wp:image -->"
                    )),
                    status: Some("publish".to_string()),
                    author_id: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        });
        make_post(&db, "Plain", "plain");
        let xml = block_on(image_sitemap_xml(&db, 1)).unwrap();
        assert!(xml.contains("<urlset"));
        assert!(xml.contains("/with-image"));
        assert!(xml.contains("/media/wolf.png"));
        assert!(!xml.contains("/plain"));
        assert_eq!(xml.matches("<url>").count(), 1);
    }

    #[test]
    fn video_sitemap_lists_embeds_only_with_a_thumbnail() {
        let db = SqliteDb::new();
        let att = insert_attachment(&db, "poster.png", "Poster");
        // Post WITH a featured image + a YouTube embed: listed.
        let with_thumb = block_on(async {
            create_post(
                &db, 1,
                CreatePostInput {
                    title: "Video Post".to_string(),
                    slug: Some("video-post".to_string()),
                    content: Some(
                        "<p>Watch:</p><iframe src=\"https://www.youtube.com/embed/dQw4w9WgXcQ\"></iframe>"
                            .to_string(),
                    ),
                    excerpt: Some("A video post.".to_string()),
                    status: Some("publish".to_string()),
                    author_id: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .id
        });
        set_thumbnail(&db, with_thumb, att);
        // Post WITHOUT a thumbnail + a <video> tag: NOT listed
        // (the protocol requires thumbnail_loc; none is invented).
        block_on(async {
            create_post(
                &db, 1,
                CreatePostInput {
                    title: "Bare Video".to_string(),
                    slug: Some("bare-video".to_string()),
                    content: Some("<video src=\"/media/clip.mp4\"></video>".to_string()),
                    status: Some("publish".to_string()),
                    author_id: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        });
        let videos = block_on(collect_sitemap_videos(&db, 1)).unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].page_loc, "http://localhost:8787/video-post");
        assert!(videos[0]
            .content_loc
            .contains("youtube.com/embed/dQw4w9WgXcQ"));
        assert!(videos[0].thumbnail_loc.contains("/media/poster.png"));
        let xml = block_on(video_sitemap_xml(&db, 1)).unwrap();
        assert!(xml.contains("<video:video>"));
        assert!(xml.contains("<video:thumbnail_loc>"));
        assert!(!xml.contains("bare-video"));
    }

    #[test]
    fn index_all_lists_chunks_plus_image_and_video_sitemaps() {
        let db = SqliteDb::new();
        make_post(&db, "Hello", "hello");
        let xml = block_on(sitemap_index_all_xml(&db, 1)).unwrap();
        assert!(xml.contains("<sitemapindex"));
        assert!(xml.contains("/sitemap.xml?page=1"));
        assert!(xml.contains("/sitemap-images.xml"));
        assert!(xml.contains("/sitemap-videos.xml"));
        let overview = block_on(sitemap_overview(&db, 1)).unwrap();
        assert_eq!(overview.len(), 4); // index + 1 chunk + images + videos
        assert!(overview[0].0.ends_with("/sitemap-index.xml"));
    }
}
