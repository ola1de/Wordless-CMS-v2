//! Generic `wp_postmeta` helpers.
//!
//! Both `wordless-cms` (menus) and `wordless-seo` (its `_wl_seo_*`
//! fields) need postmeta access, so the upsert/get helpers live here
//! rather than in the menus repo. Same SQL as v1 `menus/repo.ts`
//! (`setPostMeta` / `getPostMetaMap`); the SEO fields themselves are
//! NOT implemented here — that is `wordless-seo`'s scope.

use std::collections::HashMap;

use wordless_db::row::row_text;
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};

/// Upsert one meta row (UPDATE first, INSERT when nothing matched).
pub async fn set_post_meta(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    key: &str,
    value: &str,
) -> Result<(), WordlessError> {
    let meta = db
        .prepare("UPDATE wp_postmeta SET meta_value = ? WHERE post_id = ? AND meta_key = ? AND site_id = ?")
        .bind(vec![
            Param::Text(value.to_string()),
            Param::Integer(post_id),
            Param::Text(key.to_string()),
            Param::Integer(site_id),
        ])
        .run()
        .await?;
    if meta.changes == 0 {
        db.prepare("INSERT INTO wp_postmeta (site_id, post_id, meta_key, meta_value) VALUES (?, ?, ?, ?)")
            .bind(vec![
                Param::Integer(site_id),
                Param::Integer(post_id),
                Param::Text(key.to_string()),
                Param::Text(value.to_string()),
            ])
            .run()
            .await?;
    }
    Ok(())
}

/// One meta value, or `None` when the key is absent.
pub async fn get_post_meta(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    key: &str,
) -> Result<Option<String>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT meta_value FROM wp_postmeta WHERE post_id = ? AND meta_key = ? AND site_id = ? LIMIT 1")
        .bind(vec![
            Param::Integer(post_id),
            Param::Text(key.to_string()),
            Param::Integer(site_id),
        ])
        .first()
        .await?;
    Ok(row.map(|r| row_text(&r, "meta_value")))
}

/// All meta rows for a post as a key → value map.
pub async fn get_post_meta_map(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
) -> Result<HashMap<String, String>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare("SELECT meta_key, meta_value FROM wp_postmeta WHERE post_id = ? AND site_id = ?")
        .bind(vec![Param::Integer(post_id), Param::Integer(site_id)])
        .all()
        .await?;
    let mut map = HashMap::new();
    for row in &rows {
        map.insert(row_text(row, "meta_key"), row_text(row, "meta_value"));
    }
    Ok(map)
}

/// Delete one meta key for a post. Returns `true` when a row was removed.
pub async fn delete_post_meta(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    key: &str,
) -> Result<bool, WordlessError> {
    let meta = db
        .prepare("DELETE FROM wp_postmeta WHERE post_id = ? AND meta_key = ? AND site_id = ?")
        .bind(vec![
            Param::Integer(post_id),
            Param::Text(key.to_string()),
            Param::Integer(site_id),
        ])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

/// Apply a set/delete patch to a post's meta rows (P4,
/// `PUT /api/posts/:id/meta`). Each entry is `(key, value)`: `Some`
/// sets the key, `None` deletes it. Returns the post's full meta map
/// after the patch.
///
/// Rules (all 400 unless noted):
/// * the post must exist (404);
/// * at most 200 entries per call;
/// * keys must be non-empty, at most 255 chars (the `wp_postmeta`
///   column width), with no control characters;
/// * `_wl_publish_at` is rejected — schedules go through `publishAt`
///   on the post, which validates the ISO time and forces a future
///   post to draft; a raw meta write would bypass both;
/// * `_thumbnail_id` values must be valid attachment ids (same rule
///   as `featuredImageId` on create/update), so the two write paths
///   cannot disagree about what a thumbnail may point at.
pub async fn update_post_meta(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    entries: &[(String, Option<String>)],
) -> Result<HashMap<String, String>, WordlessError> {
    if entries.len() > 200 {
        return Err(bad_request(
            "At most 200 meta entries may be set in one request.",
        ));
    }
    crate::posts::find_post_by_id(db, site_id, post_id)
        .await?
        .ok_or_else(|| not_found(&format!("Post {post_id} not found.")))?;
    for (key, value) in entries {
        if key.is_empty() || key.len() > 255 || key.chars().any(|c| c.is_control()) {
            return Err(bad_request(
                "Meta keys must be non-empty, at most 255 characters, with no control characters.",
            ));
        }
        if key == crate::scheduling::PUBLISH_AT_META_KEY {
            return Err(bad_request(
                "Meta key \"_wl_publish_at\" is managed by the scheduling flow — use \"publishAt\" on the post instead.",
            ));
        }
        if key == crate::posts::THUMBNAIL_META_KEY {
            if let Some(raw) = value {
                let fid: i64 = raw.trim().parse().map_err(|_| {
                    bad_request("Meta key \"_thumbnail_id\" must be a media attachment id.")
                })?;
                crate::posts::validate_featured_image_id(db, site_id, fid).await?;
            }
        }
    }
    for (key, value) in entries {
        match value {
            Some(v) => set_post_meta(db, site_id, post_id, key, v).await?,
            None => {
                delete_post_meta(db, site_id, post_id, key).await?;
            }
        }
    }
    get_post_meta_map(db, site_id, post_id).await
}
