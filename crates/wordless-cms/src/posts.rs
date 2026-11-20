//! Posts — port of v1 `src/modules/posts/` (service.ts, repo.ts, types.ts).
//!
//! Statuses: draft / publish / private / trash. Slugs are unique per
//! site; collisions get a numeric suffix (-2, -3, ...). A revision
//! snapshots the pre-update content whenever the visible content
//! changes; slug changes on published posts record a 301 in
//! `wl_redirects` (see `crate::permalinks`).

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::common::{count_total, optional_str, page_of, per_page_of, require_str, total_pages};
use crate::postmeta::{delete_post_meta, get_post_meta, set_post_meta};

/// Post status. Serialises to the v1 strings `draft | publish | private | trash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PostStatus {
    Draft,
    Publish,
    Private,
    Trash,
}

impl PostStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PostStatus::Draft => "draft",
            PostStatus::Publish => "publish",
            PostStatus::Private => "private",
            PostStatus::Trash => "trash",
        }
    }

    /// `from_str` here mirrors v1's stringly-typed statuses and P1's
    /// `Role::from_str` in wordless-kernel; it is intentionally not
    /// `std::str::FromStr` (which would need an error type).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<PostStatus> {
        match s {
            "draft" => Some(PostStatus::Draft),
            "publish" => Some(PostStatus::Publish),
            "private" => Some(PostStatus::Private),
            "trash" => Some(PostStatus::Trash),
            _ => None,
        }
    }
}

pub fn is_post_status(value: &str) -> bool {
    PostStatus::from_str(value).is_some()
}

/// A `wp_posts` row, serialised with the v1 camelCase field names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Post {
    pub id: i64,
    pub author_id: i64,
    pub date: String,
    pub date_gmt: String,
    pub content: String,
    pub title: String,
    pub excerpt: String,
    pub status: PostStatus,
    pub comment_status: String,
    pub ping_status: String,
    pub password: String,
    pub slug: String,
    pub modified: String,
    pub modified_gmt: String,
    pub content_filtered: String,
    pub parent_id: i64,
    pub guid: String,
    pub menu_order: i64,
    #[serde(rename = "type")]
    pub post_type: String,
    pub mime_type: String,
    pub comment_count: i64,
    /// P12: the site that owns this post. Internal only — never
    /// serialised, so v1 parity responses are byte-identical.
    #[serde(skip)]
    pub site_id: i64,
}

/// Map a D1 row to a `Post` (port of `rowToPost` in v1 `posts/repo.ts`).
/// An unknown `post_status` falls back to `draft`, exactly as v1 does.
pub fn row_to_post(row: &Row) -> Post {
    Post {
        id: row_int(row, "ID"),
        author_id: row_int(row, "post_author"),
        date: row_text(row, "post_date"),
        date_gmt: row_text(row, "post_date_gmt"),
        content: row_text(row, "post_content"),
        title: row_text(row, "post_title"),
        excerpt: row_text(row, "post_excerpt"),
        status: PostStatus::from_str(&row_text(row, "post_status")).unwrap_or(PostStatus::Draft),
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
        // Rows predate P12 (or test DBs without the column) read 0;
        // site ids start at 1, so 0 always means "the main site".
        site_id: match row_int(row, "site_id") {
            0 => 1,
            n => n,
        },
    }
}

#[derive(Debug, Clone, Default)]
pub struct CreatePostInput {
    pub title: String,
    pub content: Option<String>,
    pub excerpt: Option<String>,
    pub slug: Option<String>,
    pub status: Option<String>,
    pub post_type: Option<String>,
    pub author_id: i64,
    pub parent_id: Option<i64>,
    pub mime_type: Option<String>,
    /// Featured image (media attachment id). `None` = leave unset.
    /// P4: v1 has no post featured-image support — the admin's
    /// `featuredImageId` only existed on products — so this writes
    /// the WordPress-standard `_thumbnail_id` postmeta row.
    pub featured_image_id: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdatePostInput {
    pub title: Option<String>,
    pub content: Option<String>,
    pub excerpt: Option<String>,
    pub slug: Option<String>,
    pub status: Option<String>,
    pub post_type: Option<String>,
    pub parent_id: Option<i64>,
    /// Tri-state like `publishAt`: `None` = absent (no change),
    /// `Some(Some(id))` = set, `Some(None)` = clear the image.
    pub featured_image_id: Option<Option<i64>>,
}

#[derive(Debug, Clone, Default)]
pub struct ListPostsOptions {
    pub status: Option<String>,
    pub post_type: Option<String>,
    pub author_id: Option<i64>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    /// When false (default), trashed posts are excluded from unfiltered lists.
    pub include_trash: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostListResult {
    pub posts: Vec<Post>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// "Hello, World!" -> "hello-world"; "Café & Bar" -> "cafe-bar"; "" -> "post".
/// Port of v1 `slugify` (`posts/service.ts`): NFD decomposition, strip
/// combining marks (U+0300–U+036F), lowercase, non-alphanumerics to `-`,
/// trim leading/trailing hyphens, truncate to 200 chars.
pub fn slugify(text: &str) -> String {
    let decomposed: String = text.nfd().collect();
    let mut slug = String::new();
    for ch in decomposed.chars() {
        if ('\u{0300}'..='\u{036f}').contains(&ch) {
            continue;
        }
        let lower = ch.to_lowercase().next().unwrap_or(ch);
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return "post".to_string();
    }
    slug.chars().take(200).collect()
}

fn validate_type(post_type: &str) -> Result<String, WordlessError> {
    let t = post_type.to_lowercase();
    let valid = !t.is_empty()
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(bad_request(
            "Field \"type\" must be a lowercase post-type key (letters, digits, underscore).",
        ));
    }
    Ok(t)
}

pub async fn ensure_unique_slug(
    db: &dyn Db,
    site_id: i64,
    base: &str,
    exclude_id: Option<i64>,
) -> Result<String, WordlessError> {
    ensure_unique_slug_for_type(db, site_id, base, "post", exclude_id).await
}

/// Slug uniqueness scoped to the post's type. v1's `ensureUniqueSlug`
/// only checks `post`/`page` slugs (`findPostBySlug`); for a custom
/// post type the check must run within that type instead, or two
/// posts of the same custom type could share a slug (WordPress
/// enforces uniqueness per post type).
pub async fn ensure_unique_slug_for_type(
    db: &dyn Db,
    site_id: i64,
    base: &str,
    post_type: &str,
    exclude_id: Option<i64>,
) -> Result<String, WordlessError> {
    let mut candidate = base.to_string();
    let mut n = 2;
    loop {
        let existing = if post_type == "post" || post_type == "page" {
            find_post_by_slug(db, site_id, &candidate).await?
        } else {
            find_post_by_slug_and_type(db, site_id, &candidate, post_type).await?
        };
        match existing {
            Some(post) if exclude_id.is_none_or(|id| post.id != id) => {
                candidate = format!("{base}-{n}");
                n += 1;
            }
            _ => return Ok(candidate),
        }
    }
}

// ---------------------------------------------------------------- repo

#[allow(clippy::too_many_arguments)]
pub async fn insert_post_row(
    db: &dyn Db,
    site_id: i64,
    author_id: i64,
    date: &str,
    content: &str,
    title: &str,
    excerpt: &str,
    status: &str,
    slug: &str,
    parent_id: i64,
    post_type: &str,
    mime_type: &str,
    comment_status: &str,
) -> Result<i64, WordlessError> {
    let meta = db
        .prepare(
            "INSERT INTO wp_posts
               (site_id, post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt,
                post_status, post_name, post_modified, post_modified_gmt, post_parent,
                post_type, post_mime_type, comment_status)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(site_id),
            Param::Integer(author_id),
            Param::Text(date.to_string()),
            Param::Text(date.to_string()),
            Param::Text(content.to_string()),
            Param::Text(title.to_string()),
            Param::Text(excerpt.to_string()),
            Param::Text(status.to_string()),
            Param::Text(slug.to_string()),
            Param::Text(date.to_string()),
            Param::Text(date.to_string()),
            Param::Integer(parent_id),
            Param::Text(post_type.to_string()),
            Param::Text(mime_type.to_string()),
            Param::Text(comment_status.to_string()),
        ])
        .run()
        .await?;
    Ok(meta.last_row_id.unwrap_or(0))
}

pub async fn find_post_by_id(
    db: &dyn Db,
    site_id: i64,
    id: i64,
) -> Result<Option<Post>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wp_posts WHERE ID = ? AND site_id = ? LIMIT 1")
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(row.map(|r| row_to_post(&r)))
}

/// v1's slug lookup: only `post` and `page` types are matched.
pub async fn find_post_by_slug(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
) -> Result<Option<Post>, WordlessError> {
    let row: Option<Row> = db
        .prepare(
            "SELECT * FROM wp_posts WHERE post_name = ? AND post_type IN ('post', 'page') AND site_id = ? LIMIT 1",
        )
        .bind(vec![Param::Text(slug.to_string()), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(row.map(|r| row_to_post(&r)))
}

/// Slug lookup for an explicit post type (used for custom post types,
/// whose slugs v1's `findPostBySlug` never matches).
pub async fn find_post_by_slug_and_type(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
    post_type: &str,
) -> Result<Option<Post>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wp_posts WHERE post_name = ? AND post_type = ? AND site_id = ? LIMIT 1")
        .bind(vec![
            Param::Text(slug.to_string()),
            Param::Text(post_type.to_string()),
            Param::Integer(site_id),
        ])
        .first()
        .await?;
    Ok(row.map(|r| row_to_post(&r)))
}

async fn count_posts(
    db: &dyn Db,
    where_clause: &str,
    params: &[Param],
) -> Result<i64, WordlessError> {
    let row: Option<Row> = db
        .prepare(&format!(
            "SELECT COUNT(*) AS total FROM wp_posts {where_clause}"
        ))
        .bind(params.to_vec())
        .first()
        .await?;
    Ok(count_total(&row))
}

async fn find_posts(
    db: &dyn Db,
    where_clause: &str,
    params: &[Param],
    limit: i64,
    offset: i64,
) -> Result<Vec<Post>, WordlessError> {
    let mut all_params = params.to_vec();
    all_params.push(Param::Integer(limit));
    all_params.push(Param::Integer(offset));
    let rows: Vec<Row> = db
        .prepare(&format!(
            "SELECT * FROM wp_posts {where_clause} ORDER BY post_date DESC, ID DESC LIMIT ? OFFSET ?"
        ))
        .bind(all_params)
        .all()
        .await?;
    Ok(rows.iter().map(row_to_post).collect())
}

pub async fn update_post_row(
    db: &dyn Db,
    site_id: i64,
    id: i64,
    sets: &[String],
    params: &[Param],
) -> Result<u64, WordlessError> {
    let mut all_params = params.to_vec();
    all_params.push(Param::Integer(id));
    all_params.push(Param::Integer(site_id));
    let meta = db
        .prepare(&format!(
            "UPDATE wp_posts SET {} WHERE ID = ? AND site_id = ?",
            sets.join(", ")
        ))
        .bind(all_params)
        .run()
        .await?;
    Ok(meta.changes)
}

pub async fn delete_post_row(db: &dyn Db, site_id: i64, id: i64) -> Result<u64, WordlessError> {
    db.prepare("DELETE FROM wp_postmeta WHERE post_id = ? AND site_id = ?")
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .run()
        .await?;
    // Revisions and autosaves belong to the post; they go with it.
    db.prepare("DELETE FROM wp_posts WHERE post_parent = ? AND post_type = 'revision' AND site_id = ?")
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .run()
        .await?;
    let meta = db
        .prepare("DELETE FROM wp_posts WHERE ID = ? AND site_id = ?")
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .run()
        .await?;
    Ok(meta.changes)
}

// ---------------------------------------------------------------- service

/// The `wp_postmeta` key WordPress uses for a post's featured image
/// (the media attachment id, stored as a decimal string).
pub const THUMBNAIL_META_KEY: &str = "_thumbnail_id";

/// Validate a featured-image id: it must reference an existing
/// `attachment` post (a media item). A missing post, a non-attachment
/// post, or an id < 1 is a 400 — a dangling thumbnail would render as
/// a broken image, so the API refuses it instead of storing it.
pub async fn validate_featured_image_id(
    db: &dyn Db,
    site_id: i64,
    id: i64,
) -> Result<(), WordlessError> {
    if id < 1 {
        return Err(bad_request(
            "Field \"featuredImageId\" must be a media attachment id (>= 1).",
        ));
    }
    match find_post_by_id(db, site_id, id).await? {
        Some(p) if p.post_type == "attachment" => Ok(()),
        _ => Err(bad_request(&format!(
            "Field \"featuredImageId\" must reference an existing media attachment (no attachment with id {id})."
        ))),
    }
}

/// Read a post's featured-image id from its `_thumbnail_id` meta.
/// `None` when unset or when the stored value is not a valid id
/// (a stale/garbage value reads as "no image", never a 500).
pub async fn get_featured_image_id(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
) -> Result<Option<i64>, WordlessError> {
    match get_post_meta(db, site_id, post_id, THUMBNAIL_META_KEY).await? {
        Some(raw) => Ok(raw.trim().parse::<i64>().ok().filter(|n| *n >= 1)),
        None => Ok(None),
    }
}

pub async fn create_post(
    db: &dyn Db,
    site_id: i64,
    input: CreatePostInput,
) -> Result<Post, WordlessError> {
    let title = require_str(&input.title, "title", 1, 1000)?;
    let content = optional_str(input.content.as_deref(), "content", 1_000_000)?.unwrap_or_default();
    let excerpt = optional_str(input.excerpt.as_deref(), "excerpt", 10000)?.unwrap_or_default();
    let status = input.status.clone().unwrap_or_else(|| "draft".to_string());
    if !is_post_status(&status) {
        return Err(bad_request(
            "Field \"status\" must be one of draft, publish, private, trash.",
        ));
    }
    let post_type = validate_type(
        &input
            .post_type
            .clone()
            .unwrap_or_else(|| "post".to_string()),
    )?;
    // Validate the featured image before inserting anything, so a
    // bad attachment id fails the whole create (no orphaned row).
    if let Some(featured) = input.featured_image_id {
        validate_featured_image_id(db, site_id, featured).await?;
    };
    let base_slug = match &input.slug {
        Some(s) => slugify(s),
        None => slugify(&title),
    };
    let slug = ensure_unique_slug_for_type(db, site_id, &base_slug, &post_type, None).await?;
    let now = now_iso();
    // New posts inherit the site's default comment status (settings, M3).
    let comment_status = crate::settings::get_default_comment_status(db, site_id).await?;
    let id = insert_post_row(
        db,
        site_id,
        input.author_id,
        &now,
        &content,
        &title,
        &excerpt,
        &status,
        &slug,
        input.parent_id.unwrap_or(0),
        &post_type,
        &input.mime_type.unwrap_or_default(),
        &comment_status,
    )
    .await?;
    // P4: a featured image rides on `_thumbnail_id` meta (already
    // validated above, before the insert).
    if let Some(featured) = input.featured_image_id {
        set_post_meta(db, site_id, id, THUMBNAIL_META_KEY, &featured.to_string()).await?;
    }
    let created = find_post_by_id(db, site_id, id).await?;
    created.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the created post.",
        )
    })
}

pub async fn get_post_by_id(
    db: &dyn Db,
    site_id: i64,
    id: i64,
) -> Result<Option<Post>, WordlessError> {
    find_post_by_id(db, site_id, id).await
}

/// Public lookup: trashed posts are never returned by slug.
pub async fn get_post_by_slug(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
) -> Result<Option<Post>, WordlessError> {
    let post = find_post_by_slug(db, site_id, slug).await?;
    match post {
        Some(p) if p.status == PostStatus::Trash => Ok(None),
        other => Ok(other),
    }
}

/// Route helper mirroring `GET /api/posts/:slug` in v1 `src/index.ts`:
/// numeric "slugs" are post IDs (the editor loads /api/posts/<id>).
/// Returns `None` for a missing post or a trashed post; the coordinator
/// turns that into `not_found` (`No post found with slug "..."`).
pub async fn get_post_by_slug_or_id(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
) -> Result<Option<Post>, WordlessError> {
    let is_numeric = !slug.is_empty() && slug.chars().all(|c| c.is_ascii_digit());
    let post = if is_numeric {
        let id: i64 = slug.parse().unwrap_or(0);
        find_post_by_id(db, site_id, id).await?
    } else {
        find_post_by_slug(db, site_id, slug).await?
    };
    match post {
        Some(p) if p.status == PostStatus::Trash => Ok(None),
        other => Ok(other),
    }
}

pub async fn list_posts(
    db: &dyn Db,
    site_id: i64,
    opts: ListPostsOptions,
) -> Result<PostListResult, WordlessError> {
    let page = page_of(opts.page);
    let per_page = per_page_of(opts.per_page);

    // P12: site scoping is the first predicate on every list, so a
    // list can never return another site's posts.
    let mut where_parts: Vec<String> = vec!["site_id = ?".to_string()];
    let mut params: Vec<Param> = vec![Param::Integer(site_id)];
    if let Some(status) = &opts.status {
        if !is_post_status(status) {
            return Err(bad_request(&format!("Unknown post status \"{status}\".")));
        }
        where_parts.push("post_status = ?".to_string());
        params.push(Param::Text(status.clone()));
    } else if !opts.include_trash {
        where_parts.push("post_status != ?".to_string());
        params.push(Param::Text("trash".to_string()));
    }
    if let Some(t) = &opts.post_type {
        where_parts.push("post_type = ?".to_string());
        params.push(Param::Text(validate_type(t)?));
    }
    if let Some(author_id) = opts.author_id {
        where_parts.push("post_author = ?".to_string());
        params.push(Param::Integer(author_id));
    }
    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    let total = count_posts(db, &where_clause, &params).await?;
    let total_pages = total_pages(total, per_page);
    let posts = find_posts(db, &where_clause, &params, per_page, (page - 1) * per_page).await?;
    Ok(PostListResult {
        posts,
        total,
        page,
        per_page,
        total_pages,
    })
}

pub async fn update_post(
    db: &dyn Db,
    site_id: i64,
    id: i64,
    patch: UpdatePostInput,
) -> Result<Post, WordlessError> {
    update_post_inner(db, site_id, id, patch, false).await
}

pub(crate) async fn update_post_inner(
    db: &dyn Db,
    site_id: i64,
    id: i64,
    patch: UpdatePostInput,
    skip_revision: bool,
) -> Result<Post, WordlessError> {
    let existing = find_post_by_id(db, site_id, id)
        .await?
        .ok_or_else(|| not_found(&format!("Post {id} not found.")))?;

    // P4: validate the featured image before writing anything, so a
    // bad attachment id cannot half-apply a title/content change.
    if let Some(Some(featured)) = patch.featured_image_id {
        validate_featured_image_id(db, site_id, featured).await?;
    }
    let meta_change = patch.featured_image_id.is_some();

    let mut sets: Vec<String> = Vec::new();
    let mut params: Vec<Param> = Vec::new();
    // A revision snapshots the pre-update content whenever the visible
    // content changes. Trash/restore transitions pass skipRevision.
    let content_changed = patch.title.is_some()
        || patch.content.is_some()
        || patch.excerpt.is_some()
        || patch.status.is_some();
    if let Some(title) = &patch.title {
        sets.push("post_title = ?".to_string());
        params.push(Param::Text(require_str(title, "title", 1, 1000)?));
    }
    if let Some(content) = &patch.content {
        sets.push("post_content = ?".to_string());
        params.push(Param::Text(
            optional_str(Some(content.as_str()), "content", 1_000_000)?.unwrap_or_default(),
        ));
    }
    if let Some(excerpt) = &patch.excerpt {
        sets.push("post_excerpt = ?".to_string());
        params.push(Param::Text(
            optional_str(Some(excerpt.as_str()), "excerpt", 10000)?.unwrap_or_default(),
        ));
    }
    if let Some(status) = &patch.status {
        if !is_post_status(status) {
            return Err(bad_request(
                "Field \"status\" must be one of draft, publish, private, trash.",
            ));
        }
        sets.push("post_status = ?".to_string());
        params.push(Param::Text(status.clone()));
    }
    if let Some(slug) = &patch.slug {
        let base = slugify(slug);
        let new_slug =
            ensure_unique_slug_for_type(db, site_id, &base, &existing.post_type, Some(id)).await?;
        if new_slug != existing.slug && existing.status == PostStatus::Publish {
            // Record the old permalink so the old URL 301s to the new one.
            crate::permalinks::record_slug_change(db, site_id, &existing, &new_slug).await?;
        }
        sets.push("post_name = ?".to_string());
        params.push(Param::Text(new_slug));
    }
    if let Some(t) = &patch.post_type {
        sets.push("post_type = ?".to_string());
        params.push(Param::Text(validate_type(t)?));
    }
    if let Some(parent_id) = patch.parent_id {
        sets.push("post_parent = ?".to_string());
        params.push(Param::Integer(parent_id));
    }
    if sets.is_empty() && !meta_change {
        return Ok(existing);
    }

    if content_changed && !skip_revision {
        crate::revisions::create_revision(db, site_id, &existing).await?;
    }
    if !sets.is_empty() {
        let now = now_iso();
        sets.push("post_modified = ?".to_string());
        sets.push("post_modified_gmt = ?".to_string());
        params.push(Param::Text(now.clone()));
        params.push(Param::Text(now));
        update_post_row(db, site_id, id, &sets, &params).await?;
    }
    // P4: the featured image is meta, not a row column — it applies
    // even when no row field changed. Setting it does not snapshot a
    // revision (revisions snapshot title/content/excerpt/status).
    if let Some(featured) = patch.featured_image_id {
        match featured {
            Some(attachment_id) => {
                set_post_meta(db, site_id, id, THUMBNAIL_META_KEY, &attachment_id.to_string()).await?;
            }
            None => {
                delete_post_meta(db, site_id, id, THUMBNAIL_META_KEY).await?;
            }
        }
    }
    let updated = find_post_by_id(db, site_id, id).await?;
    updated.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the updated post.",
        )
    })
}

pub async fn trash_post(db: &dyn Db, site_id: i64, id: i64) -> Result<Post, WordlessError> {
    update_post_inner(
        db,
        site_id,
        id,
        UpdatePostInput {
            status: Some("trash".to_string()),
            ..Default::default()
        },
        true,
    )
    .await
}

pub async fn restore_post(db: &dyn Db, site_id: i64, id: i64) -> Result<Post, WordlessError> {
    let existing = find_post_by_id(db, site_id, id)
        .await?
        .ok_or_else(|| not_found(&format!("Post {id} not found.")))?;
    if existing.status != PostStatus::Trash {
        return Ok(existing);
    }
    update_post_inner(
        db,
        site_id,
        id,
        UpdatePostInput {
            status: Some("draft".to_string()),
            ..Default::default()
        },
        true,
    )
    .await
}

/// Permanently delete a post and its postmeta. Irreversible — routes must gate it.
pub async fn delete_post(db: &dyn Db, site_id: i64, id: i64) -> Result<bool, WordlessError> {
    find_post_by_id(db, site_id, id)
        .await?
        .ok_or_else(|| not_found(&format!("Post {id} not found.")))?;
    let changes = delete_post_row(db, site_id, id).await?;
    Ok(changes > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_shapes() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  Café & Bar  "), "cafe-bar");
        assert_eq!(slugify("a--b___c"), "a-b-c");
        assert_eq!(slugify("2026 Report: Q4"), "2026-report-q4");
        assert_eq!(slugify(""), "post");
        assert_eq!(slugify("!!!"), "post");
        // Truncates to 200 chars.
        assert_eq!(slugify(&"a".repeat(300)).len(), 200);
    }

    #[test]
    fn status_round_trip() {
        assert_eq!(PostStatus::from_str("publish"), Some(PostStatus::Publish));
        assert_eq!(PostStatus::from_str("bogus"), None);
        assert!(is_post_status("trash"));
        assert!(!is_post_status("future"));
    }

    #[test]
    fn post_serialises_camel_case() {
        let post = Post {
            id: 1,
            author_id: 2,
            date: "2026-11-19T00:00:00.000Z".to_string(),
            date_gmt: "2026-11-19T00:00:00.000Z".to_string(),
            content: "Body".to_string(),
            title: "Hi".to_string(),
            excerpt: "".to_string(),
            status: PostStatus::Draft,
            comment_status: "open".to_string(),
            ping_status: "open".to_string(),
            password: "".to_string(),
            slug: "hi".to_string(),
            modified: "2026-11-19T00:00:00.000Z".to_string(),
            modified_gmt: "2026-11-19T00:00:00.000Z".to_string(),
            content_filtered: "".to_string(),
            parent_id: 0,
            guid: "".to_string(),
            menu_order: 0,
            post_type: "post".to_string(),
            mime_type: "".to_string(),
            comment_count: 0,
            site_id: 1,
        };
        let v = serde_json::to_value(&post).unwrap();
        assert_eq!(v["authorId"], 2);
        assert_eq!(v["type"], "post");
        assert_eq!(v["status"], "draft");
        assert_eq!(v["commentCount"], 0);
    }
}
