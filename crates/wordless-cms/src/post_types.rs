//! Custom post types (NEW in v1.3 / P2 — explicit requirement).
//!
//! v1's `register_post_type` (`src/compat/wordpress.ts:218`) is an
//! in-memory stub only: not persisted, no admin UI, no rewrite rules.
//! This module makes CPTs persistent WITHOUT a new migration: definitions
//! are stored as JSON in `wp_options` under the key `wl_post_types`
//! (the same JSON-values rule as v1 options — never PHP-serialized).
//!
//! Option key: `wl_post_types`
//! JSON shape: a map from type slug to definition:
//! ```json
//! {
//!   "book": {
//!     "slug": "book",
//!     "label": "Books",
//!     "singularLabel": "Book",
//!     "description": "Long-form works.",
//!     "public": true,
//!     "hasArchive": true,
//!     "hierarchical": false,
//!     "supports": ["title", "editor", "excerpt", "thumbnail"],
//!     "rewriteSlug": "books",
//!     "taxonomies": ["category", "genre"],
//!     "builtin": false
//!   }
//! }
//! ```
//! Built-in types (`post`, `page`) are synthesised, never stored, and
//! are returned by `list_post_types` / `get_post_type` with
//! `"builtin": true`. They cannot be registered, updated, or deleted.
//!
//! Rewrite rules (this module's port of the v1 permalink logic for a
//! type — v1 had none, so these follow the WordPress defaults):
//!
//! - single: `/{rewriteSlug}/{post slug}` (e.g. `/books/dune`)
//! - archive: `/{rewriteSlug}` when `hasArchive` (e.g. `/books`)
//!
//! `rewriteSlug` defaults to the type slug. Pages and posts keep the
//! v1 permalink structures untouched.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, conflict, not_found, WordlessError};

use crate::posts::Post;

pub const POST_TYPES_OPTION: &str = "wl_post_types";

/// Slugs WordPress reserves for built-in types; registering one is a 400.
pub const RESERVED_POST_TYPES: &[&str] = &[
    "post",
    "page",
    "attachment",
    "revision",
    "nav_menu_item",
    "custom_css",
    "customize_changeset",
    "oembed_cache",
    "user_request",
    "wp_block",
    "wp_template",
    "wp_template_part",
    "wp_global_styles",
    "wp_navigation",
];

/// A custom post type definition, serialised with camelCase field names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostType {
    pub slug: String,
    pub label: String,
    pub singular_label: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub public: bool,
    #[serde(default = "default_true")]
    pub has_archive: bool,
    #[serde(default)]
    pub hierarchical: bool,
    #[serde(default)]
    pub supports: Vec<String>,
    pub rewrite_slug: String,
    #[serde(default)]
    pub taxonomies: Vec<String>,
    #[serde(default)]
    pub builtin: bool,
}

fn default_true() -> bool {
    true
}

impl PostType {
    fn builtin_post() -> PostType {
        PostType {
            slug: "post".to_string(),
            label: "Posts".to_string(),
            singular_label: "Post".to_string(),
            description: "Blog posts.".to_string(),
            public: true,
            has_archive: true,
            hierarchical: false,
            supports: vec![
                "title".to_string(),
                "editor".to_string(),
                "excerpt".to_string(),
                "thumbnail".to_string(),
                "author".to_string(),
                "comments".to_string(),
                "revisions".to_string(),
            ],
            rewrite_slug: "post".to_string(),
            taxonomies: vec!["category".to_string(), "tag".to_string()],
            builtin: true,
        }
    }

    fn builtin_page() -> PostType {
        PostType {
            slug: "page".to_string(),
            label: "Pages".to_string(),
            singular_label: "Page".to_string(),
            description: "Static pages.".to_string(),
            public: true,
            has_archive: false,
            hierarchical: true,
            supports: vec![
                "title".to_string(),
                "editor".to_string(),
                "excerpt".to_string(),
                "thumbnail".to_string(),
                "author".to_string(),
                "revisions".to_string(),
                "page-attributes".to_string(),
            ],
            rewrite_slug: "page".to_string(),
            taxonomies: vec![],
            builtin: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RegisterPostTypeInput {
    pub slug: String,
    pub label: String,
    pub singular_label: Option<String>,
    pub description: Option<String>,
    pub public: Option<bool>,
    pub has_archive: Option<bool>,
    pub hierarchical: Option<bool>,
    pub supports: Option<Vec<String>>,
    pub rewrite_slug: Option<String>,
    pub taxonomies: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdatePostTypeInput {
    pub label: Option<String>,
    pub singular_label: Option<String>,
    pub description: Option<String>,
    pub public: Option<bool>,
    pub has_archive: Option<bool>,
    pub hierarchical: Option<bool>,
    pub supports: Option<Vec<String>>,
    pub rewrite_slug: Option<String>,
    pub taxonomies: Option<Vec<String>>,
}

fn validate_type_slug(slug: &str) -> Result<String, WordlessError> {
    let s = slug.to_lowercase();
    let valid = !s.is_empty()
        && s.len() <= 20
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(bad_request(
            "Field \"slug\" must be a lowercase post-type key (letters, digits, underscore), at most 20 characters.",
        ));
    }
    if RESERVED_POST_TYPES.contains(&s.as_str()) {
        return Err(bad_request(&format!(
            "Post type \"{s}\" is reserved for a built-in type."
        )));
    }
    Ok(s)
}

fn validate_label(label: &str, field: &str) -> Result<String, WordlessError> {
    let l = label.trim().to_string();
    if l.is_empty() || l.len() > 200 {
        return Err(bad_request(&format!(
            "Field \"{field}\" is required and must be at most 200 characters."
        )));
    }
    Ok(l)
}

fn validate_supports(supports: &[String]) -> Result<Vec<String>, WordlessError> {
    if supports.len() > 20 {
        return Err(bad_request(
            "Field \"supports\" must have at most 20 entries.",
        ));
    }
    let mut out = Vec::new();
    for s in supports {
        let v = s.trim().to_lowercase();
        if v.is_empty()
            || !v
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(bad_request(
                "Field \"supports\" entries must be lowercase keys (letters, digits, hyphen, underscore).",
            ));
        }
        out.push(v);
    }
    Ok(out)
}

fn validate_taxonomy_keys(keys: &[String]) -> Result<Vec<String>, WordlessError> {
    let mut out = Vec::new();
    for k in keys {
        let v = k.trim().to_lowercase();
        if v.is_empty()
            || !v
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(bad_request(
                "Field \"taxonomies\" entries must be lowercase keys (letters, digits, underscore).",
            ));
        }
        out.push(v);
    }
    Ok(out)
}

async fn load_map(db: &dyn Db, site_id: i64) -> Result<BTreeMap<String, PostType>, WordlessError> {
    // P12: the registry is a per-site option.
    wordless_multisite::get_site_option::<BTreeMap<String, PostType>>(db, site_id, POST_TYPES_OPTION, BTreeMap::new()).await
}

async fn save_map(db: &dyn Db, site_id: i64, map: &BTreeMap<String, PostType>) -> Result<(), WordlessError> {
    wordless_multisite::set_site_option(db, site_id, POST_TYPES_OPTION, map).await
}

/// Register a new custom post type. Fails with 409 `post_type_exists`
/// when the slug is already registered.
pub async fn register_post_type(
    db: &dyn Db,
    site_id: i64,
    input: RegisterPostTypeInput,
) -> Result<PostType, WordlessError> {
    let slug = validate_type_slug(&input.slug)?;
    let label = validate_label(&input.label, "label")?;
    let singular_label = match &input.singular_label {
        Some(s) => validate_label(s, "singularLabel")?,
        None => label.clone(),
    };
    let description = match &input.description {
        Some(d) => {
            if d.len() > 1000 {
                return Err(bad_request(
                    "Field \"description\" must be at most 1000 characters.",
                ));
            }
            d.clone()
        }
        None => String::new(),
    };
    let supports = validate_supports(&input.supports.unwrap_or_default())?;
    let taxonomies = validate_taxonomy_keys(&input.taxonomies.unwrap_or_default())?;
    let rewrite_slug = match &input.rewrite_slug {
        Some(r) => {
            let s = crate::posts::slugify(r);
            if s.is_empty() || s == "post" {
                slug.clone()
            } else {
                s
            }
        }
        None => slug.clone(),
    };
    let mut map = load_map(db, site_id).await?;
    if map.contains_key(&slug) {
        return Err(conflict(
            &format!("Post type \"{slug}\" already exists."),
            "post_type_exists",
        ));
    }
    let def = PostType {
        slug: slug.clone(),
        label,
        singular_label,
        description,
        public: input.public.unwrap_or(true),
        has_archive: input.has_archive.unwrap_or(true),
        hierarchical: input.hierarchical.unwrap_or(false),
        supports,
        rewrite_slug,
        taxonomies,
        builtin: false,
    };
    map.insert(slug, def.clone());
    save_map(db, site_id, &map).await?;
    Ok(def)
}

/// All post types: built-ins (`post`, `page`) first, then registered
/// custom types sorted by slug.
pub async fn list_post_types(db: &dyn Db, site_id: i64) -> Result<Vec<PostType>, WordlessError> {
    let map = load_map(db, site_id).await?;
    let mut out = vec![PostType::builtin_post(), PostType::builtin_page()];
    out.extend(map.values().cloned());
    Ok(out)
}

/// One post type, built-ins included. `None` when unknown.
pub async fn find_post_type(db: &dyn Db, site_id: i64, slug: &str) -> Result<Option<PostType>, WordlessError> {
    let s = slug.to_lowercase();
    if s == "post" {
        return Ok(Some(PostType::builtin_post()));
    }
    if s == "page" {
        return Ok(Some(PostType::builtin_page()));
    }
    let map = load_map(db, site_id).await?;
    Ok(map.get(&s).cloned())
}

pub async fn get_post_type(db: &dyn Db, site_id: i64, slug: &str) -> Result<PostType, WordlessError> {
    find_post_type(db, site_id, slug)
        .await?
        .ok_or_else(|| not_found(&format!("Post type \"{slug}\" not found.")))
}

pub async fn update_post_type(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
    patch: UpdatePostTypeInput,
) -> Result<PostType, WordlessError> {
    let s = slug.to_lowercase();
    if s == "post" || s == "page" {
        return Err(bad_request(&format!(
            "Built-in post type \"{s}\" cannot be updated."
        )));
    }
    let mut map = load_map(db, site_id).await?;
    let mut def = map
        .get(&s)
        .cloned()
        .ok_or_else(|| not_found(&format!("Post type \"{s}\" not found.")))?;
    if let Some(label) = &patch.label {
        def.label = validate_label(label, "label")?;
    }
    if let Some(singular) = &patch.singular_label {
        def.singular_label = validate_label(singular, "singularLabel")?;
    }
    if let Some(description) = &patch.description {
        if description.len() > 1000 {
            return Err(bad_request(
                "Field \"description\" must be at most 1000 characters.",
            ));
        }
        def.description = description.clone();
    }
    if let Some(public) = patch.public {
        def.public = public;
    }
    if let Some(has_archive) = patch.has_archive {
        def.has_archive = has_archive;
    }
    if let Some(hierarchical) = patch.hierarchical {
        def.hierarchical = hierarchical;
    }
    if let Some(supports) = &patch.supports {
        def.supports = validate_supports(supports)?;
    }
    if let Some(rewrite_slug) = &patch.rewrite_slug {
        let r = crate::posts::slugify(rewrite_slug);
        def.rewrite_slug = if r.is_empty() || r == "post" {
            s.clone()
        } else {
            r
        };
    }
    if let Some(taxonomies) = &patch.taxonomies {
        def.taxonomies = validate_taxonomy_keys(taxonomies)?;
    }
    map.insert(s, def.clone());
    save_map(db, site_id, &map).await?;
    Ok(def)
}

/// Delete a custom post type. Built-ins cannot be deleted; a type that
/// still has posts cannot be deleted (409 `post_type_in_use`).
pub async fn delete_post_type(db: &dyn Db, site_id: i64, slug: &str) -> Result<bool, WordlessError> {
    let s = slug.to_lowercase();
    if s == "post" || s == "page" {
        return Err(bad_request(&format!(
            "Built-in post type \"{s}\" cannot be deleted."
        )));
    }
    let mut map = load_map(db, site_id).await?;
    if !map.contains_key(&s) {
        return Err(not_found(&format!("Post type \"{s}\" not found.")));
    }
    let row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS total FROM wp_posts WHERE post_type = ? AND site_id = ?")
        .bind(vec![Param::Text(s.clone()), Param::Integer(site_id)])
        .first()
        .await?;
    let count = crate::common::count_total(&row);
    if count > 0 {
        return Err(conflict(
            &format!("Post type \"{s}\" still has {count} post(s); delete or reassign them first."),
            "post_type_in_use",
        ));
    }
    map.remove(&s);
    save_map(db, site_id, &map).await?;
    Ok(true)
}

// ---------------------------------------------------------------- rewrite rules

/// Single-post URL for a post of this type: `/{rewriteSlug}/{slug}`.
pub fn post_type_single_url(def: &PostType, post: &Post) -> String {
    format!("/{}/{}", def.rewrite_slug, post.slug)
}

/// Archive URL for the type, or `None` when `hasArchive` is false.
pub fn post_type_archive_url(def: &PostType) -> Option<String> {
    if def.has_archive {
        Some(format!("/{}", def.rewrite_slug))
    } else {
        None
    }
}

/// Whether a registered type's rewrite slug matches `segment`.
pub async fn find_post_type_by_rewrite(
    db: &dyn Db,
    site_id: i64,
    segment: &str,
) -> Result<Option<PostType>, WordlessError> {
    let map = load_map(db, site_id).await?;
    Ok(map
        .values()
        .find(|def| def.rewrite_slug == segment)
        .cloned())
}
