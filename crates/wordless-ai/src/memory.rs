//! Website memory — port of v1 `src/ai/memory.ts` (Milestone 6).
//!
//! The site-knowledge store agents use as context. Facts are
//! re-ingested from the site's own data (title, tagline, published
//! posts, active theme, installed extensions); notes are short facts
//! an admin adds by hand. Memory NEVER contains secrets: only public
//! site content and admin-written notes go in, and `add_note` refuses
//! anything that looks like key material.
//!
//! Storage: `wl_ai_memory` (kind 'fact' | 'note', key, value,
//! source). Rebuild deletes stale auto facts first (admin notes are
//! never touched), then writes fresh ones. The secret check is
//! hand-rolled string matching (no `regex` crate in this workspace):
//! it mirrors v1's two patterns — a value starting with `sk-`,
//! `AKIA`, `-----BEGIN` or `api_key=`/`api-key=`, and any value
//! containing `password:` / `password=` followed by a value.

use serde::{Deserialize, Serialize};
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};

use crate::util::new_uuid_v4;

/// One memory entry, mirroring v1 `MemoryEntry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: i64,
    /// `"fact"` (rebuilt from the site) or `"note"` (admin-written).
    pub kind: String,
    pub key: String,
    pub value: String,
    pub source: String,
    pub updated_at: String,
}

/// Facts and notes, mirroring v1 `MemoryList`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryList {
    pub facts: Vec<MemoryEntry>,
    pub notes: Vec<MemoryEntry>,
}

/// Counts returned by a rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRebuildResult {
    pub facts: i64,
    pub notes: i64,
}

const MAX_POSTS_INGESTED: i64 = 100;
const MAX_NOTES_IN_CONTEXT: usize = 10;
const MAX_POSTS_IN_CONTEXT: usize = 20;

fn row_to_entry(row: &Row) -> MemoryEntry {
    MemoryEntry {
        id: row_int(row, "id"),
        kind: if row_text(row, "kind") == "note" {
            "note".to_string()
        } else {
            "fact".to_string()
        },
        key: row_text(row, "key"),
        value: row_text(row, "value"),
        source: row_text(row, "source"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// List all memory entries, facts first, then notes (v1 `listMemory`).
pub async fn list_memory(db: &dyn Db) -> Result<MemoryList, WordlessError> {
    let rows: Vec<Row> = db
        .prepare("SELECT * FROM wl_ai_memory ORDER BY kind ASC, key ASC LIMIT 500")
        .all::<Row>()
        .await?;
    let entries: Vec<MemoryEntry> = rows.iter().map(row_to_entry).collect();
    Ok(MemoryList {
        facts: entries
            .iter()
            .filter(|e| e.kind == "fact")
            .cloned()
            .collect(),
        notes: entries
            .iter()
            .filter(|e| e.kind == "note")
            .cloned()
            .collect(),
    })
}

/// List admin notes only (the command centre's notes view).
pub async fn list_memory_notes(db: &dyn Db) -> Result<Vec<MemoryEntry>, WordlessError> {
    Ok(list_memory(db).await?.notes)
}

/// True when a note looks like API-key / password / private-key
/// material. Port of v1's two regexes in `addNote`, without a regex
/// crate: `^(sk-|AKIA|-----BEGIN|api[_-]?key\s*=)` (case-insensitive,
/// anchored) and `password\s*[:=]\s*\S+` (case-insensitive, anywhere).
fn looks_like_secret(value: &str) -> bool {
    let lower = value.to_lowercase();
    if lower.starts_with("sk-") || lower.starts_with("akia") || lower.starts_with("-----begin") {
        return true;
    }
    for prefix in ["api_key", "api-key"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if rest.trim_start().starts_with('=') {
                return true;
            }
        }
    }
    // `password` followed by optional spaces, `:` or `=`, optional
    // spaces, then at least one non-space character.
    let mut search_from = 0;
    while let Some(pos) = lower[search_from..].find("password") {
        let after = &lower[search_from + pos + "password".len()..];
        let after = after.trim_start();
        if let Some(rest) = after.strip_prefix(':').or_else(|| after.strip_prefix('=')) {
            if !rest.trim_start().is_empty() {
                return true;
            }
        }
        search_from += pos + "password".len();
    }
    false
}

/// Add an admin note. Refuses anything that looks like a secret, and
/// caps notes at 2000 characters (v1 `addNote`).
pub async fn add_memory_note(db: &dyn Db, text: &str) -> Result<MemoryEntry, WordlessError> {
    let value = crate::util::require_str(text, "text", 2000)?;
    if looks_like_secret(&value) {
        return Err(bad_request(
            "Notes must not contain API keys, passwords or private-key material.",
        ));
    }
    let key = format!("note:{}", new_uuid_v4()?);
    let now = wordless_kernel::time::now_iso();
    let meta = db
        .prepare(
            "INSERT INTO wl_ai_memory (kind, key, value, source, updated_at) VALUES ('note', ?, ?, 'admin', ?)",
        )
        .bind(vec![
            Param::Text(key),
            Param::Text(value),
            Param::Text(now),
        ])
        .run()
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_ai_memory WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first::<Row>()
        .await?;
    row.map(|r| row_to_entry(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Memory note was not created."))
}

/// Delete an admin note by id. Facts cannot be deleted — they are
/// rebuilt from the site (v1 `deleteNote`).
pub async fn delete_memory_note(db: &dyn Db, id: i64) -> Result<bool, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_ai_memory WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Err(not_found("Memory note not found."));
    };
    if row_text(&row, "kind") != "note" {
        return Err(bad_request(
            "Only admin notes can be deleted; facts are rebuilt from the site.",
        ));
    }
    let meta = db
        .prepare("DELETE FROM wl_ai_memory WHERE id = ? AND kind = 'note'")
        .bind(vec![Param::Integer(id)])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

async fn upsert_fact(db: &dyn Db, key: &str, value: &str) -> Result<(), WordlessError> {
    let now = wordless_kernel::time::now_iso();
    db.prepare(
        "INSERT INTO wl_ai_memory (kind, key, value, source, updated_at) VALUES ('fact', ?, ?, 'system', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, source = excluded.source, updated_at = excluded.updated_at",
    )
    .bind(vec![
        Param::Text(key.to_string()),
        Param::Text(value.to_string()),
        Param::Text(now),
    ])
    .run()
    .await?;
    Ok(())
}

/// Re-ingest facts from the site's own data. Deletes stale auto facts
/// first (admin notes are never touched), then writes fresh ones
/// (v1 `rebuildMemory`).
pub async fn rebuild_memory(db: &dyn Db, site_id: i64) -> Result<MemoryRebuildResult, WordlessError> {
    db.prepare("DELETE FROM wl_ai_memory WHERE kind = 'fact'")
        .run()
        .await?;
    // P12: title/tagline come from the current site's options;
    // the memory rows themselves stay network-wide (wl_ai_memory has
    // no site column — see the P12 outcome block).
    let title: String = wordless_multisite::get_site_option(db, site_id, "blogname", "Wordless".to_string()).await?;
    let tagline: String = wordless_multisite::get_site_option(db, site_id, "blogdescription", String::new()).await?;
    let theme = wordless_themes::get_active_theme_slug(db, site_id).await?;
    let posts = wordless_cms::posts::list_posts(
        db,
        site_id,
        wordless_cms::posts::ListPostsOptions {
            status: Some("publish".to_string()),
            post_type: Some("post".to_string()),
            author_id: None,
            page: Some(1),
            per_page: Some(MAX_POSTS_INGESTED),
            include_trash: false,
        },
    )
    .await?;
    let pages = wordless_cms::posts::list_posts(
        db,
        site_id,
        wordless_cms::posts::ListPostsOptions {
            status: Some("publish".to_string()),
            post_type: Some("page".to_string()),
            author_id: None,
            page: Some(1),
            per_page: Some(MAX_POSTS_INGESTED),
            include_trash: false,
        },
    )
    .await?;
    let extension_rows = wordless_integrations::list_extensions(db).await?;
    let post_list: Vec<serde_json::Value> = posts
        .posts
        .iter()
        .map(|p| serde_json::json!({"id": p.id, "title": p.title, "slug": p.slug}))
        .collect();
    let extension_list: Vec<serde_json::Value> = extension_rows
        .iter()
        .map(|e| {
            serde_json::json!({
                "slug": e.slug,
                "name": e.name,
                "status": e.status,
            })
        })
        .collect();
    upsert_fact(db, "site.title", &title).await?;
    upsert_fact(db, "site.tagline", &tagline).await?;
    upsert_fact(db, "site.theme", &theme).await?;
    upsert_fact(db, "site.post_count", &posts.total.to_string()).await?;
    upsert_fact(db, "site.page_count", &pages.total.to_string()).await?;
    upsert_fact(
        db,
        "site.posts",
        &serde_json::to_string(&post_list).unwrap_or_else(|_| "[]".to_string()),
    )
    .await?;
    upsert_fact(
        db,
        "site.extensions",
        &serde_json::to_string(&extension_list).unwrap_or_else(|_| "[]".to_string()),
    )
    .await?;
    upsert_fact(db, "site.generated_at", &wordless_kernel::time::now_iso()).await?;
    let notes: i64 = db
        .prepare("SELECT COUNT(*) AS n FROM wl_ai_memory WHERE kind = 'note'")
        .first::<Row>()
        .await?
        .map(|r| row_int(&r, "n"))
        .unwrap_or(0);
    let facts: i64 = db
        .prepare("SELECT COUNT(*) AS n FROM wl_ai_memory WHERE kind = 'fact'")
        .first::<Row>()
        .await?
        .map(|r| row_int(&r, "n"))
        .unwrap_or(0);
    Ok(MemoryRebuildResult { facts, notes })
}

/// Render memory as the context block injected into agent prompts
/// (v1 `buildMemoryContext`).
pub async fn build_memory_context(db: &dyn Db) -> Result<String, WordlessError> {
    let memory = list_memory(db).await?;
    let mut lines: Vec<String> = vec![
        "Website memory (facts about this site — use them, do not invent others):".to_string(),
    ];
    let find = |key: &str| memory.facts.iter().find(|f| f.key == key);
    if let Some(title) = find("site.title") {
        lines.push(format!("- Site title: {}", title.value));
    }
    if let Some(tagline) = find("site.tagline") {
        if !tagline.value.is_empty() {
            lines.push(format!("- Tagline: {}", tagline.value));
        }
    }
    if let Some(theme) = find("site.theme") {
        lines.push(format!("- Active theme: {}", theme.value));
    }
    if let Some(post_count) = find("site.post_count") {
        lines.push(format!("- Published posts: {}", post_count.value));
    }
    if let Some(posts_fact) = find("site.posts") {
        if let Ok(posts) = serde_json::from_str::<Vec<serde_json::Value>>(&posts_fact.value) {
            for post in posts.iter().take(MAX_POSTS_IN_CONTEXT) {
                let title = post.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let slug = post.get("slug").and_then(|v| v.as_str()).unwrap_or("");
                lines.push(format!("- Post: \"{title}\" (/{slug})"));
            }
        }
        // A corrupt posts fact is ignored; the rest of memory still renders.
    }
    if !memory.notes.is_empty() {
        lines.push("Admin notes:".to_string());
        for note in memory.notes.iter().take(MAX_NOTES_IN_CONTEXT) {
            lines.push(format!("- {}", note.value));
        }
    }
    if lines.len() == 1 {
        lines.push(
            "- (No website memory yet — run a memory rebuild from the command centre.)".to_string(),
        );
    }
    Ok(lines.join("\n"))
}
