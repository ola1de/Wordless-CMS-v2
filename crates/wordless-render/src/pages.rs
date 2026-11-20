//! Public rendering service (P2).
//!
//! Port of v1 `src/modules/render/service.ts` (public pages only —
//! the shop pages in that file belong to the commerce milestone and
//! are NOT ported here; see the report) plus the path resolver from
//! `src/modules/permalinks/service.ts` (`resolvePath`), which the
//! public handler needs.
//!
//! Pages render through the ACTIVE block theme when the coordinator
//! supplies it (`RenderOptions.theme`: files + stylesheet hash from
//! `wordless-themes` `loadActiveTheme`); otherwise the M2 built-in
//! layout renders — still linked to `/theme.css` when a theme is
//! supplied. Any theme-loading failure falls back silently to the
//! M2 layout in v1; here the coordinator decides (it passes
//! `theme: None` when loading failed), so a broken theme never takes
//! the public site down.
//!
//! SEO: v1 `finishPage` computes `buildPageSeo` for every page. That
//! lives in `wordless-seo`, which this crate does not depend on, so
//! the coordinator computes it and passes the result in
//! `RenderOptions.seo`. When absent, the page title/description fall
//! back to the values v1 passes into `buildPageSeo` and no SEO head
//! tags are emitted.
//!
//! Elementor (P3, closed): when a post carries `_elementor_data`
//! postmeta, `render_post_content` renders the Elementor layout
//! *instead of* the block content, falling back to the block
//! renderer only when there is no Elementor data or it renders
//! nothing (v1 rule).

use std::collections::BTreeMap;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wordless_db::options::get_option;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::converter::ConverterClient;
use wordless_kernel::error::{not_found, WordlessError};
use wordless_kernel::woo_client::WooClient;

use crate::blocks::{escape_html, plain_text_excerpt, render_content};
use crate::feeds::site_url;
use crate::templates::{
    render_template_file, resolve_template_name, themed_document, TemplateContext, TemplateKind,
    TemplateLink, TemplatePostData, TemplateTerm,
};

/// Elementor rendering is implemented (P3, `wordless-elementor` — in
/// the wordless-converter Worker since the 2026-11-20 split).
/// Kept so callers that recorded the gap can find the note.
pub const ELEMENTOR_GAP: &str = "Closed in P3: posts carrying _elementor_data postmeta are rendered from their Elementor layout by wordless-elementor (in the wordless-converter Worker since the 2026-11-20 split), falling back to block content when it renders nothing.";

/// Postmeta key Elementor stores its layout under (v1 `ELEMENTOR_META_KEY`).
pub const ELEMENTOR_META_KEY: &str = "_elementor_data";

/// A rendered public page.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedPage {
    pub status: u16,
    pub html: String,
}

/// One `wp_posts` row, as v1's `Post` (`posts/repo.ts` `rowToPost`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub id: i64,
    pub author_id: i64,
    pub date: String,
    pub date_gmt: String,
    pub content: String,
    pub title: String,
    pub excerpt: String,
    pub status: String,
    pub comment_status: String,
    pub ping_status: String,
    #[serde(default)]
    pub password: String,
    pub slug: String,
    pub modified: String,
    pub modified_gmt: String,
    #[serde(default)]
    pub content_filtered: String,
    pub parent_id: i64,
    #[serde(default)]
    pub guid: String,
    pub menu_order: i64,
    #[serde(rename = "type")]
    pub post_type: String,
    pub mime_type: String,
    pub comment_count: i64,
}

impl Post {
    /// `true` for a `post` row, `false` for pages/attachments/etc.
    pub fn is_post(&self) -> bool {
        self.post_type == "post"
    }
}

pub fn row_to_post(row: &Row) -> Post {
    Post {
        id: row_int(row, "ID"),
        author_id: row_int(row, "post_author"),
        date: row_text(row, "post_date"),
        date_gmt: row_text(row, "post_date_gmt"),
        content: row_text(row, "post_content"),
        title: row_text(row, "post_title"),
        excerpt: row_text(row, "post_excerpt"),
        status: row_text(row, "post_status"),
        comment_status: row_text(row, "comment_status"),
        ping_status: row_text(row, "ping_status"),
        password: row_text(row, "post_password"),
        slug: row_text(row, "post_name"),
        modified: row_text(row, "post_modified"),
        modified_gmt: row_text(row, "post_modified_gmt"),
        content_filtered: row_text(row, "post_content_filtered"),
        parent_id: row_int(row, "post_parent"),
        guid: row_text(row, "guid"),
        menu_order: row_int(row, "menu_order"),
        post_type: row_text(row, "post_type"),
        mime_type: row_text(row, "post_mime_type"),
        comment_count: row_int(row, "comment_count"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostListResult {
    pub posts: Vec<Post>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// One term (`wp_terms` + `wp_term_taxonomy`), as v1's `Term`.
#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub id: i64,
    pub term_taxonomy_id: i64,
    pub name: String,
    pub slug: String,
    pub taxonomy: String,
    pub description: String,
    pub parent_id: i64,
    pub count: i64,
}

pub fn row_to_term(row: &Row) -> Term {
    Term {
        id: row_int(row, "term_id"),
        term_taxonomy_id: row_int(row, "term_taxonomy_id"),
        name: row_text(row, "name"),
        slug: row_text(row, "slug"),
        taxonomy: row_text(row, "taxonomy"),
        description: row_text(row, "description"),
        parent_id: row_int(row, "parent"),
        count: row_int(row, "count"),
    }
}

/// One approved comment, threaded (`children` nested).
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: i64,
    pub post_id: i64,
    pub author: String,
    pub date: String,
    pub content: String,
    pub parent_id: i64,
    pub children: Vec<Comment>,
}

/// A primary-nav item (v1 `NavItem`: title + url).
#[derive(Debug, Clone, PartialEq)]
pub struct NavItem {
    pub title: String,
    pub url: String,
}

/// The active theme as loaded by `wordless-themes` (`loadActiveTheme`).
/// The coordinator supplies this; this crate never reads R2.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeView {
    /// `"block"` or `"classic"`.
    pub kind: String,
    /// Theme files: relative path → content.
    pub files: BTreeMap<String, String>,
    /// Stylesheet hash for the `/theme.css?v=<hash>` link.
    pub stylesheet_hash: String,
}

/// Precomputed page SEO, as returned by `wordless-seo`
/// `build_page_seo`: effective title/description (without the site
/// name suffix) plus the remaining head tags.
#[derive(Debug, Clone, PartialEq)]
pub struct PageSeoHead {
    pub title: String,
    pub description: String,
    pub head_tags: String,
    /// Visible breadcrumb navigation (P10), empty when the page has
    /// no trail beyond Home. `finish_page` places it at the top of
    /// `<main>` on the fallback layout path.
    pub breadcrumbs_html: String,
}

/// Options threaded through public rendering (M5 themes + preview).
#[derive(Clone, Default)]
pub struct RenderOptions {
    /// Active theme (files + stylesheet hash). `None` → M2 layout
    /// with the built-in inline CSS.
    pub theme: Option<ThemeView>,
    /// Precomputed SEO head (from `wordless-seo`). `None` → plain
    /// title/description, no SEO head tags.
    pub seo: Option<PageSeoHead>,
    /// Preview token (kept on the `/theme.css` URL so preview CSS loads).
    pub preview_token: Option<String>,
    /// Split (2026-11-20): the converter Worker client, for Elementor
    /// rendering. `None` → Elementor content falls back to block
    /// content (public pages never 500 on a missing converter).
    pub converter: Option<Rc<dyn ConverterClient>>,
    /// Size remedy (2026-11-20): the WooCommerce Worker client, for
    /// the storefront (variable-product detail, shipping zones).
    /// `None` → the plain add-to-cart form / no shipping-method
    /// selector (public pages never 500 on a missing Woo Worker).
    pub woo_client: Option<Rc<dyn WooClient>>,
}

impl std::fmt::Debug for RenderOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderOptions")
            .field("theme", &self.theme)
            .field("seo", &self.seo)
            .field("preview_token", &self.preview_token)
            .field(
                "converter",
                &self.converter.as_ref().map(|_| "ConverterClient"),
            )
            .field("woo_client", &self.woo_client.as_ref().map(|_| "WooClient"))
            .finish()
    }
}

// ------------------------------------------------------------ permalinks

/// Permalink structure (`permalink_structure` option, default `postname`).
pub async fn get_permalink_structure(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let raw: String = wordless_multisite::get_site_option(db, site_id, "permalink_structure", "postname".to_string())
        .await
        .map_err(WordlessError::from)?;
    match raw.as_str() {
        "plain" | "postname" | "day-name" | "month-name" => Ok(raw),
        _ => Ok("postname".to_string()),
    }
}

fn date_parts(iso: &str) -> (String, String, String) {
    (
        iso.get(0..4).unwrap_or("").to_string(),
        iso.get(5..7).unwrap_or("").to_string(),
        iso.get(8..10).unwrap_or("").to_string(),
    )
}

/// Public URL path for a post or page under the given structure.
/// Port of v1 `postUrl`.
pub fn post_url_with_structure(post: &Post, structure: &str) -> String {
    if post.post_type == "page" {
        return format!("/{}", post.slug);
    }
    if structure == "plain" {
        return format!("/?p={}", post.id);
    }
    let (year, month, day) = date_parts(if post.date.is_empty() {
        &post.date_gmt
    } else {
        &post.date
    });
    if structure == "day-name" {
        return format!("/{year}/{month}/{day}/{}", post.slug);
    }
    if structure == "month-name" {
        return format!("/{year}/{month}/{}", post.slug);
    }
    format!("/{}", post.slug)
}

pub async fn post_url(db: &dyn Db, site_id: i64, post: &Post) -> Result<String, WordlessError> {
    let structure = get_permalink_structure(db, site_id).await?;
    Ok(post_url_with_structure(post, &structure))
}

/// Public URL path for a term. Port of v1 `termUrl`.
pub fn term_url(taxonomy: &str, slug: &str) -> String {
    if taxonomy == "category" {
        format!("/category/{slug}")
    } else if taxonomy == "tag" {
        format!("/tag/{slug}")
    } else {
        format!("/{taxonomy}/{slug}")
    }
}

// ------------------------------------------------------------ post queries

pub async fn get_post_by_id(db: &dyn Db, site_id: i64, id: i64) -> Result<Option<Post>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wp_posts WHERE ID = ? AND site_id = ? LIMIT 1")
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(row.as_ref().map(row_to_post))
}

/// Public lookup: trashed posts are never returned by slug (v1 `getPostBySlug`).
pub async fn get_post_by_slug(db: &dyn Db, site_id: i64, slug: &str) -> Result<Option<Post>, WordlessError> {
    let row: Option<Row> = db
        .prepare(
            "SELECT * FROM wp_posts WHERE post_name = ? AND post_type IN ('post', 'page') AND site_id = ? LIMIT 1",
        )
        .bind(vec![Param::Text(slug.to_string()), Param::Integer(site_id)])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(row
        .as_ref()
        .map(row_to_post)
        .filter(|p| p.status != "trash"))
}

pub async fn list_posts(
    db: &dyn Db,
    site_id: i64,
    status: &str,
    post_type: &str,
    page: i64,
    per_page: i64,
) -> Result<PostListResult, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let total_row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS total FROM wp_posts WHERE site_id = ? AND post_status = ? AND post_type = ?")
        .bind(vec![
            Param::Integer(site_id),
            Param::Text(status.to_string()),
            Param::Text(post_type.to_string()),
        ])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    let total = total_row.as_ref().map(|r| row_int(r, "total")).unwrap_or(0);
    let total_pages = if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    };
    let rows: Vec<Row> = db
        .prepare(
            "SELECT * FROM wp_posts WHERE site_id = ? AND post_status = ? AND post_type = ? \
             ORDER BY post_date DESC, ID DESC LIMIT ? OFFSET ?",
        )
        .bind(vec![
            Param::Integer(site_id),
            Param::Text(status.to_string()),
            Param::Text(post_type.to_string()),
            Param::Integer(per_page),
            Param::Integer((page - 1) * per_page),
        ])
        .all::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(PostListResult {
        posts: rows.iter().map(row_to_post).collect(),
        total,
        page,
        per_page,
        total_pages,
    })
}

pub async fn list_posts_by_term(
    db: &dyn Db,
    site_id: i64,
    term_taxonomy_id: i64,
    page: i64,
    per_page: i64,
) -> Result<PostListResult, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let total_row: Option<Row> = db
        .prepare(
            "SELECT COUNT(*) AS total FROM wp_posts p JOIN wp_term_relationships tr ON tr.object_id = p.ID \
             WHERE tr.term_taxonomy_id = ? AND p.site_id = ? AND p.post_status = 'publish'",
        )
        .bind(vec![Param::Integer(term_taxonomy_id), Param::Integer(site_id)])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    let total = total_row.as_ref().map(|r| row_int(r, "total")).unwrap_or(0);
    let total_pages = if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    };
    let rows: Vec<Row> = db
        .prepare(
            "SELECT p.* FROM wp_posts p JOIN wp_term_relationships tr ON tr.object_id = p.ID \
             WHERE tr.term_taxonomy_id = ? AND p.site_id = ? AND p.post_status = 'publish' \
             ORDER BY p.post_date DESC, p.ID DESC LIMIT ? OFFSET ?",
        )
        .bind(vec![
            Param::Integer(term_taxonomy_id),
            Param::Integer(site_id),
            Param::Integer(per_page),
            Param::Integer((page - 1) * per_page),
        ])
        .all::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(PostListResult {
        posts: rows.iter().map(row_to_post).collect(),
        total,
        page,
        per_page,
        total_pages,
    })
}

/// Posts whose `post_date` starts with `prefix` (`YYYY` or `YYYY-MM`).
/// Port of v1 `listPostsByDatePrefix` (post type `post` only).
pub async fn list_posts_by_date_prefix(
    db: &dyn Db,
    site_id: i64,
    prefix: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Post>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare(
            "SELECT * FROM wp_posts WHERE site_id = ? AND post_type = 'post' AND post_status = 'publish' AND post_date LIKE ? \
             ORDER BY post_date DESC, ID DESC LIMIT ? OFFSET ?",
        )
        .bind(vec![
            Param::Integer(site_id),
            Param::Text(format!("{prefix}%")),
            Param::Integer(limit),
            Param::Integer(offset),
        ])
        .all::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(rows.iter().map(row_to_post).collect())
}

fn escape_like(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    for c in q.chars() {
        if c == '\\' || c == '%' || c == '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Search published posts/pages. Port of v1 `searchPosts` (title
/// matches first, then date; `q` must be 1–200 chars).
pub async fn search_posts(
    db: &dyn Db,
    site_id: i64,
    query: &str,
    page: i64,
    per_page: i64,
) -> Result<PostListResult, WordlessError> {
    let q = query.trim();
    if q.is_empty() || q.chars().count() > 200 {
        return Err(WordlessError::new(
            "bad_request",
            400,
            "Query parameter \"q\" is required and must be 1–200 characters.",
        ));
    }
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let like = format!("%{}%", escape_like(q));
    let where_sql = "WHERE site_id = ? AND post_status = 'publish' AND post_type IN ('post', 'page') \
        AND (post_title LIKE ? ESCAPE '\\' OR post_content LIKE ? ESCAPE '\\')";
    let total_row: Option<Row> = db
        .prepare(&format!(
            "SELECT COUNT(*) AS total FROM wp_posts {where_sql}"
        ))
        .bind(vec![Param::Integer(site_id), Param::Text(like.clone()), Param::Text(like.clone())])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    let total = total_row.as_ref().map(|r| row_int(r, "total")).unwrap_or(0);
    let total_pages = if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    };
    let rows: Vec<Row> = db
        .prepare(&format!(
            "SELECT * FROM wp_posts {where_sql} \
             ORDER BY CASE WHEN post_title LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END, \
             post_date DESC, ID DESC LIMIT ? OFFSET ?"
        ))
        .bind(vec![
            Param::Integer(site_id),
            Param::Text(like.clone()),
            Param::Text(like.clone()),
            Param::Text(like),
            Param::Integer(per_page),
            Param::Integer((page - 1) * per_page),
        ])
        .all::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(PostListResult {
        posts: rows.iter().map(row_to_post).collect(),
        total,
        page,
        per_page,
        total_pages,
    })
}

pub async fn get_term_by_taxonomy_id(db: &dyn Db, site_id: i64, id: i64) -> Result<Option<Term>, WordlessError> {
    let row: Option<Row> = db
        .prepare(
            "SELECT t.term_id, tt.term_taxonomy_id, t.name, t.slug, tt.taxonomy, tt.description, tt.parent, tt.count \
             FROM wp_terms t JOIN wp_term_taxonomy tt ON tt.term_id = t.term_id \
             WHERE tt.term_taxonomy_id = ? AND tt.site_id = ? LIMIT 1",
        )
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(row.as_ref().map(row_to_term))
}

pub async fn get_term_by_slug(
    db: &dyn Db,
    site_id: i64,
    taxonomy: &str,
    slug: &str,
) -> Result<Option<Term>, WordlessError> {
    let row: Option<Row> = db
        .prepare(
            "SELECT t.term_id, tt.term_taxonomy_id, t.name, t.slug, tt.taxonomy, tt.description, tt.parent, tt.count \
             FROM wp_terms t JOIN wp_term_taxonomy tt ON tt.term_id = t.term_id \
             WHERE tt.taxonomy = ? AND t.slug = ? AND tt.site_id = ? LIMIT 1",
        )
        .bind(vec![Param::Text(taxonomy.to_string()), Param::Text(slug.to_string()), Param::Integer(site_id)])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(row.as_ref().map(row_to_term))
}

/// Terms assigned to a post (v1 `getPostTerms` / `findPostTerms`).
pub async fn get_post_terms(db: &dyn Db, site_id: i64, post_id: i64) -> Result<Vec<Term>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare(
            "SELECT t.term_id, tt.term_taxonomy_id, t.name, t.slug, tt.taxonomy, tt.description, tt.parent, tt.count \
             FROM wp_terms t JOIN wp_term_taxonomy tt ON tt.term_id = t.term_id \
             JOIN wp_term_relationships tr ON tr.term_taxonomy_id = tt.term_taxonomy_id \
             WHERE tr.object_id = ? AND tr.site_id = ? ORDER BY t.name ASC",
        )
        .bind(vec![Param::Integer(post_id), Param::Integer(site_id)])
        .all::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(rows.iter().map(row_to_term).collect())
}

// ------------------------------------------------------------ path resolving

/// What a public path resolves to. Port of v1 `ResolvedPath`
/// (`permalinks/service.ts` `resolvePath`).
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedPath {
    /// 301 to this target (a redirect target string, e.g. `/new-slug`).
    Redirect(String),
    Home,
    Post(Post),
    Page(Post),
    Term(Term),
    DateArchive {
        year: i64,
        month: Option<i64>,
    },
    NotFound,
}

pub async fn find_redirect_by_path(
    db: &dyn Db,
    site_id: i64,
    path: &str,
) -> Result<Option<String>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT new_path FROM wl_redirects WHERE old_path = ? AND site_id = ? LIMIT 1")
        .bind(vec![Param::Text(path.to_string()), Param::Integer(site_id)])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(row.map(|r| row_text(&r, "new_path")))
}

pub async fn find_redirect_by_slug(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
) -> Result<Option<String>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT new_path FROM wl_redirects WHERE old_path LIKE ? AND site_id = ? LIMIT 1")
        .bind(vec![Param::Text(format!("%/{slug}")), Param::Integer(site_id)])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(row.map(|r| row_text(&r, "new_path")))
}

fn normalize_path(raw: &str) -> String {
    // v1: `decodeURIComponent(path.replace(/^\/+|\/+$/g, ""))`.
    let trimmed = raw.trim_matches('/');
    percent_decode(trimmed)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(n) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(n);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_digits(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit())
}

/// Resolve a public path. Resolution order (v1 `resolvePath`):
/// exact redirect → home → term archive → date archive → dated
/// post/page → plain slug → redirect by trailing slug → not-found.
pub async fn resolve_path(db: &dyn Db, site_id: i64, raw_path: &str) -> Result<ResolvedPath, WordlessError> {
    let path = normalize_path(raw_path);
    if let Some(target) = find_redirect_by_path(db, site_id, &format!("/{path}")).await? {
        return Ok(ResolvedPath::Redirect(target));
    }
    if path.is_empty() {
        return Ok(ResolvedPath::Home);
    }
    let segments: Vec<&str> = path.split('/').collect();
    let first = segments.first().copied().unwrap_or("");
    // Term archives: /category/slug, /tag/slug.
    if (first == "category" || first == "tag") && segments.len() == 2 {
        return match get_term_by_slug(db, site_id, first, segments[1]).await? {
            Some(term) => Ok(ResolvedPath::Term(term)),
            None => Ok(ResolvedPath::NotFound),
        };
    }
    // Date archives: /YYYY/ and /YYYY/MM/.
    if is_digits(first, 4) && segments.len() <= 2 {
        let year: i64 = first.parse().unwrap_or(0);
        let month = if segments.len() == 2 && is_digits(segments[1], 2) {
            Some(segments[1].parse::<i64>().unwrap_or(0))
        } else {
            None
        };
        if month.map_or(true, |m| (1..=12).contains(&m)) {
            return Ok(ResolvedPath::DateArchive { year, month });
        }
        return Ok(ResolvedPath::NotFound);
    }
    // Dated post: /YYYY/MM/DD/slug or /YYYY/MM/slug.
    let slug = segments.last().copied().unwrap_or("");
    if segments.len() == 3 || segments.len() == 4 {
        let dated = if segments.len() == 4 {
            is_digits(segments[0], 4) && is_digits(segments[1], 2) && is_digits(segments[2], 2)
        } else {
            is_digits(segments[0], 4) && is_digits(segments[1], 2)
        };
        if dated {
            if let Some(post) = get_post_by_slug(db, site_id, slug).await? {
                if post.status == "publish" {
                    let (year, month, day) = date_parts(if post.date.is_empty() {
                        &post.date_gmt
                    } else {
                        &post.date
                    });
                    let matches = if segments.len() == 4 {
                        segments[0] == year && segments[1] == month && segments[2] == day
                    } else {
                        segments[0] == year && segments[1] == month
                    };
                    if matches {
                        return Ok(if post.post_type == "page" {
                            ResolvedPath::Page(post)
                        } else {
                            ResolvedPath::Post(post)
                        });
                    }
                }
            }
            return Ok(ResolvedPath::NotFound);
        }
    }
    // Plain slug: /hello-world (post or page).
    if segments.len() == 1 {
        if let Some(post) = get_post_by_slug(db, site_id, slug).await? {
            if post.status == "publish" {
                return Ok(if post.post_type == "page" {
                    ResolvedPath::Page(post)
                } else {
                    ResolvedPath::Post(post)
                });
            }
        }
        if let Some(target) = find_redirect_by_slug(db, site_id, slug).await? {
            return Ok(ResolvedPath::Redirect(target));
        }
        return Ok(ResolvedPath::NotFound);
    }
    Ok(ResolvedPath::NotFound)
}

/// Resolve `/?p=ID` (plain structure) to a published post or page.
pub async fn resolve_post_by_id(db: &dyn Db, site_id: i64, id: i64) -> Result<Option<Post>, WordlessError> {
    let post = get_post_by_id(db, site_id, id).await?;
    Ok(post.filter(|p| p.status == "publish" && (p.post_type == "post" || p.post_type == "page")))
}

// ------------------------------------------------------------ site / nav

pub(crate) async fn site_info(db: &dyn Db, site_id: i64) -> Result<(String, String), WordlessError> {
    let name: String = wordless_multisite::get_site_option(db, site_id, "blogname", "Wordless".to_string())
        .await
        .map_err(WordlessError::from)?;
    let description: String = wordless_multisite::get_site_option(db, site_id, "blogdescription", String::new())
        .await
        .map_err(WordlessError::from)?;
    Ok((name, description))
}

/// Posts-per-page for public list rendering (clamped 1–100, default 10).
pub async fn get_posts_per_page(db: &dyn Db, site_id: i64) -> Result<i64, WordlessError> {
    let raw: Value = wordless_multisite::get_site_option(db, site_id, "posts_per_page", Value::from(10))
        .await
        .map_err(WordlessError::from)?;
    let n = raw
        .as_i64()
        .unwrap_or_else(|| raw.as_f64().map(|f| f as i64).unwrap_or(10));
    Ok(n.clamp(1, 100))
}

pub(crate) async fn nav_items(db: &dyn Db, site_id: i64) -> Result<Vec<NavItem>, WordlessError> {
    // v1: menu assigned to location "primary" (option
    // `nav_menu_locations`, JSON map location -> term_taxonomy_id);
    // items are `nav_menu_item` posts linked by term relationship,
    // nested by parent, then flattened [item, ...children].
    let locations: Value = wordless_multisite::get_site_option(db, site_id, "nav_menu_locations", Value::Object(Default::default()))
        .await
        .map_err(WordlessError::from)?;
    let Some(menu_id) = locations.get("primary").and_then(|v| v.as_i64()) else {
        return Ok(Vec::new());
    };
    let term = match get_term_by_taxonomy_id(db, site_id, menu_id).await? {
        Some(t) if t.taxonomy == "nav_menu" => t,
        _ => return Ok(Vec::new()),
    };
    let _ = term;
    let rows: Vec<Row> = db
        .prepare(
            "SELECT p.ID, p.post_title, p.post_parent, p.menu_order FROM wp_posts p \
             JOIN wp_term_relationships tr ON tr.object_id = p.ID \
             WHERE tr.term_taxonomy_id = ? AND p.post_type = 'nav_menu_item' \
             ORDER BY p.menu_order ASC, p.ID ASC",
        )
        .bind(vec![Param::Integer(menu_id)])
        .all::<Row>()
        .await
        .map_err(WordlessError::from)?;
    #[derive(Debug, Clone)]
    struct RawItem {
        id: i64,
        title: String,
        parent_id: i64,
    }
    let mut raw: Vec<RawItem> = Vec::new();
    for row in &rows {
        raw.push(RawItem {
            id: row_int(row, "ID"),
            title: row_text(row, "post_title"),
            parent_id: row_int(row, "post_parent"),
        });
    }
    // Resolve each item's URL from its postmeta.
    let mut items: Vec<(NavItem, i64, i64)> = Vec::new(); // (item, id, parent_id)
    for r in &raw {
        let meta_rows: Vec<Row> = db
            .prepare("SELECT meta_key, meta_value FROM wp_postmeta WHERE post_id = ?")
            .bind(vec![Param::Integer(r.id)])
            .all::<Row>()
            .await
            .map_err(WordlessError::from)?;
        let mut meta: std::collections::HashMap<String, String> = Default::default();
        for m in &meta_rows {
            meta.insert(row_text(m, "meta_key"), row_text(m, "meta_value"));
        }
        let item_type = meta
            .get("_menu_item_type")
            .map(|s| s.as_str())
            .unwrap_or("custom");
        let url = if item_type == "custom" {
            meta.get("_menu_item_url")
                .cloned()
                .unwrap_or_else(|| "#".to_string())
        } else {
            let object_id: i64 = meta
                .get("_menu_item_object_id")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if object_id == 0 {
                "#".to_string()
            } else if item_type == "post" || item_type == "page" {
                match get_post_by_id(db, site_id, object_id).await? {
                    Some(post) => post_url(db, site_id, &post).await?,
                    None => "#".to_string(),
                }
            } else if item_type == "term" {
                match get_term_by_taxonomy_id(db, site_id, object_id).await? {
                    Some(term) => term_url(&term.taxonomy, &term.slug),
                    None => "#".to_string(),
                }
            } else {
                meta.get("_menu_item_url")
                    .cloned()
                    .unwrap_or_else(|| "#".to_string())
            }
        };
        items.push((
            NavItem {
                title: r.title.clone(),
                url,
            },
            r.id,
            r.parent_id,
        ));
    }
    // Nest, then flatten [item, ...children] like v1's navItems
    // (one level: grandchildren are dropped by v1's flatMap too —
    // it maps each item to {title, url} only).
    let mut ordered: Vec<NavItem> = Vec::new();
    let mut emitted: std::collections::HashSet<i64> = Default::default();
    for (item, id, parent_id) in &items {
        if *parent_id != 0 {
            continue; // emitted with its parent below
        }
        ordered.push(item.clone());
        emitted.insert(*id);
        for (child, cid, cparent) in &items {
            if *cparent == *id {
                ordered.push(child.clone());
                emitted.insert(*cid);
            }
        }
    }
    for (item, id, _) in &items {
        if !emitted.contains(id) {
            ordered.push(item.clone());
        }
    }
    Ok(ordered)
}

// ------------------------------------------------------------ rendering bits

/// `YYYY-MM-DD` for a stored ISO date; invalid dates pass through
/// as their first 10 chars (v1 `formatDate`).
pub fn format_date(iso: &str) -> String {
    if iso.is_empty() {
        return String::new();
    }
    iso.chars().take(10).collect()
}

pub(crate) fn pagination(current: i64, total_pages: i64, base: &str, query: &str) -> String {
    if total_pages <= 1 {
        return String::new();
    }
    let sep = if base.contains('?') { "&" } else { "?" };
    let prev = if current > 1 {
        format!(
            "<a href=\"{}\">&larr; Newer</a>",
            escape_html(&format!("{base}{sep}page={}{query}", current - 1))
        )
    } else {
        "<span></span>".to_string()
    };
    let next = if current < total_pages {
        format!(
            "<a href=\"{}\">Older &rarr;</a>",
            escape_html(&format!("{base}{sep}page={}{query}", current + 1))
        )
    } else {
        "<span></span>".to_string()
    };
    format!("<nav class=\"pagination\" aria-label=\"Pagination\">{prev}{next}</nav>")
}

/// `encodeURIComponent` (JS): percent-encode everything except
/// `A-Z a-z 0-9 - _ . ! ~ * ' ( )`.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
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
            | b')' => out.push(*b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Share links for a page (v1 `shareLinks`, integrations/social.ts).
pub fn share_links(page_url: &str, title: &str) -> Vec<(String, String)> {
    let url = encode_uri_component(page_url);
    let text = encode_uri_component(title);
    vec![
        (
            "X".to_string(),
            format!("https://twitter.com/intent/tweet?text={text}&url={url}"),
        ),
        (
            "Facebook".to_string(),
            format!("https://www.facebook.com/sharer/sharer.php?u={url}"),
        ),
        (
            "WhatsApp".to_string(),
            format!("https://wa.me/?text={text}%20{url}"),
        ),
        (
            "Telegram".to_string(),
            format!("https://t.me/share/url?url={url}&text={text}"),
        ),
    ]
}

/// Analytics snippet, or `None` when unconfigured (v1
/// `buildAnalyticsSnippet`: option `wl_integration_analytics`,
/// `{provider: "ga4"|"plausible", measurementId}`).
pub async fn build_analytics_snippet(db: &dyn Db) -> Result<Option<String>, WordlessError> {
    let config: Value = get_option(
        db,
        "wl_integration_analytics",
        Value::Object(Default::default()),
    )
    .await
    .map_err(WordlessError::from)?;
    let provider = config
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("none");
    let measurement_id = config
        .get("measurementId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if provider == "none" || measurement_id.is_empty() {
        return Ok(None);
    }
    let id = escape_html(&measurement_id);
    if provider == "ga4" {
        Ok(Some(format!(
            "<script async src=\"https://www.googletagmanager.com/gtag/js?id={id}\"></script>\n<script>window.dataLayer=window.dataLayer||[];function gtag(){{dataLayer.push(arguments);}}gtag('js',new Date());gtag('config','{id}');</script>"
        )))
    } else if provider == "plausible" {
        Ok(Some(format!(
            "<script defer data-domain=\"{id}\" src=\"https://plausible.io/js/script.js\"></script>"
        )))
    } else {
        Ok(None)
    }
}

/// `true` when the post carries Elementor data. P3 renders it in
/// [`render_post_content`]; this helper is for callers that only
/// need to know whether Elementor data exists.
pub async fn has_elementor_data(db: &dyn Db, site_id: i64, post_id: i64) -> Result<bool, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT meta_value FROM wp_postmeta WHERE post_id = ? AND meta_key = ? AND site_id = ? LIMIT 1")
        .bind(vec![
            Param::Integer(post_id),
            Param::Text(ELEMENTOR_META_KEY.to_string()),
            Param::Integer(site_id),
        ])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    Ok(match row {
        Some(r) => {
            let raw = row_text(&r, "meta_value");
            if raw.trim().is_empty() {
                return Ok(false);
            }
            match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Array(a)) => !a.is_empty(),
                _ => false,
            }
        }
        None => false,
    })
}

/// Render a post's body HTML. Elementor first (P3): when the post
/// carries `_elementor_data` and it renders non-empty HTML, that
/// layout is used instead of the block content; otherwise the normal
/// block renderer runs (v1 rule).
///
/// Split (2026-11-20): the Elementor rendering runs in the
/// wordless-converter Worker. When no converter is configured, or the
/// converter call fails, the post falls back to block content — a
/// public page never 500s on a missing converter.
pub async fn render_post_content(
    db: &dyn Db,
    site_id: i64,
    post: &Post,
    converter: Option<&dyn ConverterClient>,
) -> Result<String, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT meta_value FROM wp_postmeta WHERE post_id = ? AND meta_key = ? AND site_id = ? LIMIT 1")
        .bind(vec![
            Param::Integer(post.id),
            Param::Text(ELEMENTOR_META_KEY.to_string()),
            Param::Integer(site_id),
        ])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)?;
    if let Some(row) = row {
        let raw = row_text(&row, "meta_value");
        if !raw.trim().is_empty() {
            if let Ok(data) = serde_json::from_str::<Value>(&raw) {
                if let Some(converter) = converter {
                    if let Ok(html) = converter.render_elementor(&data).await {
                        if !html.trim().is_empty() {
                            return Ok(html);
                        }
                    }
                }
            }
        }
    }
    Ok(render_content(&post.content))
}

async fn to_template_post(
    db: &dyn Db,
    site_id: i64,
    post: &Post,
    include_terms: bool,
    converter: Option<&dyn ConverterClient>,
) -> Result<TemplatePostData, WordlessError> {
    let url = post_url(db, site_id, post).await?;
    let mut terms: Vec<TemplateTerm> = Vec::new();
    if include_terms {
        for term in get_post_terms(db, site_id, post.id).await? {
            let base = if term.taxonomy == "category" {
                "/category".to_string()
            } else if term.taxonomy == "tag" {
                "/tag".to_string()
            } else {
                format!("/{}", term.taxonomy)
            };
            terms.push(TemplateTerm {
                name: term.name,
                url: format!("{base}/{}", term.slug),
                taxonomy: term.taxonomy,
            });
        }
    }
    Ok(TemplatePostData {
        title: post.title.clone(),
        date: post.date.clone(),
        excerpt: post.excerpt.clone(),
        content_html: render_post_content(db, site_id, post, converter).await?,
        url,
        comment_count: post.comment_count,
        terms,
    })
}

// ------------------------------------------------------------ M2 layout

/// The built-in default theme layout (v1 `src/modules/render/theme.ts`
/// `layout`). Clean semantic HTML + minimal inline CSS. All
/// interpolated text is escaped here; renderers pass raw strings.
pub fn layout(
    title: &str,
    site_name: &str,
    description: &str,
    nav: &[NavItem],
    body: &str,
    head_extra: Option<&str>,
    stylesheet_href: Option<&str>,
) -> String {
    let nav_html = if nav.is_empty() {
        String::new()
    } else {
        let items = nav
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
        format!("<nav aria-label=\"Primary\"><ul>{items}</ul></nav>")
    };
    let style = match stylesheet_href {
        Some(href) => format!("<link rel=\"stylesheet\" href=\"{}\">", escape_html(href)),
        None => format!("<style>{DEFAULT_CSS}</style>"),
    };
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{} — {}</title>\n<meta name=\"description\" content=\"{}\">\n{}\n{}\n</head>\n<body>\n<header class=\"site\">\n  <a class=\"brand\" href=\"/\">{}</a>\n  {}\n  {}\n</header>\n<main>\n{}\n</main>\n<footer class=\"site\">Powered by <a href=\"/admin\">Wordless</a></footer>\n</body>\n</html>",
        escape_html(title),
        escape_html(site_name),
        escape_html(description),
        head_extra.unwrap_or(""),
        style,
        escape_html(site_name),
        if description.is_empty() {
            String::new()
        } else {
            format!("<p class=\"tagline\">{}</p>", escape_html(description))
        },
        nav_html,
        body
    )
}

/// The inline CSS from v1 `theme.ts` `layout` (used when no theme
/// stylesheet is linked). Verbatim, including the leading newline
/// and two-space indent of the original template literal.
pub const DEFAULT_CSS: &str = r#"
  :root { color-scheme: light; }
  body { font-family: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; margin: 0; color: #122019; background: #fff; line-height: 1.65; }
  a { color: #0b7c74; }
  header.site { position: sticky; top: 0; z-index: 20; background: rgba(255,255,255,0.88); backdrop-filter: blur(10px); border-bottom: 1px solid #e6edeb; padding: 16px 24px; }
  header.site a.brand { font-size: 22px; font-weight: 800; letter-spacing: -0.01em; color: inherit; text-decoration: none; }
  header.site p.tagline { margin: 2px 0 0; color: #64748b; font-size: 14px; }
  nav ul { list-style: none; margin: 12px 0 0; padding: 0; display: flex; gap: 6px; flex-wrap: wrap; }
  nav a { color: #122019; text-decoration: none; font-weight: 600; font-size: 15px; padding: 6px 12px; border-radius: 999px; }
  nav a:hover { background: #f0f5f3; }
  main { max-width: 720px; margin: 0 auto; padding: 32px 24px; }
  article h1 { font-size: 34px; line-height: 1.2; margin: 0 0 8px; letter-spacing: -0.02em; }
  article .meta { color: #64748b; font-size: 14px; margin-bottom: 20px; }
  article img { max-width: 100%; height: auto; border-radius: 12px; }
  article blockquote { border-left: 3px solid #0d9488; background: #f4f8f7; border-radius: 0 12px 12px 0; margin: 16px 0; padding: 16px 20px; color: #3d4f48; }
  article pre { background: #061a1e; color: #d7ece7; padding: 14px; border-radius: 10px; overflow-x: auto; }
  article code { background: #f0f5f3; padding: 1px 5px; border-radius: 4px; }
  .post-list { list-style: none; padding: 0; display: flex; flex-direction: column; gap: 16px; }
  nav[aria-label="Breadcrumb"] ol { list-style: none; margin: 0 0 20px; padding: 0; display: flex; gap: 6px; flex-wrap: wrap; font-size: 14px; color: #64748b; }
  nav[aria-label="Breadcrumb"] li + li::before { content: "/"; margin-right: 6px; color: #94a3b8; }
  nav[aria-label="Breadcrumb"] a { color: #0b7c74; text-decoration: none; }
  .post-list li { padding: 24px; border: 1px solid #e6edeb; border-radius: 16px; box-shadow: 0 1px 2px rgba(6,26,30,0.05); }
  .post-list h2 { margin: 0 0 6px; font-size: 24px; line-height: 1.3; letter-spacing: -0.01em; }
  .post-list h2 a { color: inherit; text-decoration: none; }
  .post-list h2 a:hover { color: #0b7c74; }
  .post-list .meta { color: #64748b; font-size: 14px; margin-bottom: 8px; }
  .post-card { padding: 24px; border: 1px solid #e6edeb; border-radius: 16px; background: #fff; box-shadow: 0 1px 2px rgba(6,26,30,0.05); margin-bottom: 16px; }
  .post-card h2 { margin: 0 0 6px; font-size: 24px; line-height: 1.3; letter-spacing: -0.01em; }
  .post-card h2 a { color: inherit; text-decoration: none; }
  .post-card h2 a:hover { color: #0b7c74; }
  .post-card time { display: block; color: #64748b; font-size: 14px; margin-bottom: 8px; }
  .product-list { list-style: none; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 16px; }
  .product-list li { padding: 24px; border: 1px solid #e6edeb; border-radius: 16px; box-shadow: 0 1px 2px rgba(6,26,30,0.05); }
  .product-list h2 { margin: 0 0 6px; font-size: 20px; line-height: 1.3; }
  .product-list h2 a { color: inherit; text-decoration: none; }
  .product-list h2 a:hover { color: #0b7c74; }
  .product-list .price { font-weight: 700; font-size: 18px; margin: 0 0 8px; }
  .pagination { margin-top: 32px; display: flex; gap: 12px; }
  .pagination a { padding: 8px 16px; border: 1px solid #dde7e3; border-radius: 999px; text-decoration: none; font-weight: 600; font-size: 15px; }
  .comments { margin-top: 48px; border-top: 1px solid #e6edeb; padding-top: 24px; }
  .comments ol { list-style: none; padding: 0; }
  .comments li { padding: 16px 0; border-bottom: 1px solid #f0f5f3; }
  .comments .comment-meta { color: #64748b; font-size: 13px; }
  form.comment-form label { display: block; margin: 12px 0 6px; font-weight: 600; }
  form.comment-form input, form.comment-form textarea { width: 100%; box-sizing: border-box; padding: 10px 12px; border: 1px solid #c3d2cc; border-radius: 10px; font: inherit; }
  form.comment-form input:focus, form.comment-form textarea:focus { outline: none; border-color: #0d9488; box-shadow: 0 0 0 3px rgba(13,148,136,0.22); }
  form.comment-form button { margin-top: 16px; padding: 10px 20px; background: #0d9488; color: #fff; border: 0; border-radius: 999px; font-weight: 600; cursor: pointer; }
  footer.site { border-top: 1px solid #e6edeb; padding: 24px; color: #64748b; font-size: 14px; text-align: center; }
"#;

// ------------------------------------------------------------ finish_page

pub(crate) struct FallbackPage {
    pub title: String,
    pub site_name: String,
    pub description: String,
    pub nav: Vec<NavItem>,
    pub body: String,
    pub head_extra: Option<String>,
    pub status: u16,
}

pub(crate) async fn finish_page(
    db: &dyn Db,
    opts: &RenderOptions,
    ctx: TemplateContext,
    fallback: FallbackPage,
    extra_body: Option<String>,
) -> Result<RenderedPage, WordlessError> {
    let status = fallback.status;
    let analytics = build_analytics_snippet(db).await?;
    // Effective title/description: the coordinator-supplied SEO
    // values replace the fallback values (v1 `buildPageSeo`).
    let (title, description, seo_head) = match &opts.seo {
        Some(seo) => (
            seo.title.clone(),
            seo.description.clone(),
            seo.head_tags.clone(),
        ),
        None => (
            fallback.title.clone(),
            fallback.description.clone(),
            String::new(),
        ),
    };
    let fallback_head = {
        let mut parts: Vec<String> = Vec::new();
        if let Some(h) = &fallback.head_extra {
            if !h.is_empty() {
                parts.push(h.clone());
            }
        }
        if !seo_head.is_empty() {
            parts.push(seo_head.clone());
        }
        if let Some(a) = &analytics {
            parts.push(a.clone());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    };
    // P10: visible breadcrumbs go at the top of the page body on both
    // render paths — the fallback layout puts them at the top of
    // <main>, and the block-theme path puts them at the top of
    // <body> before the template output (the theme's own navigation
    // is untouched; a theme that renders its own breadcrumbs shows
    // both, which is the honest default until P11's builder owns it).
    let body_with_crumbs = match &opts.seo {
        Some(seo) if !seo.breadcrumbs_html.is_empty() => {
            format!("{}{}", seo.breadcrumbs_html, fallback.body)
        }
        _ => fallback.body.clone(),
    };
    if let Some(theme) = &opts.theme {
        let mut href = format!("/theme.css?v={}", theme.stylesheet_hash);
        if let Some(token) = &opts.preview_token {
            href.push_str(&format!("&wl_preview={}", encode_uri_component(token)));
        }
        if theme.kind == "block" {
            let available: std::collections::HashSet<String> = theme
                .files
                .keys()
                .filter(|k| k.starts_with("templates/"))
                .map(|k| k["templates/".len()..].to_string())
                .collect();
            if let Some(name) = resolve_template_name(ctx.kind, &available) {
                let rendered = render_template_file(&name, &theme.files, &ctx);
                let themed_body = match &opts.seo {
                    Some(seo) if !seo.breadcrumbs_html.is_empty() => {
                        format!("{}{}", seo.breadcrumbs_html, rendered.html)
                    }
                    _ => rendered.html.clone(),
                };
                let head_extra = {
                    let mut parts: Vec<String> = Vec::new();
                    if !seo_head.is_empty() {
                        parts.push(seo_head.clone());
                    }
                    if let Some(a) = &analytics {
                        parts.push(a.clone());
                    }
                    if parts.is_empty() {
                        None
                    } else {
                        Some(parts.join("\n"))
                    }
                };
                return Ok(RenderedPage {
                    status: if ctx.kind == TemplateKind::NotFound {
                        404
                    } else {
                        200
                    },
                    html: themed_document(
                        &title,
                        &ctx.site_name,
                        &description,
                        &href,
                        &themed_body,
                        extra_body.as_deref(),
                        head_extra.as_deref(),
                    ),
                });
            }
        }
        return Ok(RenderedPage {
            status,
            html: layout(
                &title,
                &fallback.site_name,
                &description,
                &fallback.nav,
                &body_with_crumbs,
                fallback_head.as_deref(),
                Some(&href),
            ),
        });
    }
    Ok(RenderedPage {
        status,
        html: layout(
            &title,
            &fallback.site_name,
            &description,
            &fallback.nav,
            &body_with_crumbs,
            fallback_head.as_deref(),
            None,
        ),
    })
}

pub(crate) fn template_nav(nav: &[NavItem]) -> Vec<TemplateLink> {
    nav.iter()
        .map(|n| TemplateLink {
            title: n.title.clone(),
            url: n.url.clone(),
        })
        .collect()
}

async fn post_list_html(db: &dyn Db, site_id: i64, posts: &[Post]) -> Result<String, WordlessError> {
    let mut items: Vec<String> = Vec::new();
    for post in posts {
        let url = post_url(db, site_id, post).await?;
        let excerpt = if post.excerpt.is_empty() {
            String::new()
        } else {
            format!("<p>{}</p>", escape_html(&post.excerpt))
        };
        items.push(format!(
            "<li><h2><a href=\"{}\">{}</a></h2>\n<div class=\"meta\">{}</div>\n{excerpt}</li>",
            escape_html(&url),
            escape_html(if post.title.is_empty() {
                "(untitled)"
            } else {
                &post.title
            }),
            escape_html(&format_date(&post.date))
        ));
    }
    Ok(format!("<ul class=\"post-list\">{}</ul>", items.join("")))
}

fn render_comment(comment: &Comment, depth: i64) -> String {
    let _ = depth;
    let children = comment
        .children
        .iter()
        .map(|c| render_comment(c, depth + 1))
        .collect::<Vec<_>>()
        .join("");
    let child_list = if children.is_empty() {
        String::new()
    } else {
        format!("<ol>{children}</ol>")
    };
    format!(
        "<li><div class=\"comment-meta\"><strong>{}</strong> · {}</div>\n<p>{}</p>{child_list}</li>",
        escape_html(&comment.author),
        escape_html(&format_date(&comment.date)),
        escape_html(&comment.content).replace('\n', "<br>")
    )
}

pub async fn list_approved_threaded(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
) -> Result<Vec<Comment>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare(
            "SELECT * FROM wp_comments WHERE comment_post_ID = ? AND site_id = ? AND comment_approved = '1' \
             ORDER BY comment_date ASC, comment_ID ASC LIMIT 100",
        )
        .bind(vec![Param::Integer(post_id), Param::Integer(site_id)])
        .all::<Row>()
        .await
        .map_err(WordlessError::from)?;
    let flat: Vec<Comment> = rows
        .iter()
        .map(|r| Comment {
            id: row_int(r, "comment_ID"),
            post_id: row_int(r, "comment_post_ID"),
            author: row_text(r, "comment_author"),
            date: row_text(r, "comment_date"),
            content: row_text(r, "comment_content"),
            parent_id: row_int(r, "comment_parent"),
            children: Vec::new(),
        })
        .collect();
    Ok(nest_comments(flat))
}

/// Nest a flat, date-ordered comment list into threads (v1
/// `listApprovedThreaded`): a comment whose parent is missing or
/// not in the list becomes a root.
pub fn nest_comments(flat: Vec<Comment>) -> Vec<Comment> {
    use std::collections::HashMap;
    let mut by_id: HashMap<i64, Comment> = HashMap::new();
    let mut child_ids: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut root_ids: Vec<i64> = Vec::new();
    for c in &flat {
        by_id.insert(c.id, c.clone());
    }
    for c in &flat {
        if c.parent_id != 0 && by_id.contains_key(&c.parent_id) {
            child_ids.entry(c.parent_id).or_default().push(c.id);
        } else {
            root_ids.push(c.id);
        }
    }
    fn build(
        id: i64,
        by_id: &mut HashMap<i64, Comment>,
        child_ids: &HashMap<i64, Vec<i64>>,
    ) -> Comment {
        let mut node = by_id.remove(&id).expect("comment present");
        if let Some(kids) = child_ids.get(&id) {
            node.children = kids
                .iter()
                .map(|kid| build(*kid, by_id, child_ids))
                .collect();
        }
        node
    }
    let mut by_id = by_id;
    root_ids
        .into_iter()
        .map(|id| build(id, &mut by_id, &child_ids))
        .collect()
}

async fn comments_html(db: &dyn Db, site_id: i64, post: &Post) -> Result<String, WordlessError> {
    let comments = list_approved_threaded(db, site_id, post.id).await?;
    let list = if comments.is_empty() {
        "<p>No comments yet.</p>".to_string()
    } else {
        format!(
            "<ol>{}</ol>",
            comments
                .iter()
                .map(|c| render_comment(c, 0))
                .collect::<Vec<_>>()
                .join("")
        )
    };
    let form = if post.comment_status == "open" {
        format!(
            "<h3>Leave a reply</h3>\n<form class=\"comment-form\" method=\"post\" action=\"/{}/comments\">\n  <label for=\"comment-author\">Name</label>\n  <input id=\"comment-author\" name=\"author\" required maxlength=\"100\">\n  <label for=\"comment-email\">Email (not published)</label>\n  <input id=\"comment-email\" name=\"author_email\" type=\"email\" maxlength=\"100\">\n  <label for=\"comment-content\">Comment</label>\n  <textarea id=\"comment-content\" name=\"content\" rows=\"5\" required maxlength=\"10000\"></textarea>\n  <input type=\"text\" name=\"website\" tabindex=\"-1\" autocomplete=\"off\" style=\"display:none\" aria-hidden=\"true\">\n  <button type=\"submit\">Post comment</button>\n</form>\n<p><small>Comments are moderated before they appear.</small></p>",
            escape_html(&post.slug)
        )
    } else {
        "<p><small>Comments are closed for this post.</small></p>".to_string()
    };
    Ok(format!(
        "<section class=\"comments\" id=\"comments\">\n<h2>Comments ({})</h2>\n{list}\n{form}\n</section>",
        comments.len()
    ))
}

// ------------------------------------------------------------ public pages

pub async fn render_home(
    db: &dyn Db,
    site_id: i64,
    page: i64,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let per_page = get_posts_per_page(db, site_id).await?;
    let result = list_posts(db, site_id, "publish", "post", page, per_page).await?;
    let body = if result.posts.is_empty() {
        "<p>No posts published yet.</p>".to_string()
    } else {
        post_list_html(db, site_id, &result.posts).await? + &pagination(page, result.total_pages, "/", "")
    };
    let mut posts: Vec<TemplatePostData> = Vec::new();
    for p in &result.posts {
        posts.push(to_template_post(db, site_id, p, false, opts.converter.as_deref()).await?);
    }
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Home,
    );
    ctx.title = "Home".to_string();
    ctx.posts = Some(posts);
    ctx.page = page;
    ctx.total_pages = result.total_pages;
    ctx.base_url = "/".to_string();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title: "Home".to_string(),
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: Some(format!(
                "<link rel=\"alternate\" type=\"application/rss+xml\" title=\"{}\" href=\"/feed\">",
                escape_html(&site_name)
            )),
            status: 200,
        },
        None,
    )
    .await
}

pub async fn render_single(
    db: &dyn Db,
    site_id: i64,
    post: &Post,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let comments = if post.comment_status == "open" || post.comment_count > 0 {
        comments_html(db, site_id, post).await?
    } else {
        String::new()
    };
    let post_data = to_template_post(db, site_id, post, true, opts.converter.as_deref()).await?;
    let share = share_links(
        &format!("{}{}", site_url(db, site_id).await?, post_data.url),
        if post.title.is_empty() {
            "(untitled)"
        } else {
            &post.title
        },
    );
    let share_html = format!(
        "<section class=\"share\"><h2>Share this</h2><p>{}</p></section>",
        share
            .iter()
            .map(|(label, url)| format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noreferrer\">{}</a>",
                escape_html(url),
                label
            ))
            .collect::<Vec<_>>()
            .join(" · ")
    );
    let body = format!(
        "<article>\n<h1>{}</h1>\n<div class=\"meta\">{}</div>\n{}\n</article>{share_html}{comments}",
        escape_html(if post.title.is_empty() { "(untitled)" } else { &post.title }),
        escape_html(&format_date(&post.date)),
        post_data.content_html
    );
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Single,
    );
    ctx.title = if post.title.is_empty() {
        "(untitled)".to_string()
    } else {
        post.title.clone()
    };
    ctx.post = Some(post_data.clone());
    ctx.post_id = Some(post.id);
    ctx.page = 1;
    ctx.total_pages = 1;
    ctx.base_url = post_data.url.clone();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title: if post.title.is_empty() {
                "(untitled)".to_string()
            } else {
                post.title.clone()
            },
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        if comments.is_empty() {
            None
        } else {
            Some(comments)
        },
    )
    .await
}

pub async fn render_page(
    db: &dyn Db,
    site_id: i64,
    post: &Post,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let post_data = to_template_post(db, site_id, post, false, opts.converter.as_deref()).await?;
    let body = format!(
        "<article>\n<h1>{}</h1>\n{}\n</article>",
        escape_html(if post.title.is_empty() {
            "(untitled)"
        } else {
            &post.title
        }),
        post_data.content_html
    );
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Page,
    );
    ctx.title = if post.title.is_empty() {
        "(untitled)".to_string()
    } else {
        post.title.clone()
    };
    ctx.post = Some(post_data.clone());
    ctx.post_id = Some(post.id);
    ctx.page = 1;
    ctx.total_pages = 1;
    ctx.base_url = post_data.url.clone();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title: if post.title.is_empty() {
                "(untitled)".to_string()
            } else {
                post.title.clone()
            },
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}

pub async fn render_term_archive(
    db: &dyn Db,
    site_id: i64,
    term: &Term,
    page: i64,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let per_page = get_posts_per_page(db, site_id).await?;
    let result = list_posts_by_term(db, site_id, term.term_taxonomy_id, page, per_page).await?;
    let kind = if term.taxonomy == "category" {
        "Category".to_string()
    } else if term.taxonomy == "tag" {
        "Tag".to_string()
    } else {
        term.taxonomy.clone()
    };
    let base = if term.taxonomy == "category" {
        format!("/category/{}", term.slug)
    } else {
        format!("/tag/{}", term.slug)
    };
    let mut body = format!(
        "<h1>{}: {}</h1>",
        escape_html(&kind),
        escape_html(&term.name)
    );
    if !term.description.is_empty() {
        body.push_str(&format!("<p>{}</p>", escape_html(&term.description)));
    }
    if result.posts.is_empty() {
        body.push_str("<p>No posts in this term yet.</p>");
    } else {
        body.push_str(&post_list_html(db, site_id, &result.posts).await?);
        body.push_str(&pagination(page, result.total_pages, &base, ""));
    }
    let mut posts: Vec<TemplatePostData> = Vec::new();
    for p in &result.posts {
        posts.push(to_template_post(db, site_id, p, false, opts.converter.as_deref()).await?);
    }
    let title = format!("{kind}: {}", term.name);
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Archive,
    );
    ctx.title = title.clone();
    ctx.archive_title = Some(title.clone());
    ctx.posts = Some(posts);
    ctx.page = page;
    ctx.total_pages = result.total_pages;
    ctx.base_url = base.clone();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title,
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}

pub async fn render_date_archive(
    db: &dyn Db,
    site_id: i64,
    year: i64,
    month: Option<i64>,
    page: i64,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let prefix = match month {
        Some(m) => format!("{year}-{m:02}"),
        None => format!("{year}"),
    };
    let per_page = get_posts_per_page(db, site_id).await?;
    let posts =
        list_posts_by_date_prefix(db, site_id, &prefix, per_page, (page.max(1) - 1) * per_page).await?;
    let title = match month {
        Some(m) => format!("Archive: {year}-{m:02}"),
        None => format!("Archive: {year}"),
    };
    let mut body = format!("<h1>{}</h1>", escape_html(&title));
    if posts.is_empty() {
        body.push_str("<p>No posts in this period.</p>");
    } else {
        body.push_str(&post_list_html(db, site_id, &posts).await?);
    }
    let mut template_posts: Vec<TemplatePostData> = Vec::new();
    for p in &posts {
        template_posts.push(to_template_post(db, site_id, p, false, opts.converter.as_deref()).await?);
    }
    let base_url = match month {
        Some(m) => format!("/{year}/{m:02}"),
        None => format!("/{year}"),
    };
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Archive,
    );
    ctx.title = title.clone();
    ctx.archive_title = Some(title.clone());
    ctx.posts = Some(template_posts);
    ctx.page = page;
    ctx.total_pages = 1;
    ctx.base_url = base_url.clone();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title,
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}

/// Archive of a registered custom post type (`/{rewriteSlug}`).
/// P3: closes the P2 gap where a registered type's archive rendered
/// the 404 page. Lists the type's published posts, newest first,
/// like a term archive. `label` is the type's display label (the
/// caller reads it from `wordless-cms`); `rewrite_slug` is the URL
/// segment the archive lives under.
pub async fn render_post_type_archive(
    db: &dyn Db,
    site_id: i64,
    post_type_slug: &str,
    label: &str,
    rewrite_slug: &str,
    page: i64,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let per_page = get_posts_per_page(db, site_id).await?;
    let result = list_posts(db, site_id, "publish", post_type_slug, page, per_page).await?;
    let base = format!("/{rewrite_slug}");
    let mut body = format!("<h1>{}</h1>", escape_html(label));
    if result.posts.is_empty() {
        body.push_str("<p>No posts in this archive yet.</p>");
    } else {
        body.push_str(&post_list_html(db, site_id, &result.posts).await?);
        body.push_str(&pagination(page, result.total_pages, &base, ""));
    }
    let mut posts: Vec<TemplatePostData> = Vec::new();
    for p in &result.posts {
        posts.push(to_template_post(db, site_id, p, false, opts.converter.as_deref()).await?);
    }
    let title = label.to_string();
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Archive,
    );
    ctx.title = title.clone();
    ctx.archive_title = Some(title.clone());
    ctx.posts = Some(posts);
    ctx.page = page;
    ctx.total_pages = result.total_pages;
    ctx.base_url = base.clone();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title,
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}

pub async fn render_search_page(
    db: &dyn Db,
    site_id: i64,
    query: &str,
    page: i64,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let per_page = get_posts_per_page(db, site_id).await?;
    let result = search_posts(db, site_id, query, page, per_page).await?;
    let mut body = format!(
        "<h1>Search results for &ldquo;{}&rdquo;</h1>",
        escape_html(query)
    );
    body.push_str(&format!(
        "<p>{} result{}.</p>",
        result.total,
        if result.total == 1 { "" } else { "s" }
    ));
    if !result.posts.is_empty() {
        body.push_str(&post_list_html(db, site_id, &result.posts).await?);
        body.push_str(&pagination(
            page,
            result.total_pages,
            "/search",
            &format!("&q={}", encode_uri_component(query)),
        ));
    }
    let mut posts: Vec<TemplatePostData> = Vec::new();
    for p in &result.posts {
        posts.push(to_template_post(db, site_id, p, false, opts.converter.as_deref()).await?);
    }
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Search,
    );
    ctx.title = format!("Search: {query}");
    ctx.query = Some(query.to_string());
    ctx.posts = Some(posts);
    ctx.page = page;
    ctx.total_pages = result.total_pages;
    ctx.base_url = format!("/search?q={}", encode_uri_component(query));
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title: format!("Search: {query}"),
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}

pub async fn render_not_found(
    db: &dyn Db,
    site_id: i64,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::NotFound,
    );
    ctx.title = "Not found".to_string();
    ctx.page = 1;
    ctx.total_pages = 1;
    ctx.base_url = "/".to_string();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title: "Not found".to_string(),
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body: "<h1>Page not found</h1><p>The page you asked for does not exist. <a href=\"/\">Go home</a>.</p>"
                .to_string(),
            head_extra: None,
            status: 404,
        },
        None,
    )
    .await
}

/// Look up a post by ID or return a 404 error (for the coordinator's
/// public handler: `/?p=<id>` and SEO lookups).
pub async fn require_post(db: &dyn Db, site_id: i64, id: i64) -> Result<Post, WordlessError> {
    match get_post_by_id(db, site_id, id).await? {
        Some(post) => Ok(post),
        None => Err(not_found(&format!("Post {id} not found."))),
    }
}

/// Plain-text excerpt helper re-exported for callers that build
/// list pages (v1 `plainTextExcerpt`, default 200 chars).
pub fn excerpt(content: &str) -> String {
    plain_text_excerpt(content, 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;

    fn seeded_db() -> SqliteDb {
        let db = SqliteDb::new();
        block_on(async {
            db.set_option_raw("blogname", "\"Wordless\"").await;
            db.set_option_raw("siteurl", "\"http://localhost:8787\"")
                .await;
        });
        db
    }

    fn opts() -> RenderOptions {
        RenderOptions::default()
    }

    #[test]
    fn home_lists_published_only_with_escaped_titles_v1_vector() {
        let db = seeded_db();
        block_on(async {
            db.insert_post(
                "Public <script>",
                "Body",
                "",
                "publish",
                "public-script",
                "post",
                "2026-11-19T00:00:00.000Z",
                1,
            )
            .await;
            db.insert_post(
                "Draft One",
                "Body",
                "",
                "draft",
                "draft-one",
                "post",
                "2026-11-19T00:00:00.000Z",
                1,
            )
            .await;
            let page = render_home(&db, 1, 1, &opts()).await.unwrap();
            assert_eq!(page.status, 200);
            assert!(page.html.contains("Public &lt;script&gt;"), "{}", page.html);
            assert!(!page.html.contains("<script>"), "{}", page.html);
            assert!(!page.html.contains("Draft One"), "{}", page.html);
            assert!(page.html.contains("href=\"/feed\""), "{}", page.html);
        });
    }

    #[test]
    fn single_renders_blocks_and_escapes_title_v1_vector() {
        let db = seeded_db();
        block_on(async {
            let id = db
                .insert_post(
                    "A & B",
                    "<!-- wp:paragraph --><p>Body text</p><!-- /wp:paragraph -->",
                    "",
                    "publish",
                    "a-b",
                    "post",
                    "2026-11-19T00:00:00.000Z",
                    1,
                )
                .await;
            let post = get_post_by_id(&db, 1, id).await.unwrap().unwrap();
            let page = render_single(&db, 1, &post, &opts()).await.unwrap();
            assert_eq!(page.status, 200);
            assert!(
                page.html.contains("<title>A &amp; B — Wordless</title>"),
                "{}",
                page.html
            );
            assert!(page.html.contains("<p>Body text</p>"), "{}", page.html);
            assert!(page.html.contains("Leave a reply"), "{}", page.html);
        });
    }

    #[test]
    fn not_found_page_v1_vector() {
        let db = seeded_db();
        block_on(async {
            let page = render_not_found(&db, 1, &opts()).await.unwrap();
            assert_eq!(page.status, 404);
            assert!(page.html.contains("Page not found"), "{}", page.html);
            assert!(page.html.contains("href=\"/\""), "{}", page.html);
        });
    }

    #[test]
    fn themed_home_matches_v1_for_same_content() {
        // Active block theme (default-theme home.html + header/footer
        // parts): the themed home must list the post with a linked
        // title and carry the /theme.css?v=<hash> link.
        let db = seeded_db();
        block_on(async {
            db.insert_post(
                "Hello",
                "<!-- wp:paragraph --><p>Body</p><!-- /wp:paragraph -->",
                "Intro.",
                "publish",
                "hello",
                "post",
                "2026-11-19T00:00:00.000Z",
                1,
            )
            .await;
            let mut files: BTreeMap<String, String> = BTreeMap::new();
            files.insert("parts/header.html".to_string(), "<!-- wp:group {\"tagName\":\"header\"} --><header><!-- wp:site-title /--><!-- wp:navigation /--></header><!-- /wp:group -->".to_string());
            files.insert("parts/footer.html".to_string(), "<!-- wp:paragraph --><p>Powered by <a href=\"/admin\">Wordless</a></p><!-- /wp:paragraph -->".to_string());
            files.insert("templates/home.html".to_string(), "<!-- wp:template-part {\"slug\":\"header\"} /--><!-- wp:query --><!-- wp:post-template --><!-- wp:post-title {\"level\":2,\"isLink\":true} /--><!-- wp:post-excerpt /--><!-- /wp:post-template --><!-- /wp:query --><!-- wp:template-part {\"slug\":\"footer\"} /-->".to_string());
            let opts = RenderOptions {
                theme: Some(ThemeView {
                    kind: "block".to_string(),
                    files,
                    stylesheet_hash: "abc123".to_string(),
                }),
                seo: None,
                preview_token: None,
                converter: None,
                woo_client: None,
            };
            let page = render_home(&db, 1, 1, &opts).await.unwrap();
            assert!(
                page.html
                    .contains("<link rel=\"stylesheet\" href=\"/theme.css?v=abc123\">"),
                "{}",
                page.html
            );
            assert!(
                page.html.contains(
                    "<h2 class=\"wp-block-post-title\"><a href=\"/hello\">Hello</a></h2>"
                ),
                "{}",
                page.html
            );
            assert!(
                page.html
                    .contains("<p class=\"wp-block-post-excerpt\">Intro.</p>"),
                "{}",
                page.html
            );
            assert!(
                page.html
                    .contains("<h1 class=\"wp-block-site-title\"><a href=\"/\">Wordless</a></h1>"),
                "{}",
                page.html
            );
        });
    }

    #[test]
    fn resolve_path_covers_v1_resolution_order() {
        let db = seeded_db();
        block_on(async {
            let id = db
                .insert_post(
                    "Hello",
                    "Body",
                    "",
                    "publish",
                    "hello",
                    "post",
                    "2026-11-19T00:00:00.000Z",
                    1,
                )
                .await;
            // Term archive.
            db.prepare("INSERT INTO wp_terms (name, slug, term_group) VALUES ('News', 'news', 0)")
                .bind(vec![])
                .run()
                .await
                .unwrap();
            let tt = db.prepare("INSERT INTO wp_term_taxonomy (term_id, taxonomy, description, parent, count) VALUES (1, 'category', '', 0, 1)")
                .bind(vec![]).run().await.unwrap();
            let tt_id = tt.last_row_id.unwrap();
            db.prepare("INSERT INTO wp_term_relationships (object_id, term_taxonomy_id, term_order) VALUES (?, ?, 0)")
                .bind(vec![Param::Integer(id), Param::Integer(tt_id)]).run().await.unwrap();
            // Redirect for the old slug.
            db.prepare("INSERT INTO wl_redirects (old_path, new_path, post_id, created_at) VALUES ('/old-hello', '/hello', ?, '2026-11-19T00:00:00.000Z')")
                .bind(vec![Param::Integer(id)]).run().await.unwrap();

            assert_eq!(resolve_path(&db, 1, "/").await.unwrap(), ResolvedPath::Home);
            assert!(matches!(
                resolve_path(&db, 1, "/hello").await.unwrap(),
                ResolvedPath::Post(_)
            ));
            assert!(matches!(
                resolve_path(&db, 1, "/2026/11/19/hello").await.unwrap(),
                ResolvedPath::Post(_)
            ));
            assert!(matches!(
                resolve_path(&db, 1, "/2026/11").await.unwrap(),
                ResolvedPath::DateArchive {
                    year: 2026,
                    month: Some(11)
                }
            ));
            assert!(matches!(
                resolve_path(&db, 1, "/2026").await.unwrap(),
                ResolvedPath::DateArchive {
                    year: 2026,
                    month: None
                }
            ));
            assert!(matches!(
                resolve_path(&db, 1, "/category/news").await.unwrap(),
                ResolvedPath::Term(_)
            ));
            assert_eq!(
                resolve_path(&db, 1, "/old-hello").await.unwrap(),
                ResolvedPath::Redirect("/hello".to_string())
            );
            assert_eq!(
                resolve_path(&db, 1, "/nope").await.unwrap(),
                ResolvedPath::NotFound
            );
            assert_eq!(
                resolve_path(&db, 1, "/category/nope").await.unwrap(),
                ResolvedPath::NotFound
            );
            // Dated URL with the wrong date does not hijack the slug.
            assert_eq!(
                resolve_path(&db, 1, "/2020/01/01/hello").await.unwrap(),
                ResolvedPath::NotFound
            );
        });
    }

    #[test]
    fn search_and_term_archive_render() {
        let db = seeded_db();
        block_on(async {
            let id = db
                .insert_post(
                    "Wolf Guide",
                    "Body about wolves",
                    "",
                    "publish",
                    "wolf-guide",
                    "post",
                    "2026-11-19T00:00:00.000Z",
                    1,
                )
                .await;
            db.insert_post(
                "Other",
                "Nothing relevant",
                "",
                "publish",
                "other",
                "post",
                "2026-11-19T00:00:00.000Z",
                1,
            )
            .await;
            let page = render_search_page(&db, 1, "wolves", 1, &opts()).await.unwrap();
            assert!(page.html.contains("Wolf Guide"), "{}", page.html);
            assert!(!page.html.contains(">Other<"), "{}", page.html);
            assert!(page.html.contains("1 result."), "{}", page.html);
            // Search with no query term match still renders.
            let page = render_search_page(&db, 1, "zzz-no-match", 1, &opts())
                .await
                .unwrap();
            assert!(page.html.contains("0 results."), "{}", page.html);

            db.prepare("INSERT INTO wp_terms (name, slug, term_group) VALUES ('News', 'news', 0)")
                .bind(vec![])
                .run()
                .await
                .unwrap();
            let tt = db.prepare("INSERT INTO wp_term_taxonomy (term_id, taxonomy, description, parent, count) VALUES (1, 'category', 'Site news', 0, 1)")
                .bind(vec![]).run().await.unwrap();
            let tt_id = tt.last_row_id.unwrap();
            db.prepare("INSERT INTO wp_term_relationships (object_id, term_taxonomy_id, term_order) VALUES (?, ?, 0)")
                .bind(vec![Param::Integer(id), Param::Integer(tt_id)]).run().await.unwrap();
            let term = get_term_by_taxonomy_id(&db, 1, tt_id).await.unwrap().unwrap();
            let page = render_term_archive(&db, 1, &term, 1, &opts()).await.unwrap();
            assert!(page.html.contains("Category: News"), "{}", page.html);
            assert!(page.html.contains("Site news"), "{}", page.html);
            assert!(page.html.contains("Wolf Guide"), "{}", page.html);
        });
    }

    #[test]
    fn elementor_posts_render_block_content_only() {
        let db = seeded_db();
        block_on(async {
            let id = db
                .insert_post(
                    "Built",
                    "<!-- wp:paragraph --><p>Block body</p><!-- /wp:paragraph -->",
                    "",
                    "publish",
                    "built",
                    "post",
                    "2026-11-19T00:00:00.000Z",
                    1,
                )
                .await;
            db.prepare("INSERT INTO wp_postmeta (post_id, meta_key, meta_value) VALUES (?, '_elementor_data', '[{\"id\":\"abc\",\"elements\":[]} ]')")
                .bind(vec![Param::Integer(id)]).run().await.unwrap();
            assert!(has_elementor_data(&db, 1, id).await.unwrap());
            let post = get_post_by_id(&db, 1, id).await.unwrap().unwrap();
            // P2: block content renders; the Elementor sections do not.
            assert_eq!(
                render_post_content(&db, 1, &post, None).await.unwrap(),
                "<p>Block body</p>"
            );
            let page = render_single(&db, 1, &post, &opts()).await.unwrap();
            assert!(page.html.contains("<p>Block body</p>"), "{}", page.html);
            assert!(!ELEMENTOR_GAP.is_empty());
        });
    }

    #[test]
    fn comments_thread_and_share_links() {
        let db = seeded_db();
        block_on(async {
            let id = db
                .insert_post(
                    "Talk",
                    "Body",
                    "",
                    "publish",
                    "talk",
                    "post",
                    "2026-11-19T00:00:00.000Z",
                    1,
                )
                .await;
            db.prepare("INSERT INTO wp_comments (comment_post_ID, comment_author, comment_content, comment_approved, comment_date, comment_parent) VALUES (?, 'Ada', 'First!', '1', '2026-11-19T01:00:00.000Z', 0)")
                .bind(vec![Param::Integer(id)]).run().await.unwrap();
            db.prepare("INSERT INTO wp_comments (comment_post_ID, comment_author, comment_content, comment_approved, comment_date, comment_parent) VALUES (?, 'Bo', 'Reply', '1', '2026-11-19T02:00:00.000Z', 1)")
                .bind(vec![Param::Integer(id)]).run().await.unwrap();
            db.prepare("INSERT INTO wp_comments (comment_post_ID, comment_author, comment_content, comment_approved, comment_date, comment_parent) VALUES (?, 'Cy', 'Pending', '0', '2026-11-19T03:00:00.000Z', 0)")
                .bind(vec![Param::Integer(id)]).run().await.unwrap();
            let threaded = list_approved_threaded(&db, 1, id).await.unwrap();
            assert_eq!(threaded.len(), 1);
            assert_eq!(threaded[0].children.len(), 1);
            assert_eq!(threaded[0].children[0].author, "Bo");
            assert_eq!(threaded[0].children[0].content, "Reply");
            let post = get_post_by_id(&db, 1, id).await.unwrap().unwrap();
            let page = render_single(&db, 1, &post, &opts()).await.unwrap();
            assert!(page.html.contains("Comments (1)"), "{}", page.html);
            assert!(
                page.html.contains("twitter.com/intent/tweet"),
                "{}",
                page.html
            );
            // Permalink structures.
            let plain = post_url_with_structure(&post, "plain");
            assert_eq!(plain, format!("/?p={id}"));
            assert_eq!(
                post_url_with_structure(&post, "day-name"),
                "/2026/11/19/talk"
            );
            assert_eq!(term_url("category", "news"), "/category/news");
            assert_eq!(term_url("post_tag", "x"), "/post_tag/x");
        });
    }
}
