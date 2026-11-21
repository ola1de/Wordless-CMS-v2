//! Import service — WXR and SQL-dump import (port of v1
//! `src/modules/importer/service.ts`).
//!
//! Data only: parses XML and SQL data, never evals PHP or executes
//! imported code. WordPress phpass hashes are not portable — every
//! imported user gets an empty (unusable) password + the forced-reset
//! flag `wl_password_reset_required` (same as v1).

use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Map, Value};
use wordless_auth::{get_user_by_login, update_user_meta, PASSWORD_RESET_META_KEY};
use wordless_cms::posts::{ensure_unique_slug_for_type, insert_post_row, slugify};
use wordless_cms::terms::{add_post_term, create_term, CreateTermInput};
use wordless_db::row::row_int;
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, WordlessError};
use wordless_kernel::storage::ObjectStore;
use wordless_kernel::time::now_iso;

use crate::php::{safe_unserialize, PhpUnserializeError};
use crate::sql::parse_sql_dump;
use crate::wxr::{parse_wxr, WxrExport};

// ---------------------------------------------------------------- report

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemStatus {
    Imported,
    Skipped,
    Failed,
}

/// One per-item report entry (v1 `ImportReportItem` shape).
#[derive(Debug, Clone, Serialize)]
pub struct ImportReportItem {
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(rename = "sourceId")]
    pub source_id: String,
    #[serde(rename = "targetId")]
    pub target_id: Option<i64>,
    pub status: ItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TypeCounts {
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// The counts half of the report — v1's `ImportReport.counts`
/// shape exactly: totals plus a per-type breakdown. (P5 F8: the P3
/// port had flattened this into `ImportCounts { posts, pages, users,
/// terms, comments, skipped }` with the totals and `byType` as
/// top-level report fields; that matched neither v1 nor the admin
/// Import page. The per-type buckets are the source of truth — a
/// `post` bucket counts post + page + attachment together, as in v1.)
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCounts {
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub by_type: BTreeMap<String, TypeCounts>,
}

/// The full import report — v1 `ImportReport` shape, field for field.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub kind: String,
    pub source: String,
    pub counts: ImportCounts,
    pub items: Vec<ImportReportItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignored_tables: Vec<String>,
    pub finished_at: String,
}

struct Report {
    items: Vec<ImportReportItem>,
}

impl Report {
    fn new() -> Self {
        Self { items: Vec::new() }
    }

    fn add(
        &mut self,
        item_type: &str,
        source_id: impl Into<String>,
        target_id: Option<i64>,
        status: ItemStatus,
        reason: Option<String>,
    ) {
        self.items.push(ImportReportItem {
            item_type: item_type.to_string(),
            source_id: source_id.into(),
            target_id,
            status,
            reason,
        });
    }

    fn build(self, kind: &str, source: &str, ignored_tables: Vec<String>) -> ImportReport {
        let mut by_type: BTreeMap<String, TypeCounts> = BTreeMap::new();
        let (mut imported, mut skipped, mut failed) = (0, 0, 0);
        for item in &self.items {
            let bucket = by_type.entry(item.item_type.clone()).or_default();
            match item.status {
                ItemStatus::Imported => {
                    bucket.imported += 1;
                    imported += 1;
                }
                ItemStatus::Skipped => {
                    bucket.skipped += 1;
                    skipped += 1;
                }
                ItemStatus::Failed => {
                    bucket.failed += 1;
                    failed += 1;
                }
            }
        }
        ImportReport {
            kind: kind.to_string(),
            source: source.to_string(),
            counts: ImportCounts {
                imported,
                skipped,
                failed,
                by_type,
            },
            items: self.items,
            ignored_tables,
            finished_at: now_iso(),
        }
    }
}

// ---------------------------------------------------------------- helpers

/// WordPress post status → Wordless post status
/// (`draft`/`publish`/`private`/`trash` only) — v1 `mapWpStatus`.
pub fn map_wp_status(status: &str, post_type: &str) -> (String, Option<String>) {
    let s = status.to_lowercase();
    if matches!(s.as_str(), "publish" | "draft" | "private" | "trash") {
        return (s, None);
    }
    if s == "inherit" && post_type == "attachment" {
        return (
            "publish".to_string(),
            Some(
                "WordPress status \"inherit\" → publish (attachments are not listed).".to_string(),
            ),
        );
    }
    (
        "draft".to_string(),
        Some(format!(
            "WordPress status \"{status}\" has no Wordless equivalent → imported as draft."
        )),
    )
}

/// `"2026-01-15 10:30:00"` or ISO or RFC-2822 → ISO-8601 UTC, or
/// `None` when unparseable (v1 `toIso`).
pub fn to_iso(date_str: &str) -> Option<String> {
    let trimmed = date_str.trim();
    if trimmed.is_empty() || trimmed == "0000-00-00 00:00:00" {
        return None;
    }
    // "YYYY-MM-DD HH:MM:SS" (WordPress / mysqldump form).
    if trimmed.len() == 19
        && trimmed.as_bytes().get(4) == Some(&b'-')
        && trimmed.as_bytes().get(7) == Some(&b'-')
        && trimmed.as_bytes().get(10) == Some(&b' ')
        && trimmed.as_bytes().get(13) == Some(&b':')
        && trimmed.as_bytes().get(16) == Some(&b':')
        && trimmed[..10]
            .chars()
            .chain(trimmed[11..].chars())
            .all(|c| c.is_ascii_digit() || c == '-' || c == ':')
    {
        let (y, m, d) = (
            trimmed[0..4].parse::<u32>().ok()?,
            trimmed[5..7].parse::<u32>().ok()?,
            trimmed[8..10].parse::<u32>().ok()?,
        );
        let (hh, mm, ss) = (
            trimmed[11..13].parse::<u32>().ok()?,
            trimmed[14..16].parse::<u32>().ok()?,
            trimmed[17..19].parse::<u32>().ok()?,
        );
        if !(1..=12).contains(&m) || d == 0 || d > 31 || hh > 23 || mm > 59 || ss > 60 {
            return None;
        }
        return Some(format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.000Z"));
    }
    // RFC-2822, e.g. "Sat, 10 Jan 2026 10:30:00 +0000" (WXR pubDate).
    if let Some(iso) = rfc2822_to_iso(trimmed) {
        return Some(iso);
    }
    // ISO-8601 already: normalise a naive "…T…:…:…" to UTC.
    if trimmed.len() >= 19
        && trimmed.as_bytes().get(4) == Some(&b'-')
        && trimmed.as_bytes().get(10) == Some(&b'T')
    {
        if trimmed.ends_with('Z') || trimmed.contains('+') {
            return Some(trimmed.to_string());
        }
        return Some(format!("{trimmed}.000Z"));
    }
    None
}

fn rfc2822_to_iso(s: &str) -> Option<String> {
    // "Sat, 10 Jan 2026 10:30:00 +0000"
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    let day: u32 = parts[1].parse().ok()?;
    let month = match parts[2] {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: u32 = parts[3].parse().ok()?;
    let time = parts[4];
    if time.len() != 8 || time.as_bytes()[2] != b':' || time.as_bytes()[5] != b':' {
        return None;
    }
    let offset = parts.get(5).copied().unwrap_or("+0000");
    if offset != "+0000" && offset != "GMT" && offset != "UTC" {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}T{time}.000Z"))
}

/// WordPress `comment_approved` → Wordless: `"1"` approved, `"0"`
/// pending, `"spam"` spam (v1 `mapCommentApproved`).
pub fn map_comment_approved(value: &str) -> String {
    match value.trim().to_lowercase().as_str() {
        "1" | "approve" | "approved" => "1".to_string(),
        "spam" => "spam".to_string(),
        _ => "0".to_string(),
    }
}

fn value_to_param(v: &Value) -> Param {
    match v {
        Value::Null => Param::Null,
        Value::Bool(b) => Param::Integer(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Param::Integer(i)
            } else if let Some(f) = n.as_f64() {
                Param::Real(f)
            } else {
                Param::Text(n.to_string())
            }
        }
        Value::String(s) => Param::Text(s.clone()),
        _ => Param::Text(serde_json::to_string(v).unwrap_or_default()),
    }
}

fn row_str(values: &Map<String, Value>, key: &str) -> String {
    match values.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn row_i64(values: &Map<String, Value>, key: &str) -> i64 {
    match values.get(key) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
        Some(Value::Bool(b)) => i64::from(*b),
        _ => 0,
    }
}

/// Insert an imported user: unusable password + forced-reset flag
/// (v1 `insertImportedUser` — phpass hashes are not portable).
pub async fn insert_imported_user(
    db: &dyn Db,
    login: &str,
    email: &str,
    display_name: &str,
    registered: &str,
    source: &str,
) -> Result<(i64, bool), WordlessError> {
    if let Some(existing) = get_user_by_login(db, login).await? {
        return Ok((existing.id, true));
    }
    let nicename = slugify(login);
    let meta = db
        .prepare(
            "INSERT INTO wp_users (user_login, user_pass, user_nicename, user_email, user_url, user_registered, display_name) \
             VALUES (?, '', ?, ?, '', ?, ?)",
        )
        .bind(vec![
            Param::Text(login.to_string()),
            Param::Text(nicename),
            Param::Text(email.to_string()),
            Param::Text(registered.to_string()),
            Param::Text(display_name.to_string()),
        ])
        .run()
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    update_user_meta(db, id, PASSWORD_RESET_META_KEY, &"1").await?;
    update_user_meta(db, id, "wl_imported_from", &source).await?;
    Ok((id, false))
}

/// Store one meta value: PHP-serialized values become JSON, plain
/// strings stay plain strings (stored as JSON, per the architecture).
/// Returns `(ok, reason)` — a failed conversion is recorded, never
/// thrown (v1 `insertMetaValue`).
pub async fn insert_meta_value(
    db: &dyn Db,
    site_id: i64,
    table: &str,
    id_column: &str,
    id_value: i64,
    key: &str,
    raw_value: &str,
) -> Result<(bool, Option<String>), WordlessError> {
    if !matches!(
        table,
        "wp_postmeta" | "wp_usermeta" | "wp_commentmeta" | "wp_termmeta"
    ) {
        return Err(bad_request(&format!("Unknown meta table \"{table}\".")));
    }
    let stored = match safe_unserialize(raw_value) {
        Ok(v) => serde_json::to_string(&v).map_err(|e| {
            WordlessError::new("internal_error", 500, &format!("meta JSON failed: {e}"))
        })?,
        Err(PhpUnserializeError(msg)) => return Ok((false, Some(msg))),
    };
    // P12: site-scoped meta tables carry site_id; wp_usermeta is
    // shared (users are network-wide) and has no site_id column.
    if table == "wp_usermeta" {
        db.prepare(&format!(
            "INSERT INTO {table} ({id_column}, meta_key, meta_value) VALUES (?, ?, ?)"
        ))
        .bind(vec![
            Param::Integer(id_value),
            Param::Text(key.to_string()),
            Param::Text(stored),
        ])
        .run()
        .await?;
    } else {
        db.prepare(&format!(
            "INSERT INTO {table} (site_id, {id_column}, meta_key, meta_value) VALUES (?, ?, ?, ?)"
        ))
        .bind(vec![
            Param::Integer(site_id),
            Param::Integer(id_value),
            Param::Text(key.to_string()),
            Param::Text(stored),
        ])
        .run()
        .await?;
    }
    Ok((true, None))
}

#[allow(clippy::too_many_arguments)]
async fn insert_comment_row(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    author: &str,
    author_email: &str,
    author_url: &str,
    author_ip: &str,
    date: &str,
    date_gmt: &str,
    content: &str,
    approved: &str,
    comment_type: &str,
    parent: i64,
    user_id: i64,
) -> Result<i64, WordlessError> {
    let meta = db
        .prepare(
            "INSERT INTO wp_comments
               (site_id, comment_post_ID, comment_author, comment_author_email, comment_author_url, comment_author_IP,
                comment_date, comment_date_gmt, comment_content, comment_karma, comment_approved,
                comment_agent, comment_type, comment_parent, user_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, '', ?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(site_id),
            Param::Integer(post_id),
            Param::Text(author.to_string()),
            Param::Text(author_email.to_string()),
            Param::Text(author_url.to_string()),
            Param::Text(author_ip.to_string()),
            Param::Text(date.to_string()),
            Param::Text(date_gmt.to_string()),
            Param::Text(content.to_string()),
            Param::Text(approved.to_string()),
            Param::Text(comment_type.to_string()),
            Param::Integer(parent),
            Param::Integer(user_id),
        ])
        .run()
        .await?;
    Ok(meta.last_row_id.unwrap_or(0))
}

async fn refresh_comment_count(db: &dyn Db, site_id: i64, post_id: i64) -> Result<(), WordlessError> {
    let row: Option<Row> = db
        .prepare(
            "SELECT COUNT(*) AS n FROM wp_comments WHERE comment_post_ID = ? AND comment_approved = '1' AND site_id = ?",
        )
        .bind(vec![Param::Integer(post_id), Param::Integer(site_id)])
        .first()
        .await?;
    let n = row.as_ref().map_or(0, |r| row_int(r, "n"));
    db.prepare("UPDATE wp_posts SET comment_count = ? WHERE ID = ? AND site_id = ?")
        .bind(vec![Param::Integer(n), Param::Integer(post_id), Param::Integer(site_id)])
        .run()
        .await?;
    Ok(())
}

const MIME_EXT: &[(&str, &str)] = &[
    ("image/jpeg", "jpg"),
    ("image/png", "png"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
    ("image/avif", "avif"),
    ("image/svg+xml", "svg"),
    ("application/pdf", "pdf"),
];

fn mime_ext(mime: &str) -> &'static str {
    MIME_EXT
        .iter()
        .find(|(m, _)| *m == mime)
        .map(|(_, e)| *e)
        .unwrap_or("bin")
}

/// Fetch one attachment file (URL → bytes + MIME). The worker wires
/// this to a real `fetch`; tests stub it. A `None` return (or an
/// error the implementation converts to `None`) is recorded as a
/// failed attachment, never dropped (v1 rule).
#[async_trait(?Send)]
pub trait AttachmentFetcher {
    async fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)>;
}

// ---------------------------------------------------------------- WXR

/// Import a WXR export. Attachments are recorded `skipped` (no
/// attachment fetcher configured) — use [`import_wxr_with_media`]
/// when files should be fetched into storage.
///
/// Returns the full [`ImportReport`]; the totals and per-type
/// breakdown live in `report.counts` (`imported`, `skipped`,
/// `skipped`).
pub async fn import_wxr(
    db: &dyn Db,
    site_id: i64,
    xml: &str,
    admin_id: i64,
    source: &str,
) -> Result<ImportReport, WordlessError> {
    import_wxr_inner(db, site_id, xml, admin_id, None, None, source).await
}

/// Import a WXR export, fetching attachment files into `store` under
/// `media/imported-<id>-<slug>.<ext>` (v1 key shape).
/// Import a WXR file. `source` is the uploaded filename — v1's
/// `ImportReport.source` is the filename, not the kind.
pub async fn import_wxr_with_media(
    db: &dyn Db,
    site_id: i64,
    xml: &str,
    admin_id: i64,
    store: &dyn ObjectStore,
    fetcher: &dyn AttachmentFetcher,
    source: &str,
) -> Result<ImportReport, WordlessError> {
    import_wxr_inner(db, site_id, xml, admin_id, Some(store), Some(fetcher), source).await
}

async fn import_wxr_inner(
    db: &dyn Db,
    site_id: i64,
    xml: &str,
    admin_id: i64,
    store: Option<&dyn ObjectStore>,
    fetcher: Option<&dyn AttachmentFetcher>,
    source: &str,
) -> Result<ImportReport, WordlessError> {
    let parsed: WxrExport = parse_wxr(xml).map_err(|e| bad_request(&e.to_string()))?;
    let mut report = Report::new();

    // Authors → users (forced password reset).
    let mut author_map: HashMap<String, i64> = HashMap::new();
    for author in &parsed.authors {
        if author.login.is_empty() {
            report.add(
                "user",
                format!("author:{}", author.id),
                None,
                ItemStatus::Skipped,
                Some("Author has no login name.".to_string()),
            );
            continue;
        }
        let display = if !author.display_name.is_empty() {
            author.display_name.clone()
        } else {
            format!("{} {}", author.first_name, author.last_name)
                .trim()
                .to_string()
        };
        let display = if display.is_empty() {
            author.login.clone()
        } else {
            display
        };
        let (id, existed) = insert_imported_user(
            db,
            &author.login,
            &author.email,
            &display,
            &now_iso(),
            "wxr",
        )
        .await?;
        author_map.insert(author.login.clone(), id);
        report.add(
            "user",
            format!("author:{} ({})", author.id, author.login),
            Some(id),
            if existed {
                ItemStatus::Skipped
            } else {
                ItemStatus::Imported
            },
            if existed {
                Some("User already exists.".to_string())
            } else {
                None
            },
        );
    }

    // Terms (categories, tags, custom taxonomies).
    let mut term_map: HashMap<String, (i64, i64)> = HashMap::new();
    let mut pending_parents: Vec<(i64, String)> = Vec::new();
    for def in &parsed.terms {
        let taxonomy = if def.taxonomy == "post_tag" {
            "tag".to_string()
        } else {
            def.taxonomy.clone()
        };
        let key = format!("{taxonomy}:{}", def.slug);
        let existing: Option<Row> = db
            .prepare(
                "SELECT t.term_id, tt.term_taxonomy_id FROM wp_terms t \
                 JOIN wp_term_taxonomy tt ON tt.term_id = t.term_id \
                 WHERE tt.taxonomy = ? AND t.slug = ? AND tt.site_id = ? AND t.site_id = ? LIMIT 1",
            )
            .bind(vec![
                Param::Text(taxonomy.clone()),
                Param::Text(def.slug.clone()),
                Param::Integer(site_id),
                Param::Integer(site_id),
            ])
            .first()
            .await?;
        if let Some(row) = existing {
            let pair = (row_int(&row, "term_id"), row_int(&row, "term_taxonomy_id"));
            term_map.insert(key.clone(), pair);
            report.add(
                "term",
                key,
                Some(pair.0),
                ItemStatus::Skipped,
                Some("Term already exists.".to_string()),
            );
            continue;
        }
        match create_term(
            db,
            site_id,
            CreateTermInput {
                name: if def.name.is_empty() {
                    def.slug.clone()
                } else {
                    def.name.clone()
                },
                slug: Some(def.slug.clone()),
                taxonomy: taxonomy.clone(),
                description: Some(def.description.clone()),
                parent_id: None,
            },
        )
        .await
        {
            Ok(term) => {
                term_map.insert(key.clone(), (term.id, term.term_taxonomy_id));
                report.add("term", key, Some(term.id), ItemStatus::Imported, None);
                if taxonomy == "category" && !def.parent.is_empty() {
                    pending_parents.push((term.term_taxonomy_id, def.parent.clone()));
                }
            }
            Err(e) => {
                report.add(
                    "term",
                    key,
                    None,
                    ItemStatus::Failed,
                    Some(e.message.clone()),
                );
            }
        }
    }
    // Second pass: category parents (the parent term may have been
    // created after the child). WXR parents are slugs (nicenames).
    for (tt_id, parent_slug) in pending_parents {
        match term_map.get(&format!("category:{parent_slug}")) {
            Some((parent_term_id, _)) => {
                db.prepare("UPDATE wp_term_taxonomy SET parent = ? WHERE term_taxonomy_id = ? AND site_id = ?")
                    .bind(vec![Param::Integer(*parent_term_id), Param::Integer(tt_id), Param::Integer(site_id)])
                    .run()
                    .await?;
            }
            None => {
                report.add(
                    "term_parent",
                    parent_slug,
                    None,
                    ItemStatus::Failed,
                    Some("Parent category not found in the export.".to_string()),
                );
            }
        }
    }

    // Items: posts, pages, attachments. Revisions are skipped.
    let mut post_map: HashMap<i64, i64> = HashMap::new();
    let mut touched_posts: Vec<i64> = Vec::new();
    for item in &parsed.items {
        if item.post_type == "revision" {
            report.add(
                "post",
                format!("revision:{}", item.post_id),
                None,
                ItemStatus::Skipped,
                Some("Revisions are not imported.".to_string()),
            );
            continue;
        }
        let author_id = author_map.get(&item.creator).copied().unwrap_or(admin_id);
        let base_slug = slugify(if item.post_name.is_empty() {
            &item.title
        } else {
            &item.post_name
        });
        let post_type = if item.post_type.is_empty() {
            "post".to_string()
        } else {
            item.post_type.clone()
        };
        let slug = ensure_unique_slug_for_type(db, site_id, &base_slug, &post_type, None).await?;
        let (status, note) = map_wp_status(&item.status, &post_type);
        let date = to_iso(&item.post_date)
            .or_else(|| to_iso(&item.pub_date))
            .unwrap_or_else(now_iso);
        let new_id = match insert_post_row(
            db,
            site_id,
            author_id,
            &date,
            &item.content,
            if item.title.is_empty() {
                "(no title)"
            } else {
                &item.title
            },
            &item.excerpt,
            &status,
            &slug,
            0,
            &post_type,
            "",
            if item.comment_status == "closed" {
                "closed"
            } else {
                "open"
            },
        )
        .await
        {
            Ok(id) => id,
            Err(e) => {
                report.add(
                    "post",
                    format!("{post_type}:{}", item.post_id),
                    None,
                    ItemStatus::Failed,
                    Some(e.message.clone()),
                );
                continue;
            }
        };
        post_map.insert(item.post_id, new_id);
        touched_posts.push(new_id);
        db.prepare("UPDATE wp_posts SET guid = ?, menu_order = ?, post_password = ? WHERE ID = ? AND site_id = ?")
            .bind(vec![
                Param::Text(if item.guid.is_empty() {
                    item.link.clone()
                } else {
                    item.guid.clone()
                }),
                Param::Integer(item.menu_order),
                Param::Text(item.post_password.clone()),
                Param::Integer(new_id),
                Param::Integer(site_id),
            ])
            .run()
            .await?;
        report.add(
            "post",
            format!("{post_type}:{}", item.post_id),
            Some(new_id),
            ItemStatus::Imported,
            note,
        );

        // Postmeta (serialized values → JSON).
        for meta in &item.meta {
            let (ok, reason) =
                insert_meta_value(db, site_id, "wp_postmeta", "post_id", new_id, &meta.key, &meta.value)
                    .await?;
            report.add(
                "postmeta",
                format!("{}:{}", item.post_id, meta.key),
                if ok { Some(new_id) } else { None },
                if ok {
                    ItemStatus::Imported
                } else {
                    ItemStatus::Failed
                },
                reason,
            );
        }

        // Term assignment.
        for cat in &item.categories {
            let taxonomy = if cat.domain == "post_tag" {
                "tag".to_string()
            } else {
                cat.domain.clone()
            };
            let key = format!("{taxonomy}:{}", cat.nicename);
            let mut term = term_map.get(&key).copied();
            if term.is_none() {
                if cat.nicename.is_empty() {
                    report.add(
                        "term",
                        key,
                        None,
                        ItemStatus::Skipped,
                        Some("Category has no nicename.".to_string()),
                    );
                    continue;
                }
                match create_term(
                    db,
                    site_id,
                    CreateTermInput {
                        name: if cat.name.is_empty() {
                            cat.nicename.clone()
                        } else {
                            cat.name.clone()
                        },
                        slug: Some(cat.nicename.clone()),
                        taxonomy: taxonomy.clone(),
                        description: None,
                        parent_id: None,
                    },
                )
                .await
                {
                    Ok(created) => {
                        term = Some((created.id, created.term_taxonomy_id));
                        term_map.insert(key.clone(), (created.id, created.term_taxonomy_id));
                        report.add(
                            "term",
                            key.clone(),
                            Some(created.id),
                            ItemStatus::Imported,
                            None,
                        );
                    }
                    Err(e) => {
                        report.add(
                            "term",
                            key,
                            None,
                            ItemStatus::Failed,
                            Some(e.message.clone()),
                        );
                        continue;
                    }
                }
            }
            let (_, tt_id) = term.unwrap();
            if let Err(e) = add_post_term(db, site_id, new_id, tt_id).await {
                report.add(
                    "term",
                    key,
                    None,
                    ItemStatus::Failed,
                    Some(e.message.clone()),
                );
            }
        }

        // Comments (two-pass parents below).
        let mut comment_map: HashMap<i64, i64> = HashMap::new();
        let mut pending_comment_parents: Vec<(i64, i64)> = Vec::new();
        for comment in &item.comments {
            let comment_date = to_iso(&comment.date)
                .or_else(|| to_iso(&comment.date_gmt))
                .unwrap_or_else(now_iso);
            let comment_date_gmt =
                to_iso(&comment.date_gmt).unwrap_or_else(|| comment_date.clone());
            let new_comment_id = insert_comment_row(
                db,
                site_id,
                new_id,
                &comment.author,
                &comment.author_email,
                &comment.author_url,
                &comment.author_ip,
                &comment_date,
                &comment_date_gmt,
                &comment.content,
                &map_comment_approved(&comment.approved),
                if comment.comment_type.is_empty() {
                    "comment"
                } else {
                    &comment.comment_type
                },
                0,
                comment.user_id,
            )
            .await?;
            comment_map.insert(comment.id, new_comment_id);
            if comment.parent != 0 {
                pending_comment_parents.push((new_comment_id, comment.parent));
            }
            report.add(
                "comment",
                format!("{}:{}", item.post_id, comment.id),
                Some(new_comment_id),
                ItemStatus::Imported,
                None,
            );
            for meta in &comment.meta {
                let (ok, reason) = insert_meta_value(
                    db,
                    site_id,
                    "wp_commentmeta",
                    "comment_id",
                    new_comment_id,
                    &meta.key,
                    &meta.value,
                )
                .await?;
                report.add(
                    "commentmeta",
                    format!("{}:{}", comment.id, meta.key),
                    if ok { Some(new_comment_id) } else { None },
                    if ok {
                        ItemStatus::Imported
                    } else {
                        ItemStatus::Failed
                    },
                    reason,
                );
            }
        }
        for (new_comment_id, old_parent) in pending_comment_parents {
            if let Some(parent) = comment_map.get(&old_parent) {
                db.prepare("UPDATE wp_comments SET comment_parent = ? WHERE comment_ID = ? AND site_id = ?")
                    .bind(vec![
                        Param::Integer(*parent),
                        Param::Integer(new_comment_id),
                        Param::Integer(site_id),
                    ])
                    .run()
                    .await?;
            }
        }

        // Attachments: fetch the file into storage; a failed fetch is
        // recorded, never dropped.
        if post_type == "attachment" {
            let url = if item.guid.is_empty() {
                item.link.clone()
            } else {
                item.guid.clone()
            };
            match (store, fetcher) {
                (Some(store), Some(fetcher)) if !url.is_empty() => {
                    match fetcher.fetch(&url).await {
                        Some((bytes, mime)) => {
                            let ext = mime_ext(&mime);
                            let key = format!("media/imported-{new_id}-{slug}.{ext}");
                            store
                                .put_object(&key, bytes, &mime)
                                .await
                                .map_err(|e| bad_request(&e.message.clone()))?;
                            db.prepare("UPDATE wp_posts SET post_mime_type = ? WHERE ID = ? AND site_id = ?")
                                .bind(vec![Param::Text(mime), Param::Integer(new_id), Param::Integer(site_id)])
                                .run()
                                .await?;
                            let (ok, reason) = insert_meta_value(
                                db,
                                site_id,
                                "wp_postmeta",
                                "post_id",
                                new_id,
                                "_wp_attached_file",
                                &key,
                            )
                            .await?;
                            let _ = (ok, reason);
                            report.add(
                                "attachment_media",
                                format!("{}", item.post_id),
                                Some(new_id),
                                ItemStatus::Imported,
                                None,
                            );
                        }
                        None => {
                            report.add(
                                "attachment_media",
                                format!("{}", item.post_id),
                                Some(new_id),
                                ItemStatus::Failed,
                                Some(format!("Could not fetch {url}.")),
                            );
                        }
                    }
                }
                _ => {
                    report.add(
                        "attachment_media",
                        format!("{}", item.post_id),
                        Some(new_id),
                        ItemStatus::Skipped,
                        Some(
                            "No attachment fetcher configured, or the item has no URL.".to_string(),
                        ),
                    );
                }
            }
        }
    }

    // Post parents (pages/attachments whose parent was imported later).
    for item in &parsed.items {
        if item.post_parent == 0 {
            continue;
        }
        if let (Some(new_id), Some(new_parent)) =
            (post_map.get(&item.post_id), post_map.get(&item.post_parent))
        {
            db.prepare("UPDATE wp_posts SET post_parent = ? WHERE ID = ? AND site_id = ?")
                .bind(vec![Param::Integer(*new_parent), Param::Integer(*new_id), Param::Integer(site_id)])
                .run()
                .await?;
        }
    }
    for post_id in touched_posts {
        refresh_comment_count(db, site_id, post_id).await?;
    }

    Ok(report.build("wxr", source, Vec::new()))
}

// ---------------------------------------------------------------- SQL

/// Tables whose rows belong to one site (P12). `wp_links` and the
/// user tables are network-wide and stay unscoped.
const SITE_SCOPED_TABLES: &[&str] = &[
    "wp_posts",
    "wp_postmeta",
    "wp_terms",
    "wp_term_taxonomy",
    "wp_term_relationships",
    "wp_comments",
    "wp_commentmeta",
    "wp_termmeta",
];

async fn id_exists(
    db: &dyn Db,
    table: &str,
    id_column: &str,
    id: i64,
    site_id: Option<i64>,
) -> Result<bool, WordlessError> {
    let scoped = site_id.is_some_and(|_| SITE_SCOPED_TABLES.contains(&table));
    let sql = if scoped {
        format!("SELECT COUNT(*) AS n FROM \"{table}\" WHERE \"{id_column}\" = ? AND site_id = ?")
    } else {
        format!("SELECT COUNT(*) AS n FROM \"{table}\" WHERE \"{id_column}\" = ?")
    };
    let mut params = vec![Param::Integer(id)];
    if scoped {
        params.push(Param::Integer(site_id.unwrap_or(1)));
    }
    let row: Option<Row> = db
        .prepare(&sql)
        .bind(params)
        .first()
        .await?;
    Ok(row.as_ref().map_or(0, |r| row_int(r, "n")) > 0)
}

/// Insert a row, keeping its original ID when free, else a new one.
/// Returns the final ID (v1 `insertWithOriginalId`).
async fn insert_with_original_id(
    db: &dyn Db,
    site_id: i64,
    table: &str,
    id_column: &str,
    row: &Map<String, Value>,
) -> Result<i64, WordlessError> {
    // P12: site-scoped rows are stamped with the target site; a
    // colliding original id on another site does not block the import.
    let mut owned: Map<String, Value>;
    let row = if SITE_SCOPED_TABLES.contains(&table) {
        owned = row.clone();
        owned.insert("site_id".to_string(), Value::from(site_id));
        &owned
    } else {
        row
    };
    let site = if SITE_SCOPED_TABLES.contains(&table) {
        Some(site_id)
    } else {
        None
    };
    let original_id = row_i64(row, id_column);
    let columns: Vec<String> = row.keys().cloned().collect();
    let insert = |with_id: bool| {
        let cols: Vec<&String> = if with_id {
            columns.iter().collect()
        } else {
            columns.iter().filter(|c| *c != id_column).collect()
        };
        let sql = format!(
            "INSERT INTO \"{table}\" ({}) VALUES ({})",
            cols.iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", "),
            cols.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
        );
        let params: Vec<Param> = cols
            .iter()
            .map(|c| value_to_param(row.get(*c).unwrap_or(&Value::Null)))
            .collect();
        (sql, params)
    };
    if original_id > 0 && !id_exists(db, table, id_column, original_id, site).await? {
        let (sql, params) = insert(true);
        let meta = db.prepare(&sql).bind(params).run().await?;
        return Ok(meta.last_row_id.unwrap_or(0));
    }
    let (sql, params) = insert(false);
    let meta = db.prepare(&sql).bind(params).run().await?;
    Ok(meta.last_row_id.unwrap_or(0))
}

/// Import a mysqldump. Returns the full [`ImportReport`]; the flat
/// task counts live in `report.counts`.
/// Import a SQL dump. `source` is the uploaded filename (see
/// [`import_wxr_with_media`]).
pub async fn import_sql(
    db: &dyn Db,
    site_id: i64,
    sql: &str,
    admin_id: i64,
    source: &str,
) -> Result<ImportReport, WordlessError> {
    let parsed = parse_sql_dump(sql).map_err(|e| bad_request(&e.to_string()))?;
    let mut report = Report::new();
    let mut rows_by_table: HashMap<String, Vec<Map<String, Value>>> = HashMap::new();
    for row in parsed.rows {
        rows_by_table.entry(row.table).or_default().push(row.values);
    }

    let mut user_map: HashMap<i64, i64> = HashMap::new();
    let mut post_map: HashMap<i64, i64> = HashMap::new();
    let mut term_map: HashMap<i64, (i64, i64)> = HashMap::new();
    let mut tt_map: HashMap<i64, i64> = HashMap::new();
    let mut comment_map: HashMap<i64, i64> = HashMap::new();
    let mut touched_posts: Vec<i64> = Vec::new();

    // Users first (posts reference authors).
    for row in rows_by_table.get("wp_users").cloned().unwrap_or_default() {
        let login = row_str(&row, "user_login");
        if login.is_empty() {
            report.add(
                "user",
                format!("sql:{}", row_i64(&row, "ID")),
                None,
                ItemStatus::Skipped,
                Some("User has no login name.".to_string()),
            );
            continue;
        }
        let (id, existed) = insert_imported_user(
            db,
            &login,
            &row_str(&row, "user_email"),
            &{
                let d = row_str(&row, "display_name");
                if d.is_empty() {
                    login.clone()
                } else {
                    d
                }
            },
            &to_iso(&row_str(&row, "user_registered")).unwrap_or_else(now_iso),
            "sql",
        )
        .await?;
        user_map.insert(row_i64(&row, "ID"), id);
        report.add(
            "user",
            format!("sql:{}", row_i64(&row, "ID")),
            Some(id),
            if existed {
                ItemStatus::Skipped
            } else {
                ItemStatus::Imported
            },
            if existed {
                Some("User already exists.".to_string())
            } else {
                None
            },
        );
    }

    // Terms.
    for row in rows_by_table.get("wp_terms").cloned().unwrap_or_default() {
        let slug = row_str(&row, "slug");
        let existing: Option<Row> = db
            .prepare("SELECT term_id FROM wp_terms WHERE slug = ? AND site_id = ? LIMIT 1")
            .bind(vec![Param::Text(slug.clone()), Param::Integer(site_id)])
            .first()
            .await?;
        if let Some(existing) = existing {
            let term_id = row_int(&existing, "term_id");
            term_map.insert(row_i64(&row, "term_id"), (term_id, 0));
            report.add(
                "term",
                format!("sql:{}", row_i64(&row, "term_id")),
                Some(term_id),
                ItemStatus::Skipped,
                Some("Term slug already exists.".to_string()),
            );
            continue;
        }
        let mut record = Map::new();
        record.insert("term_id".to_string(), Value::from(row_i64(&row, "term_id")));
        record.insert(
            "name".to_string(),
            Value::String({
                let n = row_str(&row, "name");
                if n.is_empty() {
                    slug.clone()
                } else {
                    n
                }
            }),
        );
        record.insert("slug".to_string(), Value::String(slug));
        record.insert(
            "term_group".to_string(),
            Value::from(row_i64(&row, "term_group")),
        );
        let new_id = insert_with_original_id(db, site_id, "wp_terms", "term_id", &record).await?;
        term_map.insert(row_i64(&row, "term_id"), (new_id, 0));
        report.add(
            "term",
            format!("sql:{}", row_i64(&row, "term_id")),
            Some(new_id),
            ItemStatus::Imported,
            None,
        );
    }

    // Options (serialized → JSON; Wordless's own wl_* options are
    // never overwritten).
    for row in rows_by_table.get("wp_options").cloned().unwrap_or_default() {
        let name = row_str(&row, "option_name");
        if name.is_empty() {
            report.add(
                "option",
                "sql:?".to_string(),
                None,
                ItemStatus::Skipped,
                Some("Option has no name.".to_string()),
            );
            continue;
        }
        if name.starts_with("wl_") {
            report.add(
                "option",
                name,
                None,
                ItemStatus::Skipped,
                Some("Wordless's own options are never overwritten by an import.".to_string()),
            );
            continue;
        }
        let stored = match safe_unserialize(&row_str(&row, "option_value")) {
            Ok(v) => match serde_json::to_string(&v) {
                Ok(s) => s,
                Err(e) => {
                    report.add(
                        "option",
                        name,
                        None,
                        ItemStatus::Failed,
                        Some(format!("Unserializable option value: {e}")),
                    );
                    continue;
                }
            },
            Err(PhpUnserializeError(msg)) => {
                report.add("option", name, None, ItemStatus::Failed, Some(msg));
                continue;
            }
        };
        // P12: imported options land on the target site (site 1 writes
        // wp_options exactly as before; other sites use wl_site_options).
        let value: Value = serde_json::from_str(&stored).unwrap_or(Value::Null);
        wordless_multisite::set_site_option(db, site_id, &name, &value).await?;
        report.add("option", name, None, ItemStatus::Imported, None);
    }

    // Links.
    for row in rows_by_table.get("wp_links").cloned().unwrap_or_default() {
        let mut record = Map::new();
        for key in [
            "link_id",
            "link_url",
            "link_name",
            "link_image",
            "link_target",
            "link_description",
            "link_visible",
            "link_owner",
            "link_rating",
            "link_updated",
            "link_rel",
            "link_notes",
            "link_rss",
        ] {
            let value = match key {
                "link_id" | "link_owner" | "link_rating" => Value::from(row_i64(&row, key)),
                "link_updated" => {
                    Value::String(to_iso(&row_str(&row, "link_updated")).unwrap_or_default())
                }
                _ => Value::String(row_str(&row, key)),
            };
            record.insert(key.to_string(), value);
        }
        if record["link_visible"] == Value::String(String::new()) {
            record.insert("link_visible".to_string(), Value::String("Y".to_string()));
        }
        let new_id = insert_with_original_id(db, site_id, "wp_links", "link_id", &record).await?;
        report.add(
            "link",
            format!("sql:{}", row_i64(&row, "link_id")),
            Some(new_id),
            ItemStatus::Imported,
            None,
        );
    }

    // Posts (revisions skipped; parents resolved in a second pass).
    let mut pending_post_parents: Vec<(i64, i64)> = Vec::new();
    for row in rows_by_table.get("wp_posts").cloned().unwrap_or_default() {
        let post_type = {
            let t = row_str(&row, "post_type");
            if t.is_empty() {
                "post".to_string()
            } else {
                t
            }
        };
        if post_type == "revision" {
            report.add(
                "post",
                format!("revision:{}", row_i64(&row, "ID")),
                None,
                ItemStatus::Skipped,
                Some("Revisions are not imported.".to_string()),
            );
            continue;
        }
        let (status, note) = map_wp_status(&row_str(&row, "post_status"), &post_type);
        let date = to_iso(&row_str(&row, "post_date")).unwrap_or_else(now_iso);
        let slug = ensure_unique_slug_for_type(
            db,
            site_id,
            &slugify(&{
                let n = row_str(&row, "post_name");
                if n.is_empty() {
                    row_str(&row, "post_title")
                } else {
                    n
                }
            }),
            &post_type,
            None,
        )
        .await?;
        let author_id = user_map
            .get(&row_i64(&row, "post_author"))
            .copied()
            .unwrap_or(admin_id);
        let mut record = Map::new();
        let put = |record: &mut Map<String, Value>, k: &str, v: Value| {
            record.insert(k.to_string(), v);
        };
        put(&mut record, "ID", Value::from(row_i64(&row, "ID")));
        put(&mut record, "post_author", Value::from(author_id));
        put(&mut record, "post_date", Value::String(date.clone()));
        put(
            &mut record,
            "post_date_gmt",
            Value::String(to_iso(&row_str(&row, "post_date_gmt")).unwrap_or_else(|| date.clone())),
        );
        put(
            &mut record,
            "post_content",
            Value::String(row_str(&row, "post_content")),
        );
        put(
            &mut record,
            "post_title",
            Value::String(row_str(&row, "post_title")),
        );
        put(
            &mut record,
            "post_excerpt",
            Value::String(row_str(&row, "post_excerpt")),
        );
        put(&mut record, "post_status", Value::String(status));
        put(
            &mut record,
            "comment_status",
            Value::String({
                let c = row_str(&row, "comment_status");
                if c.is_empty() {
                    "open".to_string()
                } else {
                    c
                }
            }),
        );
        put(
            &mut record,
            "ping_status",
            Value::String({
                let p = row_str(&row, "ping_status");
                if p.is_empty() {
                    "open".to_string()
                } else {
                    p
                }
            }),
        );
        put(
            &mut record,
            "post_password",
            Value::String(row_str(&row, "post_password")),
        );
        put(&mut record, "post_name", Value::String(slug));
        put(
            &mut record,
            "post_modified",
            Value::String(to_iso(&row_str(&row, "post_modified")).unwrap_or_else(|| date.clone())),
        );
        put(
            &mut record,
            "post_modified_gmt",
            Value::String(
                to_iso(&row_str(&row, "post_modified_gmt")).unwrap_or_else(|| date.clone()),
            ),
        );
        put(
            &mut record,
            "post_content_filtered",
            Value::String(row_str(&row, "post_content_filtered")),
        );
        put(&mut record, "post_parent", Value::from(0));
        put(&mut record, "guid", Value::String(row_str(&row, "guid")));
        put(
            &mut record,
            "menu_order",
            Value::from(row_i64(&row, "menu_order")),
        );
        put(&mut record, "post_type", Value::String(post_type.clone()));
        put(
            &mut record,
            "post_mime_type",
            Value::String(row_str(&row, "post_mime_type")),
        );
        put(&mut record, "comment_count", Value::from(0));
        let new_id = insert_with_original_id(db, site_id, "wp_posts", "ID", &record).await?;
        post_map.insert(row_i64(&row, "ID"), new_id);
        touched_posts.push(new_id);
        if row_i64(&row, "post_parent") != 0 {
            pending_post_parents.push((new_id, row_i64(&row, "post_parent")));
        }
        report.add(
            "post",
            format!("{post_type}:{}", row_i64(&row, "ID")),
            Some(new_id),
            ItemStatus::Imported,
            note,
        );
    }
    for (new_id, old_parent) in pending_post_parents {
        if let Some(parent) = post_map.get(&old_parent) {
            db.prepare("UPDATE wp_posts SET post_parent = ? WHERE ID = ? AND site_id = ?")
                .bind(vec![Param::Integer(*parent), Param::Integer(new_id), Param::Integer(site_id)])
                .run()
                .await?;
        }
    }

    // Postmeta / usermeta (serialized → JSON).
    for row in rows_by_table
        .get("wp_postmeta")
        .cloned()
        .unwrap_or_default()
    {
        let Some(post_id) = post_map.get(&row_i64(&row, "post_id")) else {
            report.add(
                "postmeta",
                format!("sql:{}", row_i64(&row, "meta_id")),
                None,
                ItemStatus::Skipped,
                Some("Parent post was not imported.".to_string()),
            );
            continue;
        };
        let post_id = *post_id;
        let (ok, reason) = insert_meta_value(
            db,
            site_id,
            "wp_postmeta",
            "post_id",
            post_id,
            &row_str(&row, "meta_key"),
            &row_str(&row, "meta_value"),
        )
        .await?;
        report.add(
            "postmeta",
            format!("sql:{}", row_i64(&row, "meta_id")),
            if ok { Some(post_id) } else { None },
            if ok {
                ItemStatus::Imported
            } else {
                ItemStatus::Failed
            },
            reason,
        );
    }
    for row in rows_by_table
        .get("wp_usermeta")
        .cloned()
        .unwrap_or_default()
    {
        let Some(user_id) = user_map.get(&row_i64(&row, "user_id")) else {
            report.add(
                "usermeta",
                format!("sql:{}", row_i64(&row, "umeta_id")),
                None,
                ItemStatus::Skipped,
                Some("Parent user was not imported.".to_string()),
            );
            continue;
        };
        let user_id = *user_id;
        let (ok, reason) = insert_meta_value(
            db,
            site_id,
            "wp_usermeta",
            "user_id",
            user_id,
            &row_str(&row, "meta_key"),
            &row_str(&row, "meta_value"),
        )
        .await?;
        report.add(
            "usermeta",
            format!("sql:{}", row_i64(&row, "umeta_id")),
            if ok { Some(user_id) } else { None },
            if ok {
                ItemStatus::Imported
            } else {
                ItemStatus::Failed
            },
            reason,
        );
    }

    // Term taxonomy (term_id remapped; post_tag → tag).
    for row in rows_by_table
        .get("wp_term_taxonomy")
        .cloned()
        .unwrap_or_default()
    {
        let Some((term_id, _)) = term_map.get(&row_i64(&row, "term_id")) else {
            report.add(
                "term_taxonomy",
                format!("sql:{}", row_i64(&row, "term_taxonomy_id")),
                None,
                ItemStatus::Skipped,
                Some("Parent term was not imported.".to_string()),
            );
            continue;
        };
        let term_id = *term_id;
        let taxonomy = {
            let t = row_str(&row, "taxonomy");
            if t == "post_tag" {
                "tag".to_string()
            } else if t.is_empty() {
                "category".to_string()
            } else {
                t
            }
        };
        let mut record = Map::new();
        record.insert(
            "term_taxonomy_id".to_string(),
            Value::from(row_i64(&row, "term_taxonomy_id")),
        );
        record.insert("term_id".to_string(), Value::from(term_id));
        record.insert("taxonomy".to_string(), Value::String(taxonomy));
        record.insert(
            "description".to_string(),
            Value::String(row_str(&row, "description")),
        );
        record.insert("parent".to_string(), Value::from(row_i64(&row, "parent")));
        record.insert("count".to_string(), Value::from(0));
        let new_tt_id =
            insert_with_original_id(db, site_id, "wp_term_taxonomy", "term_taxonomy_id", &record).await?;
        tt_map.insert(row_i64(&row, "term_taxonomy_id"), new_tt_id);
        term_map.insert(row_i64(&row, "term_id"), (term_id, new_tt_id));
        report.add(
            "term_taxonomy",
            format!("sql:{}", row_i64(&row, "term_taxonomy_id")),
            Some(new_tt_id),
            ItemStatus::Imported,
            None,
        );
    }
    for row in rows_by_table
        .get("wp_termmeta")
        .cloned()
        .unwrap_or_default()
    {
        let Some((term_id, _)) = term_map.get(&row_i64(&row, "term_id")) else {
            report.add(
                "termmeta",
                format!("sql:{}", row_i64(&row, "meta_id")),
                None,
                ItemStatus::Skipped,
                Some("Parent term was not imported.".to_string()),
            );
            continue;
        };
        let term_id = *term_id;
        let (ok, reason) = insert_meta_value(
            db,
            site_id,
            "wp_termmeta",
            "term_id",
            term_id,
            &row_str(&row, "meta_key"),
            &row_str(&row, "meta_value"),
        )
        .await?;
        report.add(
            "termmeta",
            format!("sql:{}", row_i64(&row, "meta_id")),
            if ok { Some(term_id) } else { None },
            if ok {
                ItemStatus::Imported
            } else {
                ItemStatus::Failed
            },
            reason,
        );
    }

    // Relationships (post + term-taxonomy remapped).
    for row in rows_by_table
        .get("wp_term_relationships")
        .cloned()
        .unwrap_or_default()
    {
        let (Some(post_id), Some(tt_id)) = (
            post_map.get(&row_i64(&row, "object_id")),
            tt_map.get(&row_i64(&row, "term_taxonomy_id")),
        ) else {
            report.add(
                "term_relationship",
                format!(
                    "sql:{}:{}",
                    row_i64(&row, "object_id"),
                    row_i64(&row, "term_taxonomy_id")
                ),
                None,
                ItemStatus::Skipped,
                Some("Post or term was not imported.".to_string()),
            );
            continue;
        };
        let (post_id, tt_id) = (*post_id, *tt_id);
        db.prepare(
            "INSERT OR IGNORE INTO wp_term_relationships (site_id, object_id, term_taxonomy_id, term_order) VALUES (?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(site_id),
            Param::Integer(post_id),
            Param::Integer(tt_id),
            Param::Integer(row_i64(&row, "term_order")),
        ])
        .run()
        .await?;
        report.add(
            "term_relationship",
            format!(
                "sql:{}:{}",
                row_i64(&row, "object_id"),
                row_i64(&row, "term_taxonomy_id")
            ),
            Some(post_id),
            ItemStatus::Imported,
            None,
        );
    }

    // Comments (parents resolved in a second pass).
    let mut pending_comment_parents: Vec<(i64, i64)> = Vec::new();
    for row in rows_by_table
        .get("wp_comments")
        .cloned()
        .unwrap_or_default()
    {
        let Some(post_id) = post_map.get(&row_i64(&row, "comment_post_ID")) else {
            report.add(
                "comment",
                format!("sql:{}", row_i64(&row, "comment_ID")),
                None,
                ItemStatus::Skipped,
                Some("Parent post was not imported.".to_string()),
            );
            continue;
        };
        let post_id = *post_id;
        let date = to_iso(&row_str(&row, "comment_date")).unwrap_or_else(now_iso);
        let mut record = Map::new();
        record.insert(
            "comment_ID".to_string(),
            Value::from(row_i64(&row, "comment_ID")),
        );
        record.insert("comment_post_ID".to_string(), Value::from(post_id));
        record.insert(
            "comment_author".to_string(),
            Value::String(row_str(&row, "comment_author")),
        );
        record.insert(
            "comment_author_email".to_string(),
            Value::String(row_str(&row, "comment_author_email")),
        );
        record.insert(
            "comment_author_url".to_string(),
            Value::String(row_str(&row, "comment_author_url")),
        );
        record.insert(
            "comment_author_IP".to_string(),
            Value::String(row_str(&row, "comment_author_IP")),
        );
        record.insert("comment_date".to_string(), Value::String(date.clone()));
        record.insert(
            "comment_date_gmt".to_string(),
            Value::String(
                to_iso(&row_str(&row, "comment_date_gmt")).unwrap_or_else(|| date.clone()),
            ),
        );
        record.insert(
            "comment_content".to_string(),
            Value::String(row_str(&row, "comment_content")),
        );
        record.insert(
            "comment_karma".to_string(),
            Value::from(row_i64(&row, "comment_karma")),
        );
        record.insert(
            "comment_approved".to_string(),
            Value::String(map_comment_approved(&row_str(&row, "comment_approved"))),
        );
        record.insert(
            "comment_agent".to_string(),
            Value::String(row_str(&row, "comment_agent")),
        );
        record.insert(
            "comment_type".to_string(),
            Value::String({
                let t = row_str(&row, "comment_type");
                if t.is_empty() {
                    "comment".to_string()
                } else {
                    t
                }
            }),
        );
        record.insert("comment_parent".to_string(), Value::from(0));
        record.insert(
            "user_id".to_string(),
            Value::from(
                user_map
                    .get(&row_i64(&row, "user_id"))
                    .copied()
                    .unwrap_or(0),
            ),
        );
        let new_id = insert_with_original_id(db, site_id, "wp_comments", "comment_ID", &record).await?;
        comment_map.insert(row_i64(&row, "comment_ID"), new_id);
        if row_i64(&row, "comment_parent") != 0 {
            pending_comment_parents.push((new_id, row_i64(&row, "comment_parent")));
        }
        report.add(
            "comment",
            format!("sql:{}", row_i64(&row, "comment_ID")),
            Some(new_id),
            ItemStatus::Imported,
            None,
        );
    }
    for (new_id, old_parent) in pending_comment_parents {
        if let Some(parent) = comment_map.get(&old_parent) {
            db.prepare("UPDATE wp_comments SET comment_parent = ? WHERE comment_ID = ? AND site_id = ?")
                .bind(vec![Param::Integer(*parent), Param::Integer(new_id), Param::Integer(site_id)])
                .run()
                .await?;
        }
    }
    for row in rows_by_table
        .get("wp_commentmeta")
        .cloned()
        .unwrap_or_default()
    {
        let Some(comment_id) = comment_map.get(&row_i64(&row, "comment_id")) else {
            report.add(
                "commentmeta",
                format!("sql:{}", row_i64(&row, "meta_id")),
                None,
                ItemStatus::Skipped,
                Some("Parent comment was not imported.".to_string()),
            );
            continue;
        };
        let comment_id = *comment_id;
        let (ok, reason) = insert_meta_value(
            db,
            site_id,
            "wp_commentmeta",
            "comment_id",
            comment_id,
            &row_str(&row, "meta_key"),
            &row_str(&row, "meta_value"),
        )
        .await?;
        report.add(
            "commentmeta",
            format!("sql:{}", row_i64(&row, "meta_id")),
            if ok { Some(comment_id) } else { None },
            if ok {
                ItemStatus::Imported
            } else {
                ItemStatus::Failed
            },
            reason,
        );
    }

    // Term counts + comment counts (recomputed from the imported
    // relationships, as in v1).
    // Term counts are recomputed for this site's terms only.
    let tt_rows: Vec<Row> = db
        .prepare("SELECT term_taxonomy_id FROM wp_term_taxonomy WHERE site_id = ?")
        .bind(vec![Param::Integer(site_id)])
        .all()
        .await?;
    for tt in tt_rows {
        let tt_id = row_int(&tt, "term_taxonomy_id");
        let count_row: Option<Row> = db
            .prepare("SELECT COUNT(*) AS n FROM wp_term_relationships WHERE term_taxonomy_id = ? AND site_id = ?")
            .bind(vec![Param::Integer(tt_id), Param::Integer(site_id)])
            .first()
            .await?;
        let n = count_row.as_ref().map_or(0, |r| row_int(r, "n"));
        db.prepare("UPDATE wp_term_taxonomy SET count = ? WHERE term_taxonomy_id = ? AND site_id = ?")
            .bind(vec![Param::Integer(n), Param::Integer(tt_id), Param::Integer(site_id)])
            .run()
            .await?;
    }
    for post_id in touched_posts {
        refresh_comment_count(db, site_id, post_id).await?;
    }

    Ok(report.build("sql", source, parsed.ignored_tables))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeStore, SqliteDb};
    use futures_executor::block_on;
    use wordless_auth::{check_user_password, get_user_meta, PASSWORD_RESET_META_KEY};
    use wordless_db::row::row_text;
    use wordless_db::Row;

    const WXR: &str =
        include_str!("../test-fixtures/wordpress-export.xml");
    const SQL: &str =
        include_str!("../test-fixtures/wordpress-dump.sql");

    struct PngFetcher;

    #[async_trait(?Send)]
    impl AttachmentFetcher for PngFetcher {
        async fn fetch(&self, _url: &str) -> Option<(Vec<u8>, String)> {
            Some((
                vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
                "image/png".to_string(),
            ))
        }
    }

    struct NullFetcher;

    #[async_trait(?Send)]
    impl AttachmentFetcher for NullFetcher {
        async fn fetch(&self, _url: &str) -> Option<(Vec<u8>, String)> {
            None
        }
    }

    fn admin(db: &SqliteDb) -> i64 {
        let meta = block_on(
            db.prepare(
                "INSERT INTO wp_users (user_login, user_pass, user_nicename, user_email, user_registered, display_name) \
                 VALUES ('site-admin', '', 'site-admin', 'sa@example.test', '2026-01-01T00:00:00.000Z', 'Site Admin')",
            )
            .run(),
        )
        .unwrap();
        meta.last_row_id.unwrap_or(0)
    }

    fn count(db: &SqliteDb, sql: &str) -> i64 {
        let row: Option<Row> = block_on(db.prepare(sql).first()).unwrap();
        row.as_ref().map_or(0, |r| row_int(r, "n"))
    }

    #[test]
    fn wxr_import_matches_v1_fixture_counts() {
        let db = SqliteDb::new();
        let admin_id = admin(&db);
        let store = FakeStore::new();
        let report = block_on(import_wxr_with_media(
            &db, 1,
            WXR,
            admin_id,
            &store,
            &PngFetcher,
            "wordpress-export.xml",
        ))
        .unwrap();
        assert_eq!(report.source, "wordpress-export.xml");
        assert_eq!(report.counts.failed, 0, "no failures: {:?}", report.items);
        assert_eq!(report.counts.by_type["user"].imported, 2);
        assert_eq!(report.counts.by_type["term"].imported, 4);
        assert_eq!(
            report.counts.by_type["post"].imported, 3,
            "post + page + attachment"
        );
        assert_eq!(report.counts.by_type["post"].skipped, 1, "the revision");
        assert_eq!(report.counts.by_type["comment"].imported, 2);
        assert_eq!(report.counts.by_type["attachment_media"].imported, 1);
        assert_eq!(report.counts.skipped, 1);

        // Block content preserved verbatim.
        let post: Option<Row> = block_on(
            db.prepare(
                "SELECT post_content, post_date FROM wp_posts WHERE post_name = 'hello-from-wordpress' LIMIT 1",
            )
            .first(),
        )
        .unwrap();
        let post = post.unwrap();
        assert!(row_text(&post, "post_content").contains("<!-- wp:paragraph -->"));
        assert_eq!(row_text(&post, "post_date"), "2026-01-10T10:30:00.000Z");

        // A serialized postmeta value lands as JSON.
        let meta: Option<Row> = block_on(
            db.prepare(
                "SELECT meta_value FROM wp_postmeta WHERE meta_key = '_fixture_settings' LIMIT 1",
            )
            .first(),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&row_text(&meta.unwrap(), "meta_value")).unwrap();
        assert_eq!(parsed, serde_json::json!({"color": "blue", "size": 3}));

        // Category parent resolved (tutorials → news).
        let tut: Option<Row> = block_on(
            db.prepare(
                "SELECT tt.parent FROM wp_terms t JOIN wp_term_taxonomy tt ON tt.term_id = t.term_id WHERE t.slug = 'tutorials' LIMIT 1",
            )
            .first(),
        )
        .unwrap();
        let news: Option<Row> = block_on(
            db.prepare("SELECT term_id FROM wp_terms WHERE slug = 'news' LIMIT 1")
                .first(),
        )
        .unwrap();
        assert_eq!(
            row_int(&tut.unwrap(), "parent"),
            row_int(&news.unwrap(), "term_id")
        );

        // Comment parent remapped (reply 101 → comment 100's new ID).
        let parent: Option<Row> = block_on(
            db.prepare(
                "SELECT comment_ID FROM wp_comments WHERE comment_author = 'Reader One' LIMIT 1",
            )
            .first(),
        )
        .unwrap();
        let reply: Option<Row> = block_on(
            db.prepare(
                "SELECT comment_parent, comment_approved FROM wp_comments WHERE comment_author = 'Reader Two' LIMIT 1",
            )
            .first(),
        )
        .unwrap();
        let reply = reply.unwrap();
        assert_eq!(
            row_int(&reply, "comment_parent"),
            row_int(&parent.unwrap(), "comment_ID")
        );
        assert_eq!(row_text(&reply, "comment_approved"), "0");

        // The attachment file reached storage and the post has the MIME type.
        let att: Option<Row> = block_on(
            db.prepare(
                "SELECT post_mime_type, ID FROM wp_posts WHERE post_type = 'attachment' LIMIT 1",
            )
            .first(),
        )
        .unwrap();
        assert_eq!(row_text(&att.unwrap(), "post_mime_type"), "image/png");
        assert!(store
            .keys()
            .iter()
            .any(|k| k.starts_with("media/imported-")));

        // Imported users cannot log in with any password and carry the flag.
        assert!(
            block_on(check_user_password(&db, "fixture-admin", "anything"))
                .unwrap()
                .is_none()
        );
        let imported: Option<Row> = block_on(
            db.prepare("SELECT ID FROM wp_users WHERE user_login = 'fixture-admin' LIMIT 1")
                .first(),
        )
        .unwrap();
        assert_eq!(
            block_on(get_user_meta::<String>(
                &db,
                row_int(&imported.unwrap(), "ID"),
                PASSWORD_RESET_META_KEY,
                String::new()
            ))
            .unwrap(),
            "1"
        );
    }

    #[test]
    fn wxr_records_a_failed_attachment_fetch_instead_of_dropping_it() {
        let db = SqliteDb::new();
        let admin_id = admin(&db);
        let store = FakeStore::new();
        let report = block_on(import_wxr_with_media(
            &db, 1,
            WXR,
            admin_id,
            &store,
            &NullFetcher,
            "wordpress-export.xml",
        ))
        .unwrap();
        assert_eq!(report.counts.by_type["attachment_media"].failed, 1);
        assert_eq!(report.counts.by_type["post"].imported, 3);
    }

    #[test]
    fn wxr_without_media_records_attachments_skipped() {
        let db = SqliteDb::new();
        let admin_id = admin(&db);
        let report = block_on(import_wxr(&db, 1, WXR, admin_id, "wordpress-export.xml")).unwrap();
        assert_eq!(report.counts.by_type["attachment_media"].skipped, 1);
        assert_eq!(report.counts.by_type["post"].imported, 3);
    }

    #[test]
    fn sql_import_matches_v1_fixture_counts() {
        let db = SqliteDb::new();
        let admin_id = admin(&db);
        let report = block_on(import_sql(&db, 1, SQL, admin_id, "wordpress-dump.sql")).unwrap();
        assert!(report
            .ignored_tables
            .contains(&"wp_custom_plugin".to_string()));
        assert_eq!(report.counts.by_type["user"].imported, 2);
        assert_eq!(report.counts.by_type["post"].imported, 2, "post + page");
        assert_eq!(report.counts.by_type["post"].skipped, 1, "the revision");
        assert_eq!(report.counts.by_type["comment"].imported, 2);
        assert_eq!(report.counts.by_type["link"].imported, 1);
        assert_eq!(report.counts.by_type["term_relationship"].imported, 2);

        // The serialized option lands as JSON.
        let option: Option<Row> = block_on(
            db.prepare(
                "SELECT option_value FROM wp_options WHERE option_name = 'fixture_serialized_option' LIMIT 1",
            )
            .first(),
        )
        .unwrap();
        let parsed: Value =
            serde_json::from_str(&row_text(&option.unwrap(), "option_value")).unwrap();
        assert_eq!(parsed, serde_json::json!({"color": "blue", "size": 3}));

        // The serialized OBJECT option is refused and recorded as failed.
        let failed_option = report
            .items
            .iter()
            .find(|i| i.item_type == "option" && i.source_id == "fixture_object_option")
            .unwrap();
        assert_eq!(failed_option.status, ItemStatus::Failed);
        assert!(failed_option
            .reason
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("object"));

        // The serialized capabilities become a JSON object.
        let caps: Option<Row> = block_on(
            db.prepare(
                "SELECT meta_value FROM wp_usermeta WHERE meta_key = 'wp_capabilities' AND user_id = (SELECT ID FROM wp_users WHERE user_login = 'fixture-admin') LIMIT 1",
            )
            .first(),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&row_text(&caps.unwrap(), "meta_value")).unwrap();
        assert_eq!(parsed, serde_json::json!({"administrator": true}));

        // Term relationships survived (post 10 → News + wordpress).
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS n FROM wp_term_relationships WHERE object_id = (SELECT ID FROM wp_posts WHERE post_name = 'hello-from-wordpress')"
            ),
            2
        );

        // Comment parent remapped.
        let parent: Option<Row> = block_on(
            db.prepare(
                "SELECT comment_ID FROM wp_comments WHERE comment_author = 'Reader One' LIMIT 1",
            )
            .first(),
        )
        .unwrap();
        let reply: Option<Row> = block_on(
            db.prepare("SELECT comment_parent FROM wp_comments WHERE comment_author = 'Reader Two' LIMIT 1")
                .first(),
        )
        .unwrap();
        assert_eq!(
            row_int(&reply.unwrap(), "comment_parent"),
            row_int(&parent.unwrap(), "comment_ID")
        );

        // Imported users cannot log in with the old (phpass) password.
        assert!(
            block_on(check_user_password(&db, "fixture-jane", "anything"))
                .unwrap()
                .is_none()
        );
        let imported: Option<Row> = block_on(
            db.prepare("SELECT ID FROM wp_users WHERE user_login = 'fixture-jane' LIMIT 1")
                .first(),
        )
        .unwrap();
        assert_eq!(
            block_on(get_user_meta::<String>(
                &db,
                row_int(&imported.unwrap(), "ID"),
                PASSWORD_RESET_META_KEY,
                String::new()
            ))
            .unwrap(),
            "1"
        );
    }

    #[test]
    fn sql_import_is_idempotent_on_reimport() {
        let db = SqliteDb::new();
        let admin_id = admin(&db);
        block_on(import_sql(&db, 1, SQL, admin_id, "wordpress-dump.sql")).unwrap();
        let second = block_on(import_sql(&db, 1, SQL, admin_id, "wordpress-dump.sql")).unwrap();
        assert_eq!(second.counts.by_type["user"].skipped, 2);
        assert_eq!(second.counts.by_type["term"].skipped, 4);
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS n FROM wp_users WHERE user_login LIKE 'fixture-%'"
            ),
            2
        );
    }

    #[test]
    fn date_conversion_matches_v1() {
        assert_eq!(
            to_iso("2026-01-10 10:30:00").as_deref(),
            Some("2026-01-10T10:30:00.000Z")
        );
        assert_eq!(
            to_iso("Sat, 10 Jan 2026 10:30:00 +0000").as_deref(),
            Some("2026-01-10T10:30:00.000Z")
        );
        assert_eq!(to_iso(""), None);
        assert_eq!(to_iso("0000-00-00 00:00:00"), None);
        assert_eq!(to_iso("garbage"), None);
    }
}
