//! Terms — port of v1 `src/modules/terms/` (service.ts, repo.ts, types.ts).
//!
//! Categories (hierarchical) and tags (flat) on the WordPress tables
//! `wp_terms` / `wp_term_taxonomy` / `wp_term_relationships`.
//!
//! Rules (v1):
//! - Slugs are unique per taxonomy (suffix -2, -3, ...). The same slug
//!   may exist in different taxonomies.
//! - Only categories take a parent; tags are flat (a parent on a tag is a 400).
//! - Term counts are maintained on every assign/remove/delete.
//! - Deleting a category reparents its children to the deleted term's parent.

use serde::Serialize;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};

use crate::common::{count_total, page_of, per_page_of, total_pages};
use crate::posts::{find_post_by_id, row_to_post, Post};

/// A term, serialised with the v1 camelCase field names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Term {
    pub id: i64,
    /// `wp_term_taxonomy.term_taxonomy_id` — the id relationships point at.
    pub term_taxonomy_id: i64,
    pub name: String,
    pub slug: String,
    pub taxonomy: String,
    pub description: String,
    /// Parent term id (0 = top level). Only categories are hierarchical.
    pub parent_id: i64,
    /// Number of posts assigned (maintained by the service).
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<Term>>,
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
        children: None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct CreateTermInput {
    pub name: String,
    pub slug: Option<String>,
    pub taxonomy: String,
    pub description: Option<String>,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateTermInput {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct ListTermsOptions {
    pub parent_id: Option<i64>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TermListResult {
    pub terms: Vec<Term>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

const TERM_COLUMNS: &str =
    "t.term_id, tt.term_taxonomy_id, t.name, t.slug, tt.taxonomy, tt.description, tt.parent, tt.count";
const TERM_JOIN: &str = "FROM wp_terms t JOIN wp_term_taxonomy tt ON tt.term_id = t.term_id";

pub fn validate_taxonomy(taxonomy: &str) -> Result<String, WordlessError> {
    let t = taxonomy.to_lowercase();
    let valid = !t.is_empty()
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(bad_request(
            "Field \"taxonomy\" must be a lowercase key (letters, digits, underscore).",
        ));
    }
    Ok(t)
}

// ---------------------------------------------------------------- repo

pub async fn insert_term_row(
    db: &dyn Db,
    site_id: i64,
    name: &str,
    slug: &str,
    taxonomy: &str,
    description: &str,
    parent_id: i64,
) -> Result<(i64, i64), WordlessError> {
    let term = db
        .prepare("INSERT INTO wp_terms (site_id, name, slug, term_group) VALUES (?, ?, ?, 0)")
        .bind(vec![
            Param::Integer(site_id),
            Param::Text(name.to_string()),
            Param::Text(slug.to_string()),
        ])
        .run()
        .await?;
    let term_id = term.last_row_id.unwrap_or(0);
    let taxonomy_meta = db
        .prepare("INSERT INTO wp_term_taxonomy (site_id, term_id, taxonomy, description, parent, count) VALUES (?, ?, ?, ?, ?, 0)")
        .bind(vec![
            Param::Integer(site_id),
            Param::Integer(term_id),
            Param::Text(taxonomy.to_string()),
            Param::Text(description.to_string()),
            Param::Integer(parent_id),
        ])
        .run()
        .await?;
    Ok((term_id, taxonomy_meta.last_row_id.unwrap_or(0)))
}

pub async fn find_term_by_taxonomy_id(
    db: &dyn Db,
    site_id: i64,
    term_taxonomy_id: i64,
) -> Result<Option<Term>, WordlessError> {
    let row: Option<Row> = db
        .prepare(&format!(
            "SELECT {TERM_COLUMNS} {TERM_JOIN} WHERE tt.term_taxonomy_id = ? AND tt.site_id = ? LIMIT 1"
        ))
        .bind(vec![Param::Integer(term_taxonomy_id), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(row.map(|r| row_to_term(&r)))
}

pub async fn find_term_by_slug(
    db: &dyn Db,
    site_id: i64,
    taxonomy: &str,
    slug: &str,
) -> Result<Option<Term>, WordlessError> {
    // P5 F12: the public tag archive is `/tag/<slug>` but the stored
    // taxonomy is `post_tag` (WordPress's built-in name). v1 queries
    // `taxonomy = 'tag'` and 404s every tag archive; v2 maps the
    // public name to the stored one.
    let stored = if taxonomy == "tag" {
        "post_tag"
    } else {
        taxonomy
    };
    let row: Option<Row> = db
        .prepare(&format!(
            "SELECT {TERM_COLUMNS} {TERM_JOIN} WHERE tt.taxonomy = ? AND t.slug = ? AND tt.site_id = ? LIMIT 1"
        ))
        .bind(vec![
            Param::Text(stored.to_string()),
            Param::Text(slug.to_string()),
            Param::Integer(site_id),
        ])
        .first()
        .await?;
    Ok(row.map(|r| row_to_term(&r)))
}

pub async fn find_term_by_id(
    db: &dyn Db,
    site_id: i64,
    term_id: i64,
    taxonomy: Option<&str>,
) -> Result<Option<Term>, WordlessError> {
    let mut sql = format!("SELECT {TERM_COLUMNS} {TERM_JOIN} WHERE t.term_id = ? AND t.site_id = ?");
    let mut params = vec![Param::Integer(term_id), Param::Integer(site_id)];
    if let Some(t) = taxonomy {
        sql.push_str(" AND tt.taxonomy = ?");
        params.push(Param::Text(t.to_string()));
    }
    sql.push_str(" LIMIT 1");
    let row: Option<Row> = db.prepare(&sql).bind(params).first().await?;
    Ok(row.map(|r| row_to_term(&r)))
}

async fn count_terms(
    db: &dyn Db,
    site_id: i64,
    taxonomy: &str,
    parent_id: Option<i64>,
) -> Result<i64, WordlessError> {
    let mut sql = format!("SELECT COUNT(*) AS total {TERM_JOIN} WHERE tt.taxonomy = ? AND tt.site_id = ?");
    let mut params = vec![Param::Text(taxonomy.to_string()), Param::Integer(site_id)];
    if let Some(p) = parent_id {
        sql.push_str(" AND tt.parent = ?");
        params.push(Param::Integer(p));
    }
    let row: Option<Row> = db.prepare(&sql).bind(params).first().await?;
    Ok(count_total(&row))
}

pub async fn list_term_rows(
    db: &dyn Db,
    site_id: i64,
    taxonomy: &str,
    parent_id: Option<i64>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Term>, WordlessError> {
    let mut sql = format!("SELECT {TERM_COLUMNS} {TERM_JOIN} WHERE tt.taxonomy = ? AND tt.site_id = ?");
    let mut params = vec![Param::Text(taxonomy.to_string()), Param::Integer(site_id)];
    if let Some(p) = parent_id {
        sql.push_str(" AND tt.parent = ?");
        params.push(Param::Integer(p));
    }
    sql.push_str(" ORDER BY t.name ASC LIMIT ? OFFSET ?");
    params.push(Param::Integer(limit));
    params.push(Param::Integer(offset));
    let rows: Vec<Row> = db.prepare(&sql).bind(params).all().await?;
    Ok(rows.iter().map(row_to_term).collect())
}

async fn update_term_row(
    db: &dyn Db,
    site_id: i64,
    term: &Term,
    name: Option<&str>,
    slug: Option<&str>,
    description: Option<&str>,
    parent_id: Option<i64>,
) -> Result<(), WordlessError> {
    if name.is_some() || slug.is_some() {
        db.prepare("UPDATE wp_terms SET name = ?, slug = ? WHERE term_id = ? AND site_id = ?")
            .bind(vec![
                Param::Text(name.unwrap_or(&term.name).to_string()),
                Param::Text(slug.unwrap_or(&term.slug).to_string()),
                Param::Integer(term.id),
                Param::Integer(site_id),
            ])
            .run()
            .await?;
    }
    if description.is_some() || parent_id.is_some() {
        db.prepare(
            "UPDATE wp_term_taxonomy SET description = ?, parent = ? WHERE term_taxonomy_id = ? AND site_id = ?",
        )
        .bind(vec![
            Param::Text(description.unwrap_or(&term.description).to_string()),
            Param::Integer(parent_id.unwrap_or(term.parent_id)),
            Param::Integer(term.term_taxonomy_id),
            Param::Integer(site_id),
        ])
        .run()
        .await?;
    }
    Ok(())
}

async fn delete_term_rows(db: &dyn Db, site_id: i64, term: &Term) -> Result<(), WordlessError> {
    db.prepare("DELETE FROM wp_term_relationships WHERE term_taxonomy_id = ? AND site_id = ?")
        .bind(vec![Param::Integer(term.term_taxonomy_id), Param::Integer(site_id)])
        .run()
        .await?;
    db.prepare("DELETE FROM wp_term_taxonomy WHERE term_taxonomy_id = ? AND site_id = ?")
        .bind(vec![Param::Integer(term.term_taxonomy_id), Param::Integer(site_id)])
        .run()
        .await?;
    // The wp_terms row is shared if another taxonomy still references it
    // on this site.
    let remaining: Option<Row> = db
        .prepare("SELECT COUNT(*) AS total FROM wp_term_taxonomy WHERE term_id = ? AND site_id = ?")
        .bind(vec![Param::Integer(term.id), Param::Integer(site_id)])
        .first()
        .await?;
    if count_total(&remaining) == 0 {
        db.prepare("DELETE FROM wp_terms WHERE term_id = ? AND site_id = ?")
            .bind(vec![Param::Integer(term.id), Param::Integer(site_id)])
            .run()
            .await?;
    }
    db.prepare("DELETE FROM wp_termmeta WHERE term_id = ? AND site_id = ?")
        .bind(vec![Param::Integer(term.id), Param::Integer(site_id)])
        .run()
        .await?;
    Ok(())
}

async fn reparent_children(
    db: &dyn Db,
    site_id: i64,
    taxonomy: &str,
    old_parent_term_id: i64,
    new_parent_term_id: i64,
) -> Result<(), WordlessError> {
    db.prepare("UPDATE wp_term_taxonomy SET parent = ? WHERE taxonomy = ? AND parent = ? AND site_id = ?")
        .bind(vec![
            Param::Integer(new_parent_term_id),
            Param::Text(taxonomy.to_string()),
            Param::Integer(old_parent_term_id),
            Param::Integer(site_id),
        ])
        .run()
        .await?;
    Ok(())
}

async fn count_relationships(db: &dyn Db, site_id: i64, term_taxonomy_id: i64) -> Result<i64, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS total FROM wp_term_relationships WHERE term_taxonomy_id = ? AND site_id = ?")
        .bind(vec![Param::Integer(term_taxonomy_id), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(count_total(&row))
}

async fn set_term_count(
    db: &dyn Db,
    site_id: i64,
    term_taxonomy_id: i64,
    count: i64,
) -> Result<(), WordlessError> {
    db.prepare("UPDATE wp_term_taxonomy SET count = ? WHERE term_taxonomy_id = ? AND site_id = ?")
        .bind(vec![
            Param::Integer(count),
            Param::Integer(term_taxonomy_id),
            Param::Integer(site_id),
        ])
        .run()
        .await?;
    Ok(())
}

async fn assign_term_to_post(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    term_taxonomy_id: i64,
) -> Result<(), WordlessError> {
    db.prepare("INSERT OR IGNORE INTO wp_term_relationships (site_id, object_id, term_taxonomy_id, term_order) VALUES (?, ?, ?, 0)")
        .bind(vec![Param::Integer(site_id), Param::Integer(post_id), Param::Integer(term_taxonomy_id)])
        .run()
        .await?;
    Ok(())
}

async fn remove_term_from_post(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    term_taxonomy_id: i64,
) -> Result<(), WordlessError> {
    db.prepare("DELETE FROM wp_term_relationships WHERE object_id = ? AND term_taxonomy_id = ? AND site_id = ?")
        .bind(vec![
            Param::Integer(post_id),
            Param::Integer(term_taxonomy_id),
            Param::Integer(site_id),
        ])
        .run()
        .await?;
    Ok(())
}

async fn clear_post_terms(db: &dyn Db, site_id: i64, post_id: i64, taxonomy: &str) -> Result<(), WordlessError> {
    db.prepare(
        "DELETE FROM wp_term_relationships
          WHERE object_id = ? AND site_id = ?
            AND term_taxonomy_id IN (SELECT term_taxonomy_id FROM wp_term_taxonomy WHERE taxonomy = ? AND site_id = ?)",
    )
    .bind(vec![
        Param::Integer(post_id),
        Param::Integer(site_id),
        Param::Text(taxonomy.to_string()),
        Param::Integer(site_id),
    ])
    .run()
    .await?;
    Ok(())
}

pub async fn find_post_terms(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    taxonomy: Option<&str>,
) -> Result<Vec<Term>, WordlessError> {
    let mut sql = format!(
        "SELECT {TERM_COLUMNS}
          {TERM_JOIN}
          JOIN wp_term_relationships tr ON tr.term_taxonomy_id = tt.term_taxonomy_id
          WHERE tr.object_id = ? AND tr.site_id = ?"
    );
    let mut params = vec![Param::Integer(post_id), Param::Integer(site_id)];
    if let Some(t) = taxonomy {
        sql.push_str(" AND tt.taxonomy = ?");
        params.push(Param::Text(t.to_string()));
    }
    sql.push_str(" ORDER BY t.name ASC");
    let rows: Vec<Row> = db.prepare(&sql).bind(params).all().await?;
    Ok(rows.iter().map(row_to_term).collect())
}

// ---------------------------------------------------------------- service

async fn ensure_unique_term_slug(
    db: &dyn Db,
    site_id: i64,
    taxonomy: &str,
    base: &str,
    exclude_term_id: Option<i64>,
) -> Result<String, WordlessError> {
    let mut candidate = base.to_string();
    let mut n = 2;
    loop {
        let existing = find_term_by_slug(db, site_id, taxonomy, &candidate).await?;
        match existing {
            Some(term) if exclude_term_id.is_none_or(|id| term.id != id) => {
                candidate = format!("{base}-{n}");
                n += 1;
            }
            _ => return Ok(candidate),
        }
    }
}

async fn recount(db: &dyn Db, site_id: i64, term_taxonomy_id: i64) -> Result<(), WordlessError> {
    let count = count_relationships(db, site_id, term_taxonomy_id).await?;
    set_term_count(db, site_id, term_taxonomy_id, count).await
}

pub async fn create_term(db: &dyn Db, site_id: i64, input: CreateTermInput) -> Result<Term, WordlessError> {
    let taxonomy = validate_taxonomy(&input.taxonomy)?;
    let name = input.name.trim().to_string();
    if name.is_empty() || name.len() > 200 {
        return Err(bad_request(
            "Field \"name\" is required and must be at most 200 characters.",
        ));
    }
    let parent_id = input.parent_id.unwrap_or(0);
    if taxonomy == "tag" && parent_id != 0 {
        return Err(bad_request(
            "Tags are flat: a tag cannot have a parent term.",
        ));
    }
    if parent_id != 0 {
        let parent = find_term_by_id(db, site_id, parent_id, Some(&taxonomy)).await?;
        if parent.is_none() {
            return Err(not_found(&format!(
                "Parent term {parent_id} not found in taxonomy \"{taxonomy}\"."
            )));
        }
    }
    let base_slug = match &input.slug {
        Some(s) => crate::posts::slugify(s),
        None => crate::posts::slugify(&name),
    };
    let slug = ensure_unique_term_slug(db, site_id, &taxonomy, &base_slug, None).await?;
    let (_term_id, term_taxonomy_id) = insert_term_row(
        db,
        site_id,
        &name,
        &slug,
        &taxonomy,
        &input.description.unwrap_or_default(),
        parent_id,
    )
    .await?;
    let created = find_term_by_taxonomy_id(db, site_id, term_taxonomy_id).await?;
    created.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the created term.",
        )
    })
}

pub async fn get_term(db: &dyn Db, site_id: i64, term_taxonomy_id: i64) -> Result<Term, WordlessError> {
    find_term_by_taxonomy_id(db, site_id, term_taxonomy_id)
        .await?
        .ok_or_else(|| not_found(&format!("Term {term_taxonomy_id} not found.")))
}

pub async fn get_term_by_slug(
    db: &dyn Db,
    site_id: i64,
    taxonomy: &str,
    slug: &str,
) -> Result<Term, WordlessError> {
    let t = validate_taxonomy(taxonomy)?;
    find_term_by_slug(db, site_id, &t, slug).await?.ok_or_else(|| {
        not_found(&format!(
            "Term \"{slug}\" not found in taxonomy \"{taxonomy}\"."
        ))
    })
}

pub async fn list_terms(
    db: &dyn Db,
    site_id: i64,
    taxonomy: &str,
    opts: ListTermsOptions,
) -> Result<TermListResult, WordlessError> {
    let t = validate_taxonomy(taxonomy)?;
    let page = page_of(opts.page);
    let per_page = per_page_of(opts.per_page);
    let total = count_terms(db, site_id, &t, opts.parent_id).await?;
    let total_pages = total_pages(total, per_page);
    let terms = list_term_rows(db, site_id, &t, opts.parent_id, per_page, (page - 1) * per_page).await?;
    Ok(TermListResult {
        terms,
        total,
        page,
        per_page,
        total_pages,
    })
}

/// Build a nested tree from a flat term list (children nested, sorted
/// by name — the rows arrive name-ordered from `list_term_rows`).
/// Pure function, unit-tested directly.
pub(crate) fn build_term_tree(all: &[Term], parent_id: i64) -> Vec<Term> {
    all.iter()
        .filter(|term| term.parent_id == parent_id)
        .map(|term| Term {
            children: Some(build_term_tree(all, term.id)),
            ..term.clone()
        })
        .collect()
}

/// Full category tree for a taxonomy (children nested, sorted by name).
pub async fn list_term_tree(db: &dyn Db, site_id: i64, taxonomy: &str) -> Result<Vec<Term>, WordlessError> {
    let t = validate_taxonomy(taxonomy)?;
    let all = list_term_rows(db, site_id, &t, None, 1000, 0).await?;
    Ok(build_term_tree(&all, 0))
}

pub async fn update_term(
    db: &dyn Db,
    site_id: i64,
    term_taxonomy_id: i64,
    patch: UpdateTermInput,
) -> Result<Term, WordlessError> {
    let term = find_term_by_taxonomy_id(db, site_id, term_taxonomy_id)
        .await?
        .ok_or_else(|| not_found(&format!("Term {term_taxonomy_id} not found.")))?;
    if term.taxonomy == "tag" && patch.parent_id.is_some_and(|p| p != 0) {
        return Err(bad_request(
            "Tags are flat: a tag cannot have a parent term.",
        ));
    }
    if let Some(parent_id) = patch.parent_id {
        if parent_id != 0 {
            if parent_id == term.id {
                return Err(bad_request("A term cannot be its own parent."));
            }
            let parent = find_term_by_id(db, site_id, parent_id, Some(&term.taxonomy)).await?;
            if parent.is_none() {
                return Err(not_found(&format!(
                    "Parent term {parent_id} not found in taxonomy \"{}\".",
                    term.taxonomy
                )));
            }
            // No cycles: the new parent must not be a descendant of this term.
            let mut cursor = parent;
            while let Some(c) = cursor {
                if c.parent_id == term.id {
                    return Err(bad_request(
                        "A term cannot be moved under its own descendant.",
                    ));
                }
                if c.parent_id == 0 {
                    break;
                }
                cursor = find_term_by_id(db, site_id, c.parent_id, Some(&term.taxonomy)).await?;
            }
        }
    }
    let mut next_name: Option<String> = None;
    let mut next_slug: Option<String> = None;
    let mut next_description: Option<String> = None;
    if let Some(name) = &patch.name {
        let name = name.trim().to_string();
        if name.is_empty() || name.len() > 200 {
            return Err(bad_request("Field \"name\" must be 1–200 characters."));
        }
        next_name = Some(name);
    }
    if let Some(slug) = &patch.slug {
        next_slug = Some(
            ensure_unique_term_slug(
                db,
                site_id,
                &term.taxonomy,
                &crate::posts::slugify(slug),
                Some(term.id),
            )
            .await?,
        );
    }
    if let Some(description) = &patch.description {
        next_description = Some(description.trim().to_string());
    }
    update_term_row(
        db,
        site_id,
        &term,
        next_name.as_deref(),
        next_slug.as_deref(),
        next_description.as_deref(),
        patch.parent_id,
    )
    .await?;
    let updated = find_term_by_taxonomy_id(db, site_id, term_taxonomy_id).await?;
    updated.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the updated term.",
        )
    })
}

pub async fn delete_term(db: &dyn Db, site_id: i64, term_taxonomy_id: i64) -> Result<bool, WordlessError> {
    let term = find_term_by_taxonomy_id(db, site_id, term_taxonomy_id)
        .await?
        .ok_or_else(|| not_found(&format!("Term {term_taxonomy_id} not found.")))?;
    // Children move up to the deleted term's parent (WordPress behaviour).
    reparent_children(db, site_id, &term.taxonomy, term.id, term.parent_id).await?;
    delete_term_rows(db, site_id, &term).await?;
    Ok(true)
}

pub async fn get_post_terms(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    taxonomy: Option<&str>,
) -> Result<Vec<Term>, WordlessError> {
    let post = find_post_by_id(db, site_id, post_id).await?;
    if post.is_none() {
        return Err(not_found(&format!("Post {post_id} not found.")));
    }
    let t = match taxonomy {
        Some(t) => Some(validate_taxonomy(t)?),
        None => None,
    };
    find_post_terms(db, site_id, post_id, t.as_deref()).await
}

/// Replace a post's terms in one taxonomy with exactly `term_ids`.
pub async fn set_post_terms(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    taxonomy: &str,
    term_ids: &[i64],
) -> Result<Vec<Term>, WordlessError> {
    let t = validate_taxonomy(taxonomy)?;
    let post = find_post_by_id(db, site_id, post_id).await?;
    if post.is_none() {
        return Err(not_found(&format!("Post {post_id} not found.")));
    }
    let mut terms: Vec<Term> = Vec::new();
    for term_taxonomy_id in term_ids {
        let term = find_term_by_taxonomy_id(db, site_id, *term_taxonomy_id)
            .await?
            .ok_or_else(|| not_found(&format!("Term {term_taxonomy_id} not found.")))?;
        if term.taxonomy != t {
            return Err(bad_request(&format!(
                "Term {term_taxonomy_id} is not in taxonomy \"{t}\"."
            )));
        }
        terms.push(term);
    }
    let previous = find_post_terms(db, site_id, post_id, Some(&t)).await?;
    clear_post_terms(db, site_id, post_id, &t).await?;
    for term in &terms {
        assign_term_to_post(db, site_id, post_id, term.term_taxonomy_id).await?;
    }
    let mut affected: std::collections::HashMap<i64, Term> = std::collections::HashMap::new();
    for term in previous.into_iter().chain(terms) {
        affected.insert(term.term_taxonomy_id, term);
    }
    for term in affected.values() {
        recount(db, site_id, term.term_taxonomy_id).await?;
    }
    find_post_terms(db, site_id, post_id, Some(&t)).await
}

pub async fn add_post_term(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    term_taxonomy_id: i64,
) -> Result<Vec<Term>, WordlessError> {
    let term = find_term_by_taxonomy_id(db, site_id, term_taxonomy_id)
        .await?
        .ok_or_else(|| not_found(&format!("Term {term_taxonomy_id} not found.")))?;
    let post = find_post_by_id(db, site_id, post_id).await?;
    if post.is_none() {
        return Err(not_found(&format!("Post {post_id} not found.")));
    }
    assign_term_to_post(db, site_id, post_id, term_taxonomy_id).await?;
    recount(db, site_id, term_taxonomy_id).await?;
    find_post_terms(db, site_id, post_id, Some(&term.taxonomy)).await
}

pub async fn remove_post_term(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    term_taxonomy_id: i64,
) -> Result<Vec<Term>, WordlessError> {
    let term = find_term_by_taxonomy_id(db, site_id, term_taxonomy_id)
        .await?
        .ok_or_else(|| not_found(&format!("Term {term_taxonomy_id} not found.")))?;
    let post = find_post_by_id(db, site_id, post_id).await?;
    if post.is_none() {
        return Err(not_found(&format!("Post {post_id} not found.")));
    }
    remove_term_from_post(db, site_id, post_id, term_taxonomy_id).await?;
    recount(db, site_id, term_taxonomy_id).await?;
    find_post_terms(db, site_id, post_id, Some(&term.taxonomy)).await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostsByTermResult {
    pub posts: Vec<Post>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// Published posts in a term (used by term archive pages).
pub async fn list_posts_by_term(
    db: &dyn Db,
    site_id: i64,
    term_taxonomy_id: i64,
    status: Option<&str>,
    page: Option<i64>,
    per_page: Option<i64>,
) -> Result<PostsByTermResult, WordlessError> {
    let term = find_term_by_taxonomy_id(db, site_id, term_taxonomy_id)
        .await?
        .ok_or_else(|| not_found(&format!("Term {term_taxonomy_id} not found.")))?;
    let _ = term;
    let page = page_of(page);
    let per_page = per_page_of(per_page);
    let status = status.unwrap_or("publish");
    let count_row: Option<Row> = db
        .prepare(
            "SELECT COUNT(*) AS total FROM wp_posts p
              JOIN wp_term_relationships tr ON tr.object_id = p.ID
              WHERE tr.term_taxonomy_id = ? AND p.post_status = ? AND p.site_id = ? AND tr.site_id = ?",
        )
        .bind(vec![
            Param::Integer(term_taxonomy_id),
            Param::Text(status.to_string()),
            Param::Integer(site_id),
            Param::Integer(site_id),
        ])
        .first()
        .await?;
    let total = count_total(&count_row);
    let total_pages = total_pages(total, per_page);
    let rows: Vec<Row> = db
        .prepare(
            "SELECT p.* FROM wp_posts p
              JOIN wp_term_relationships tr ON tr.object_id = p.ID
              WHERE tr.term_taxonomy_id = ? AND p.post_status = ? AND p.site_id = ? AND tr.site_id = ?
              ORDER BY p.post_date DESC, p.ID DESC LIMIT ? OFFSET ?",
        )
        .bind(vec![
            Param::Integer(term_taxonomy_id),
            Param::Text(status.to_string()),
            Param::Integer(site_id),
            Param::Integer(site_id),
            Param::Integer(per_page),
            Param::Integer((page - 1) * per_page),
        ])
        .all()
        .await?;
    Ok(PostsByTermResult {
        posts: rows.iter().map(row_to_post).collect(),
        total,
        page,
        per_page,
        total_pages,
    })
}
