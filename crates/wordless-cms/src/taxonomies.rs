//! Custom taxonomies (NEW in v1.3 / P2 — explicit requirement).
//!
//! v1's `register_taxonomy` (`src/compat/wordpress.ts:221`) is an
//! in-memory stub only: not persisted, no term UI. This module makes
//! taxonomies persistent WITHOUT a new migration: definitions are
//! stored as JSON in `wp_options` under the key `wl_taxonomies` (the
//! same JSON-values rule as v1 options).
//!
//! Option key: `wl_taxonomies`
//! JSON shape: a map from taxonomy slug to definition:
//! ```json
//! {
//!   "genre": {
//!     "slug": "genre",
//!     "label": "Genres",
//!     "singularLabel": "Genre",
//!     "description": "Book genres.",
//!     "hierarchical": true,
//!     "public": true,
//!     "objectTypes": ["book"],
//!     "rewriteSlug": "genre",
//!     "builtin": false
//!   }
//! }
//! ```
//! Built-in taxonomies (`category`, `tag`, `nav_menu`) are synthesised,
//! never stored, and are returned by `list_taxonomies` / `get_taxonomy`
//! with `"builtin": true`. They cannot be registered, updated, or deleted.
//!
//! Terms themselves live in the normal `wp_terms` / `wp_term_taxonomy` /
//! `wp_term_relationships` tables and are managed entirely through the
//! `crate::terms` service, which accepts any lowercase taxonomy key —
//! so term assignment for a custom taxonomy works the moment the
//! taxonomy is registered, with no extra code path.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, conflict, not_found, WordlessError};


pub const TAXONOMIES_OPTION: &str = "wl_taxonomies";

/// Slugs WordPress reserves for built-in taxonomies; registering one is a 400.
pub const RESERVED_TAXONOMIES: &[&str] = &[
    "category",
    "tag",
    "nav_menu",
    "link_category",
    "post_format",
];

/// A taxonomy definition, serialised with camelCase field names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Taxonomy {
    pub slug: String,
    pub label: String,
    pub singular_label: String,
    pub description: String,
    #[serde(default)]
    pub hierarchical: bool,
    #[serde(default = "default_true")]
    pub public: bool,
    #[serde(default)]
    pub object_types: Vec<String>,
    pub rewrite_slug: String,
    #[serde(default)]
    pub builtin: bool,
}

fn default_true() -> bool {
    true
}

impl Taxonomy {
    fn builtin_category() -> Taxonomy {
        Taxonomy {
            slug: "category".to_string(),
            label: "Categories".to_string(),
            singular_label: "Category".to_string(),
            description: String::new(),
            hierarchical: true,
            public: true,
            object_types: vec!["post".to_string()],
            rewrite_slug: "category".to_string(),
            builtin: true,
        }
    }

    fn builtin_tag() -> Taxonomy {
        Taxonomy {
            slug: "tag".to_string(),
            label: "Tags".to_string(),
            singular_label: "Tag".to_string(),
            description: String::new(),
            hierarchical: false,
            public: true,
            object_types: vec!["post".to_string()],
            rewrite_slug: "tag".to_string(),
            builtin: true,
        }
    }

    fn builtin_nav_menu() -> Taxonomy {
        Taxonomy {
            slug: "nav_menu".to_string(),
            label: "Menus".to_string(),
            singular_label: "Menu".to_string(),
            description: String::new(),
            hierarchical: false,
            public: false,
            object_types: vec!["nav_menu_item".to_string()],
            rewrite_slug: "nav_menu".to_string(),
            builtin: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RegisterTaxonomyInput {
    pub slug: String,
    pub label: String,
    pub singular_label: Option<String>,
    pub description: Option<String>,
    pub hierarchical: Option<bool>,
    pub public: Option<bool>,
    pub object_types: Option<Vec<String>>,
    pub rewrite_slug: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateTaxonomyInput {
    pub label: Option<String>,
    pub singular_label: Option<String>,
    pub description: Option<String>,
    pub hierarchical: Option<bool>,
    pub public: Option<bool>,
    pub object_types: Option<Vec<String>>,
    pub rewrite_slug: Option<String>,
}

fn validate_taxonomy_slug(slug: &str) -> Result<String, WordlessError> {
    let s = slug.to_lowercase();
    let valid = !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(bad_request(
            "Field \"slug\" must be a lowercase taxonomy key (letters, digits, underscore), at most 32 characters.",
        ));
    }
    if RESERVED_TAXONOMIES.contains(&s.as_str()) {
        return Err(bad_request(&format!(
            "Taxonomy \"{s}\" is reserved for a built-in taxonomy."
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

fn validate_object_types(types: &[String]) -> Result<Vec<String>, WordlessError> {
    if types.len() > 20 {
        return Err(bad_request(
            "Field \"objectTypes\" must have at most 20 entries.",
        ));
    }
    let mut out = Vec::new();
    for t in types {
        let v = t.trim().to_lowercase();
        if v.is_empty()
            || !v
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(bad_request(
                "Field \"objectTypes\" entries must be lowercase keys (letters, digits, underscore).",
            ));
        }
        out.push(v);
    }
    Ok(out)
}

async fn load_map(db: &dyn Db, site_id: i64) -> Result<BTreeMap<String, Taxonomy>, WordlessError> {
    // P12: the registry is a per-site option.
    wordless_multisite::get_site_option::<BTreeMap<String, Taxonomy>>(db, site_id, TAXONOMIES_OPTION, BTreeMap::new()).await
}

async fn save_map(db: &dyn Db, site_id: i64, map: &BTreeMap<String, Taxonomy>) -> Result<(), WordlessError> {
    wordless_multisite::set_site_option(db, site_id, TAXONOMIES_OPTION, map).await
}

/// Register a new custom taxonomy. Fails with 409 `taxonomy_exists`
/// when the slug is already registered.
pub async fn register_taxonomy(
    db: &dyn Db,
    site_id: i64,
    input: RegisterTaxonomyInput,
) -> Result<Taxonomy, WordlessError> {
    let slug = validate_taxonomy_slug(&input.slug)?;
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
    let object_types = validate_object_types(&input.object_types.unwrap_or_default())?;
    let rewrite_slug = match &input.rewrite_slug {
        Some(r) => {
            let s = crate::posts::slugify(r);
            if s.is_empty() {
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
            &format!("Taxonomy \"{slug}\" already exists."),
            "taxonomy_exists",
        ));
    }
    let def = Taxonomy {
        slug: slug.clone(),
        label,
        singular_label,
        description,
        hierarchical: input.hierarchical.unwrap_or(false),
        public: input.public.unwrap_or(true),
        object_types,
        rewrite_slug,
        builtin: false,
    };
    map.insert(slug, def.clone());
    save_map(db, site_id, &map).await?;
    Ok(def)
}

/// All taxonomies: built-ins (`category`, `tag`, `nav_menu`) first,
/// then registered custom taxonomies sorted by slug.
pub async fn list_taxonomies(db: &dyn Db, site_id: i64) -> Result<Vec<Taxonomy>, WordlessError> {
    let map = load_map(db, site_id).await?;
    let mut out = vec![
        Taxonomy::builtin_category(),
        Taxonomy::builtin_tag(),
        Taxonomy::builtin_nav_menu(),
    ];
    out.extend(map.values().cloned());
    Ok(out)
}

/// One taxonomy, built-ins included. `None` when unknown.
pub async fn find_taxonomy(db: &dyn Db, site_id: i64, slug: &str) -> Result<Option<Taxonomy>, WordlessError> {
    let s = slug.to_lowercase();
    match s.as_str() {
        "category" => return Ok(Some(Taxonomy::builtin_category())),
        "tag" => return Ok(Some(Taxonomy::builtin_tag())),
        "nav_menu" => return Ok(Some(Taxonomy::builtin_nav_menu())),
        _ => {}
    }
    let map = load_map(db, site_id).await?;
    Ok(map.get(&s).cloned())
}

pub async fn get_taxonomy(db: &dyn Db, site_id: i64, slug: &str) -> Result<Taxonomy, WordlessError> {
    find_taxonomy(db, site_id, slug)
        .await?
        .ok_or_else(|| not_found(&format!("Taxonomy \"{slug}\" not found.")))
}

/// Whether a taxonomy is known (built-in or registered). The terms
/// service does not call this — v1 never validated taxonomy existence
/// — but coordinators can use it to guard admin-only registration flows.
pub async fn taxonomy_exists(db: &dyn Db, site_id: i64, slug: &str) -> Result<bool, WordlessError> {
    Ok(find_taxonomy(db, site_id, slug).await?.is_some())
}

pub async fn update_taxonomy(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
    patch: UpdateTaxonomyInput,
) -> Result<Taxonomy, WordlessError> {
    let s = slug.to_lowercase();
    if RESERVED_TAXONOMIES.contains(&s.as_str()) {
        return Err(bad_request(&format!(
            "Built-in taxonomy \"{s}\" cannot be updated."
        )));
    }
    let mut map = load_map(db, site_id).await?;
    let mut def = map
        .get(&s)
        .cloned()
        .ok_or_else(|| not_found(&format!("Taxonomy \"{s}\" not found.")))?;
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
    if let Some(hierarchical) = patch.hierarchical {
        def.hierarchical = hierarchical;
    }
    if let Some(public) = patch.public {
        def.public = public;
    }
    if let Some(object_types) = &patch.object_types {
        def.object_types = validate_object_types(object_types)?;
    }
    if let Some(rewrite_slug) = &patch.rewrite_slug {
        let r = crate::posts::slugify(rewrite_slug);
        def.rewrite_slug = if r.is_empty() { s.clone() } else { r };
    }
    map.insert(s, def.clone());
    save_map(db, site_id, &map).await?;
    Ok(def)
}

/// Delete a custom taxonomy. Built-ins cannot be deleted; a taxonomy
/// that still has terms cannot be deleted (409 `taxonomy_in_use`).
/// Deleting the taxonomy does NOT delete its terms' posts — it only
/// refuses while terms exist, so no content is orphaned silently.
pub async fn delete_taxonomy(db: &dyn Db, site_id: i64, slug: &str) -> Result<bool, WordlessError> {
    let s = slug.to_lowercase();
    if RESERVED_TAXONOMIES.contains(&s.as_str()) {
        return Err(bad_request(&format!(
            "Built-in taxonomy \"{s}\" cannot be deleted."
        )));
    }
    let mut map = load_map(db, site_id).await?;
    if !map.contains_key(&s) {
        return Err(not_found(&format!("Taxonomy \"{s}\" not found.")));
    }
    let row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS total FROM wp_term_taxonomy WHERE taxonomy = ? AND site_id = ?")
        .bind(vec![Param::Text(s.clone()), Param::Integer(site_id)])
        .first()
        .await?;
    let count = crate::common::count_total(&row);
    if count > 0 {
        return Err(conflict(
            &format!("Taxonomy \"{s}\" still has {count} term(s); delete them first."),
            "taxonomy_in_use",
        ));
    }
    map.remove(&s);
    save_map(db, site_id, &map).await?;
    Ok(true)
}

/// Archive URL for a taxonomy: `/category/{slug}`, `/tag/{slug}`, or
/// `/{rewriteSlug}/{slug}` for a custom taxonomy.
pub fn taxonomy_archive_url(def: &Taxonomy, term_slug: &str) -> String {
    crate::permalinks::term_url(&def.rewrite_slug, term_slug)
}

/// Find a registered (non-built-in) taxonomy by its rewrite slug.
/// Built-ins are handled by `permalinks::resolve_path` step 2.
pub async fn find_taxonomy_by_rewrite(
    db: &dyn Db,
    site_id: i64,
    segment: &str,
) -> Result<Option<Taxonomy>, WordlessError> {
    let map = load_map(db, site_id).await?;
    Ok(map
        .values()
        .find(|def| def.rewrite_slug == segment)
        .cloned())
}
