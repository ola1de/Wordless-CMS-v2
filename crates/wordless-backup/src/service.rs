//! Backup + restore service (port of v1 `src/modules/backup/service.ts`).

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::Digest;
use wordless_cms::activity::{log_activity, LogActivityInput};
use wordless_db::options::get_option;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, internal_error, not_found, WordlessError};
use wordless_kernel::storage::ObjectStore;
use wordless_kernel::time::now_iso;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const BACKUP_FORMAT_VERSION: &str = "wordless-backup/1";
pub const BACKUP_RETENTION_DEFAULT: i64 = 10;
const MEDIA_MANIFEST_LIMIT: usize = 1000;
const BATCH_SIZE: usize = 50;

/// Tables restored first, in dependency order; anything else follows
/// alphabetically (same as v1 `RESTORE_ORDER`).
const RESTORE_ORDER: &[&str] = &[
    "wp_users",
    "wp_terms",
    "wp_options",
    "wp_links",
    "wp_posts",
    "wp_postmeta",
    "wp_usermeta",
    "wp_term_taxonomy",
    "wp_termmeta",
    "wp_term_relationships",
    "wp_comments",
    "wp_commentmeta",
];

const EXCLUDED_TABLES: &[&str] = &["sqlite_sequence", "wl_backups", "wl_jobs"];

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A random backup id in UUID format (v1 `crypto.randomUUID()`).
/// The 16 random bytes are rendered as 8-4-4-4-12 hex with the UUID
/// version (4) and variant (8/9/a/b) bits set, so ids match v1's
/// format exactly; no UUID crate needed.
fn new_backup_id() -> Result<String, WordlessError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| internal_error(&format!("random backup id failed: {e}")))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSummary {
    pub count: usize,
    pub missing: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub format: String,
    pub wordless_version: String,
    pub created_at: String,
    pub created_by: i64,
    pub tables: BTreeMap<String, u64>,
    pub checksums: BTreeMap<String, String>,
    pub media: MediaSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    pub id: String,
    pub filename: String,
    pub size: u64,
    pub checksum: String,
    pub tables: BTreeMap<String, u64>,
    pub media_count: usize,
    pub media_missing: usize,
    pub verified: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRow {
    pub id: String,
    pub filename: String,
    pub size: u64,
    pub checksum: String,
    pub tables: BTreeMap<String, u64>,
    pub media_count: usize,
    pub media_missing: usize,
    pub verified: bool,
    pub verified_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyFile {
    pub file: String,
    pub expected: String,
    pub actual: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub backup_id: String,
    pub ok: bool,
    pub files: Vec<VerifyFile>,
    pub verified_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub backup_id: String,
    pub restored_tables: Vec<String>,
    pub rows_restored: u64,
    pub missing_media: Vec<String>,
    pub media_checked: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaEntry {
    key: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaManifest {
    count: usize,
    truncated: bool,
    keys: Vec<MediaEntry>,
}

async fn list_backup_tables(db: &dyn Db) -> Result<Vec<String>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name ASC")
        .all()
        .await?;
    Ok(rows
        .iter()
        .map(|r| row_text(r, "name"))
        .filter(|name| !EXCLUDED_TABLES.contains(&name.as_str()))
        .collect())
}

async fn get_backup_row(db: &dyn Db, id: &str) -> Result<Option<BackupRow>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_backups WHERE id = ? LIMIT 1")
        .bind(vec![Param::Text(id.to_string())])
        .first()
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let tables: BTreeMap<String, u64> =
        serde_json::from_str(&row_text(&row, "tables")).unwrap_or_default();
    Ok(Some(BackupRow {
        id: row_text(&row, "id"),
        filename: row_text(&row, "filename"),
        size: row_int(&row, "size") as u64,
        checksum: row_text(&row, "checksum"),
        tables,
        media_count: row_int(&row, "media_count") as usize,
        media_missing: row_int(&row, "media_missing") as usize,
        verified: row_int(&row, "verified") == 1,
        verified_at: match row_text(&row, "verified_at") {
            s if s.is_empty() => None,
            s => Some(s),
        },
        created_at: row_text(&row, "created_at"),
    }))
}

pub async fn list_backups(db: &dyn Db) -> Result<Vec<BackupRow>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare("SELECT * FROM wl_backups ORDER BY created_at DESC, rowid DESC LIMIT 100")
        .all()
        .await?;
    let mut out = Vec::new();
    for row in rows {
        if let Some(parsed) = get_backup_row(db, &row_text(&row, "id")).await? {
            out.push(parsed);
        }
    }
    Ok(out)
}

/// Create a backup now: checksummed JSON bundle + media manifest as
/// one ZIP in R2. Retention pruning runs afterwards.
pub async fn create_backup(
    db: &dyn Db,
    store: &dyn ObjectStore,
    created_by: i64,
    wordless_version: &str,
) -> Result<BackupSummary, WordlessError> {
    let id = new_backup_id()?;
    let created_at = now_iso();
    let filename = format!("backups/{id}.zip");
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut tables: BTreeMap<String, u64> = BTreeMap::new();

    for table in list_backup_tables(db).await? {
        let rows: Vec<Row> = db
            .prepare(&format!("SELECT * FROM \"{table}\""))
            .all()
            .await?;
        tables.insert(table.clone(), rows.len() as u64);
        let json = serde_json::to_string(&rows)
            .map_err(|e| internal_error(&format!("backup JSON failed: {e}")))?;
        files.insert(format!("{table}.json"), json.into_bytes());
    }

    // Media manifest: R2 keys + sizes (R2 gives no SHA-256, and the
    // ObjectStore trait exposes no etag — the manifest records
    // key + size and says so by omitting the etag).
    let page = store.list_objects("", MEDIA_MANIFEST_LIMIT, None).await?;
    let media_entries: Vec<MediaEntry> = page
        .objects
        .iter()
        .map(|o| MediaEntry {
            key: o.key.clone(),
            size: o.size,
        })
        .collect();
    let media_count = media_entries.len();
    let media_manifest = MediaManifest {
        count: media_count,
        truncated: page.truncated,
        keys: media_entries,
    };
    files.insert(
        "media-manifest.json".to_string(),
        serde_json::to_string(&media_manifest)
            .map_err(|e| internal_error(&format!("media manifest JSON failed: {e}")))?
            .into_bytes(),
    );

    let mut checksums: BTreeMap<String, String> = BTreeMap::new();
    for (name, bytes) in &files {
        checksums.insert(name.clone(), sha256_hex(bytes));
    }
    let manifest = BackupManifest {
        format: BACKUP_FORMAT_VERSION.to_string(),
        wordless_version: wordless_version.to_string(),
        created_at: created_at.clone(),
        created_by,
        tables: tables.clone(),
        checksums,
        media: MediaSummary {
            count: media_count,
            missing: 0,
            truncated: page.truncated,
        },
    };
    files.insert(
        "manifest.json".to_string(),
        serde_json::to_string(&manifest)
            .map_err(|e| internal_error(&format!("manifest JSON failed: {e}")))?
            .into_bytes(),
    );

    let zip_bytes = write_zip(&files)?;
    let checksum = sha256_hex(&zip_bytes);
    store
        .put_object(&filename, zip_bytes.clone(), "application/zip")
        .await?;
    db.prepare(
        "INSERT INTO wl_backups (id, filename, size, checksum, tables, media_count, media_missing, verified, verified_at, created_at, created_by) \
         VALUES (?, ?, ?, ?, ?, ?, 0, 0, NULL, ?, ?)",
    )
    .bind(vec![
        Param::Text(id.clone()),
        Param::Text(filename.clone()),
        Param::Integer(zip_bytes.len() as i64),
        Param::Text(checksum.clone()),
        Param::Text(
            serde_json::to_string(&tables)
                .map_err(|e| internal_error(&format!("tables JSON failed: {e}")))?,
        ),
        Param::Integer(media_count as i64),
        Param::Text(created_at.clone()),
        Param::Integer(created_by),
    ])
    .run()
    .await?;
    prune_backups(db, store).await?;
    Ok(BackupSummary {
        id,
        filename,
        size: zip_bytes.len() as u64,
        checksum,
        tables,
        media_count,
        media_missing: 0,
        verified: false,
        created_at,
    })
}

fn write_zip(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, WordlessError> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in files {
        writer
            .start_file(name, options)
            .map_err(|e| internal_error(&format!("ZIP write failed: {e}")))?;
        writer
            .write_all(bytes)
            .map_err(|e| internal_error(&format!("ZIP write failed: {e}")))?;
    }
    let cursor = writer
        .finish()
        .map_err(|e| internal_error(&format!("ZIP finish failed: {e}")))?;
    Ok(cursor.into_inner())
}

/// Read a backup ZIP from storage into a name → bytes map.
/// Returns `None` when the object is missing or unreadable.
async fn read_backup_zip(
    store: &dyn ObjectStore,
    filename: &str,
) -> Option<BTreeMap<String, Vec<u8>>> {
    let (bytes, _) = store.get_object(filename).await.ok()??;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut out = BTreeMap::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).ok()?;
        let name = file.name().to_string();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).ok()?;
        out.insert(name, buf);
    }
    Some(out)
}

pub async fn get_backup_retention(db: &dyn Db) -> Result<i64, WordlessError> {
    let value = get_option::<i64>(db, "wl_backup_retention", BACKUP_RETENTION_DEFAULT)
        .await
        .map_err(|e| WordlessError::from(e))?;
    Ok(if value >= 1 {
        value
    } else {
        BACKUP_RETENTION_DEFAULT
    })
}

/// Keep the N most recent backups (configurable); prune older ones
/// from R2 + the registry. Returns the number pruned.
pub async fn prune_backups(db: &dyn Db, store: &dyn ObjectStore) -> Result<usize, WordlessError> {
    let keep = get_backup_retention(db).await? as usize;
    let rows: Vec<Row> = db
        .prepare("SELECT id, filename FROM wl_backups ORDER BY created_at DESC, rowid DESC")
        .all()
        .await?;
    let stale = &rows[keep.min(rows.len())..];
    let pruned = stale.len();
    for row in stale {
        store.delete_object(&row_text(row, "filename")).await?;
        db.prepare("DELETE FROM wl_backups WHERE id = ?")
            .bind(vec![Param::Text(row_text(row, "id"))])
            .run()
            .await?;
    }
    Ok(pruned)
}

/// Re-read the bundle from R2 and recompute every checksum. Only
/// when every file matches is the backup marked verified.
pub async fn verify_backup_report(
    db: &dyn Db,
    store: &dyn ObjectStore,
    id: &str,
) -> Result<VerifyReport, WordlessError> {
    let row = get_backup_row(db, id)
        .await?
        .ok_or_else(|| not_found("Backup not found."))?;
    let zip = read_backup_zip(store, &row.filename).await;
    let verified_at = now_iso();
    let mut files: Vec<VerifyFile> = Vec::new();
    let mut ok = zip.is_some();
    match &zip {
        None => {
            files.push(VerifyFile {
                file: "(bundle)".to_string(),
                expected: "readable ZIP".to_string(),
                actual: None,
                ok: false,
            });
        }
        Some(zip) => match zip.get("manifest.json") {
            None => {
                ok = false;
                files.push(VerifyFile {
                    file: "manifest.json".to_string(),
                    expected: "(present)".to_string(),
                    actual: None,
                    ok: false,
                });
            }
            Some(manifest_raw) => {
                let manifest: BackupManifest = serde_json::from_slice(manifest_raw)
                    .map_err(|e| bad_request(&format!("Backup manifest is invalid: {e}")))?;
                for (name, expected) in &manifest.checksums {
                    let actual = zip.get(name).map(|b| sha256_hex(b));
                    let file_ok = actual.as_deref() == Some(expected.as_str());
                    if !file_ok {
                        ok = false;
                    }
                    files.push(VerifyFile {
                        file: name.clone(),
                        expected: expected.clone(),
                        actual,
                        ok: file_ok,
                    });
                }
            }
        },
    }
    db.prepare("UPDATE wl_backups SET verified = ?, verified_at = ? WHERE id = ?")
        .bind(vec![
            Param::Integer(i64::from(ok)),
            Param::Text(verified_at.clone()),
            Param::Text(id.to_string()),
        ])
        .run()
        .await?;
    Ok(VerifyReport {
        backup_id: id.to_string(),
        ok,
        files,
        verified_at,
    })
}

/// Checksum check for a backup: re-reads the ZIP from storage and
/// recomputes every checksum. Returns `true` only when every file
/// matches (and marks the registry row accordingly).
pub async fn verify_backup(
    db: &dyn Db,
    store: &dyn ObjectStore,
    id: &str,
) -> Result<bool, WordlessError> {
    Ok(verify_backup_report(db, store, id).await?.ok)
}

fn minor_version(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// Restore a verified backup. Requires the typed confirmation
/// `RESTORE`, a passing checksum verification, and a matching
/// Wordless minor version. The checksum is verified BEFORE anything
/// is restored. Tables are restored in dependency order; the media
/// manifest is checked key-by-key against R2.
pub async fn restore_backup(
    db: &dyn Db,
    store: &dyn ObjectStore,
    id: &str,
    confirmation: &str,
    wordless_version: &str,
    actor_id: i64,
) -> Result<RestoreReport, WordlessError> {
    if confirmation != "RESTORE" {
        return Err(bad_request(
            "Restore requires the confirmation string \"RESTORE\" in the request body.",
        ));
    }
    let row = get_backup_row(db, id)
        .await?
        .ok_or_else(|| not_found("Backup not found."))?;
    let verification = verify_backup_report(db, store, id).await?;
    if !verification.ok {
        return Err(bad_request(
            "This backup failed checksum verification and cannot be restored.",
        ));
    }
    let zip = read_backup_zip(store, &row.filename)
        .await
        .ok_or_else(|| bad_request("The backup file is missing from storage."))?;
    let manifest_raw = zip
        .get("manifest.json")
        .ok_or_else(|| bad_request("The backup is missing its manifest."))?;
    let manifest: BackupManifest = serde_json::from_slice(manifest_raw)
        .map_err(|e| bad_request(&format!("Backup manifest is invalid: {e}")))?;
    if minor_version(&manifest.wordless_version) != minor_version(wordless_version) {
        return Err(bad_request(&format!(
            "This backup is from Wordless {} and cannot be restored into {} (minor versions must match).",
            manifest.wordless_version, wordless_version
        )));
    }

    let mut table_names: Vec<String> = manifest.tables.keys().cloned().collect();
    table_names.sort_by(|a, b| {
        let ai = RESTORE_ORDER.iter().position(|t| t == a);
        let bi = RESTORE_ORDER.iter().position(|t| t == b);
        match (ai, bi) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.cmp(b),
        }
    });
    let mut restored_tables: Vec<String> = Vec::new();
    let mut rows_restored: u64 = 0;
    for table in &table_names {
        let Some(raw) = zip.get(&format!("{table}.json")) else {
            continue;
        };
        let rows: Vec<Map<String, Value>> = serde_json::from_slice(raw)
            .map_err(|e| bad_request(&format!("Backup table {table} is invalid JSON: {e}")))?;
        let mut statements: Vec<(String, Vec<Param>)> =
            vec![(format!("DELETE FROM \"{table}\""), Vec::new())];
        for record in &rows {
            let columns: Vec<&String> = record.keys().collect();
            if columns.is_empty() {
                continue;
            }
            statements.push((
                format!(
                    "INSERT INTO \"{table}\" ({}) VALUES ({})",
                    columns
                        .iter()
                        .map(|c| format!("\"{c}\""))
                        .collect::<Vec<_>>()
                        .join(", "),
                    columns.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
                ),
                columns
                    .iter()
                    .map(|c| value_to_param(record.get(*c).unwrap_or(&Value::Null)))
                    .collect(),
            ));
        }
        for chunk in statements.chunks(BATCH_SIZE) {
            db.batch(chunk).await?;
        }
        restored_tables.push(table.clone());
        rows_restored += rows.len() as u64;
    }

    // Media manifest: a check-list, not a copy. Report every missing key.
    let mut missing_media: Vec<String> = Vec::new();
    let mut media_checked = 0;
    if let Some(media_raw) = zip.get("media-manifest.json") {
        if let Ok(media_manifest) = serde_json::from_slice::<MediaManifest>(media_raw) {
            for entry in media_manifest.keys {
                media_checked += 1;
                if store.head_object(&entry.key).await?.is_none() {
                    missing_media.push(entry.key);
                }
            }
        }
    }

    // Best-effort audit entry: logging must never fail a restore
    // that already succeeded.
    let _ = log_activity(
        db,
        LogActivityInput {
            actor_id,
            action: "backup.restore".to_string(),
            object_type: Some("backup".to_string()),
            object_id: Some(id.to_string()),
            details: Some(format!(
                "Restored backup {id}: {rows_restored} rows across {} tables; {} media keys missing.",
                restored_tables.len(),
                missing_media.len()
            )),
            ip_hash: None,
        },
    )
    .await;
    Ok(RestoreReport {
        backup_id: id.to_string(),
        restored_tables,
        rows_restored,
        missing_media,
        media_checked,
    })
}

pub async fn get_backup_download(
    db: &dyn Db,
    store: &dyn ObjectStore,
    id: &str,
) -> Result<(String, Vec<u8>, u64), WordlessError> {
    let row = get_backup_row(db, id)
        .await?
        .ok_or_else(|| not_found("Backup not found."))?;
    let (bytes, _) = store
        .get_object(&row.filename)
        .await?
        .ok_or_else(|| not_found("The backup file is missing from storage."))?;
    let size = bytes.len() as u64;
    Ok((format!("{id}.zip"), bytes, size))
}

/// Run a backup as a `wl_jobs` job (same shape as the v1 conversion
/// jobs): payload `{createdBy}`, result is the [`BackupSummary`].
pub async fn run_backup_job(
    db: &dyn Db,
    store: &dyn ObjectStore,
    job_id: &str,
    wordless_version: &str,
) -> Result<(), WordlessError> {
    let job: Option<Row> = db
        .prepare("SELECT id, payload FROM wl_jobs WHERE id = ? LIMIT 1")
        .bind(vec![Param::Text(job_id.to_string())])
        .first()
        .await?;
    let Some(job) = job else {
        return Ok(());
    };
    let payload: Value = serde_json::from_str(&row_text(&job, "payload")).unwrap_or(Value::Null);
    let created_by = payload
        .get("createdBy")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    db.prepare("UPDATE wl_jobs SET status = 'running', updated_at = ? WHERE id = ?")
        .bind(vec![
            Param::Text(now_iso()),
            Param::Text(job_id.to_string()),
        ])
        .run()
        .await?;
    match create_backup(db, store, created_by, wordless_version).await {
        Ok(summary) => {
            let result = serde_json::to_string(&summary)
                .map_err(|e| internal_error(&format!("backup job result JSON failed: {e}")))?;
            db.prepare(
                "UPDATE wl_jobs SET status = 'done', result = ?, updated_at = ? WHERE id = ?",
            )
            .bind(vec![
                Param::Text(result),
                Param::Text(now_iso()),
                Param::Text(job_id.to_string()),
            ])
            .run()
            .await?;
            Ok(())
        }
        Err(e) => {
            let result = serde_json::to_string(&serde_json::json!({ "error": e.message }))
                .map_err(|e| internal_error(&format!("backup job result JSON failed: {e}")))?;
            db.prepare(
                "UPDATE wl_jobs SET status = 'failed', result = ?, updated_at = ? WHERE id = ?",
            )
            .bind(vec![
                Param::Text(result),
                Param::Text(now_iso()),
                Param::Text(job_id.to_string()),
            ])
            .run()
            .await?;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeStore, SqliteDb};
    use futures_executor::block_on;
    use std::io::Cursor;

    fn seed_content(db: &SqliteDb) -> i64 {
        block_on(
            db.prepare(
                "INSERT INTO wp_users (user_login, user_pass, user_nicename, user_email, user_registered, display_name) \
                 VALUES ('backup-user', '', 'backup-user', 'bu@example.test', '2026-01-01T00:00:00.000Z', 'Backup User')",
            )
            .run(),
        )
        .unwrap();
        let meta = block_on(
            db.prepare(
                "INSERT INTO wp_posts (post_author, post_date, post_date_gmt, post_content, post_title, post_status, post_name, post_modified, post_modified_gmt, post_type) \
                 VALUES (1, '2026-01-10T10:30:00.000Z', '2026-01-10T10:30:00.000Z', '<p>Content worth backing up.</p>', 'Backup fixture post', 'publish', 'backup-fixture-post', '2026-01-10T10:30:00.000Z', '2026-01-10T10:30:00.000Z', 'post')",
            )
            .run(),
        )
        .unwrap();
        block_on(
            db.prepare(
                "INSERT INTO wp_options (option_name, option_value, autoload) VALUES ('blogname', '\"Backup Fixture Site\"', 'yes')",
            )
            .run(),
        )
        .unwrap();
        meta.last_row_id.unwrap_or(0)
    }

    fn set_option(db: &SqliteDb, name: &str, json_value: &str) {
        block_on(
            db.prepare(
                "INSERT INTO wp_options (option_name, option_value, autoload) VALUES (?, ?, 'yes') \
                 ON CONFLICT(option_name) DO UPDATE SET option_value = excluded.option_value",
            )
            .bind(vec![
                Param::Text(name.to_string()),
                Param::Text(json_value.to_string()),
            ])
            .run(),
        )
        .unwrap();
    }

    fn read_zip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
        let mut archive = ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        let mut out = BTreeMap::new();
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();
            let name = file.name().to_string();
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).unwrap();
            out.insert(name, buf);
        }
        out
    }

    fn write_zip_raw(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, bytes) in files {
            writer.start_file(name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn round_trips_real_rows_and_marks_verified_only_after_a_reread() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        let post_id = seed_content(&db);
        let summary = block_on(create_backup(&db, &store, 1, "0.9.0")).unwrap();
        assert!(!summary.verified);
        assert!(summary.tables["wp_posts"] > 0);
        assert!(summary.tables["wp_users"] > 0);
        assert_eq!(summary.checksum.len(), 64);
        assert!(summary.checksum.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(store.keys().contains(&summary.filename));

        let listed = block_on(list_backups(&db)).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].verified);

        let verification = block_on(verify_backup_report(&db, &store, &summary.id)).unwrap();
        assert!(verification.ok);
        assert!(verification.files.iter().all(|f| f.ok));
        assert!(verification.files.iter().any(|f| f.file == "wp_posts.json"));
        assert!(verification
            .files
            .iter()
            .any(|f| f.file == "media-manifest.json"));
        assert!(block_on(list_backups(&db)).unwrap()[0].verified);
        assert!(block_on(verify_backup(&db, &store, &summary.id)).unwrap());

        // Download returns the same ZIP bytes.
        let (filename, bytes, size) =
            block_on(get_backup_download(&db, &store, &summary.id)).unwrap();
        assert_eq!(bytes.len() as u64, summary.size);
        assert_eq!(size, summary.size);
        assert_eq!(filename, format!("{}.zip", summary.id));

        // Delete the content, then restore it.
        block_on(
            db.prepare("DELETE FROM wp_posts WHERE ID = ?")
                .bind(vec![Param::Integer(post_id)])
                .run(),
        )
        .unwrap();
        let n: Option<Row> =
            block_on(db.prepare("SELECT COUNT(*) AS n FROM wp_posts").first()).unwrap();
        assert_eq!(row_int(&n.unwrap(), "n"), 0);
        let report = block_on(restore_backup(
            &db,
            &store,
            &summary.id,
            "RESTORE",
            "0.9.0",
            1,
        ))
        .unwrap();
        assert!(report.restored_tables.contains(&"wp_posts".to_string()));
        assert!(report.rows_restored > 0);
        assert!(report.missing_media.is_empty());
        let restored: Option<Row> = block_on(
            db.prepare("SELECT post_title FROM wp_posts WHERE ID = ?")
                .bind(vec![Param::Integer(post_id)])
                .first(),
        )
        .unwrap();
        assert_eq!(
            row_text(&restored.unwrap(), "post_title"),
            "Backup fixture post"
        );
    }

    #[test]
    fn detects_a_checksum_tamper_and_refuses_to_restore() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        seed_content(&db);
        let summary = block_on(create_backup(&db, &store, 1, "0.9.0")).unwrap();
        let stored = block_on(store.get_object(&summary.filename))
            .unwrap()
            .unwrap()
            .0;
        let mut files = read_zip(&stored);
        files.insert(
            "wp_posts.json".to_string(),
            b"[{\"ID\":999,\"post_title\":\"tampered\"}]".to_vec(),
        );
        let tampered = write_zip_raw(&files);
        block_on(store.put_object(&summary.filename, tampered, "application/zip")).unwrap();

        let verification = block_on(verify_backup_report(&db, &store, &summary.id)).unwrap();
        assert!(!verification.ok);
        assert!(
            !verification
                .files
                .iter()
                .find(|f| f.file == "wp_posts.json")
                .unwrap()
                .ok
        );
        assert!(
            verification
                .files
                .iter()
                .find(|f| f.file == "media-manifest.json")
                .unwrap()
                .ok
        );
        assert!(!block_on(list_backups(&db)).unwrap()[0].verified);

        let err = block_on(restore_backup(
            &db,
            &store,
            &summary.id,
            "RESTORE",
            "0.9.0",
            1,
        ))
        .unwrap_err();
        assert!(err.message.contains("failed checksum verification"));
    }

    #[test]
    fn rejects_restore_without_the_typed_confirmation() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        seed_content(&db);
        let summary = block_on(create_backup(&db, &store, 1, "0.9.0")).unwrap();
        block_on(verify_backup(&db, &store, &summary.id)).unwrap();
        for bad in ["", "restore", "Restore", "RESTORE "] {
            let err =
                block_on(restore_backup(&db, &store, &summary.id, bad, "0.9.0", 1)).unwrap_err();
            assert!(
                err.message.contains("RESTORE"),
                "input {bad:?} was accepted"
            );
        }
    }

    #[test]
    fn refuses_a_backup_from_a_different_minor_version() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        seed_content(&db);
        let summary = block_on(create_backup(&db, &store, 1, "0.9.0")).unwrap();
        block_on(verify_backup(&db, &store, &summary.id)).unwrap();
        let err = block_on(restore_backup(
            &db,
            &store,
            &summary.id,
            "RESTORE",
            "0.8.0",
            1,
        ))
        .unwrap_err();
        assert!(err.message.contains("minor versions must match"));
    }

    #[test]
    fn reports_missing_media_keys_instead_of_silently_skipping() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        seed_content(&db);
        block_on(store.put_object("media/ghost.png", b"not-a-png".to_vec(), "image/png")).unwrap();
        let summary = block_on(create_backup(&db, &store, 1, "0.9.0")).unwrap();
        block_on(store.delete_object("media/ghost.png")).unwrap();
        block_on(verify_backup(&db, &store, &summary.id)).unwrap();
        let report = block_on(restore_backup(
            &db,
            &store,
            &summary.id,
            "RESTORE",
            "0.9.0",
            1,
        ))
        .unwrap();
        assert!(report
            .missing_media
            .contains(&"media/ghost.png".to_string()));
        assert!(report.media_checked > 0);
    }

    #[test]
    fn prunes_older_backups_beyond_the_retention_limit() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        seed_content(&db);
        set_option(&db, "wl_backup_retention", "2");
        let first = block_on(create_backup(&db, &store, 1, "0.9.0")).unwrap();
        let second = block_on(create_backup(&db, &store, 1, "0.9.0")).unwrap();
        block_on(create_backup(&db, &store, 1, "0.9.0")).unwrap();
        let listed = block_on(list_backups(&db)).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(!listed.iter().any(|b| b.id == first.id));
        assert!(listed.iter().any(|b| b.id == second.id));
        assert!(!store.keys().contains(&first.filename));
    }

    #[test]
    fn runs_as_a_wl_jobs_job() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        seed_content(&db);
        let job_id = "job-backup-1";
        let now = now_iso();
        block_on(
            db.prepare(
                "INSERT INTO wl_jobs (id, kind, status, payload, result, attempts, created_at, updated_at) \
                 VALUES (?, 'backup_create', 'queued', ?, NULL, 0, ?, ?)",
            )
            .bind(vec![
                Param::Text(job_id.to_string()),
                Param::Text("{\"createdBy\":1}".to_string()),
                Param::Text(now.clone()),
                Param::Text(now),
            ])
            .run(),
        )
        .unwrap();
        block_on(run_backup_job(&db, &store, job_id, "0.9.0")).unwrap();
        let job: Option<Row> = block_on(
            db.prepare("SELECT status, result FROM wl_jobs WHERE id = ?")
                .bind(vec![Param::Text(job_id.to_string())])
                .first(),
        )
        .unwrap();
        let job = job.unwrap();
        assert_eq!(row_text(&job, "status"), "done");
        let result: Value = serde_json::from_str(&row_text(&job, "result")).unwrap();
        assert!(result["tables"]["wp_posts"].as_u64().unwrap_or(0) > 0);
        assert!(block_on(list_backups(&db))
            .unwrap()
            .iter()
            .any(|b| b.id == result["id"].as_str().unwrap_or("")));
    }

    #[test]
    fn stores_a_manifest_with_per_file_checksums_and_a_media_manifest() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        seed_content(&db);
        block_on(store.put_object("media/a.png", b"png-bytes".to_vec(), "image/png")).unwrap();
        let summary = block_on(create_backup(&db, &store, 1, "0.9.0")).unwrap();
        let stored = block_on(store.get_object(&summary.filename))
            .unwrap()
            .unwrap()
            .0;
        let files = read_zip(&stored);
        let manifest: BackupManifest = serde_json::from_slice(&files["manifest.json"]).unwrap();
        assert_eq!(manifest.format, "wordless-backup/1");
        assert_eq!(manifest.wordless_version, "0.9.0");
        assert!(manifest.checksums.contains_key("wp_posts.json"));
        assert!(manifest.media.count > 0);
        let media_manifest: MediaManifest =
            serde_json::from_slice(&files["media-manifest.json"]).unwrap();
        assert!(media_manifest.keys.iter().any(|k| k.key == "media/a.png"));
        // The backup ZIP itself is stored after the manifest is built,
        // so it is (correctly) not listed in its own media manifest.
        assert!(!media_manifest
            .keys
            .iter()
            .any(|k| k.key == summary.filename));
    }
}
