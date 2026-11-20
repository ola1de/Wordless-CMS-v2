//! Test-only in-memory SQLite `Db` (dev-dependency `rusqlite`) plus
//! an in-memory `ObjectStore` fake.
//!
//! Adapted from `wordless-seo/src/test_support.rs` (same pattern) for
//! the tables the backup reads and writes: `wp_posts`, `wp_users`,
//! `wp_options`, `wl_backups`, `wl_jobs` (migrations 0001/0009) and
//! `wl_audit_log` (restore logs here, best-effort).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;
use wordless_db::{Db, DbError, Param, Row, RunMeta};
use wordless_kernel::error::WordlessError;
use wordless_kernel::storage::{ListPage, ObjectMeta, ObjectStore};

pub struct SqliteDb {
    conn: Mutex<Connection>,
}

fn param_to_rusqlite(p: &Param) -> rusqlite::types::Value {
    match p {
        Param::Null => rusqlite::types::Value::Null,
        Param::Integer(n) => rusqlite::types::Value::Integer(*n),
        Param::Real(f) => rusqlite::types::Value::Real(*f),
        Param::Text(s) => rusqlite::types::Value::Text(s.clone()),
        Param::Blob(b) => rusqlite::types::Value::Blob(b.clone()),
    }
}

fn rusqlite_to_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(n) => Value::from(n),
        rusqlite::types::Value::Real(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        rusqlite::types::Value::Text(s) => Value::String(s),
        rusqlite::types::Value::Blob(_) => Value::Null,
    }
}

const SCHEMA: &str = "
CREATE TABLE wp_posts (ID INTEGER PRIMARY KEY AUTOINCREMENT, post_author INTEGER NOT NULL DEFAULT 0, post_date TEXT, post_date_gmt TEXT, post_content TEXT NOT NULL DEFAULT '', post_title TEXT NOT NULL DEFAULT '', post_excerpt TEXT NOT NULL DEFAULT '', post_status TEXT NOT NULL DEFAULT 'publish', comment_status TEXT NOT NULL DEFAULT 'open', ping_status TEXT NOT NULL DEFAULT 'open', post_password TEXT NOT NULL DEFAULT '', post_name TEXT NOT NULL DEFAULT '', post_modified TEXT, post_modified_gmt TEXT, post_content_filtered TEXT NOT NULL DEFAULT '', post_parent INTEGER NOT NULL DEFAULT 0, guid TEXT NOT NULL DEFAULT '', menu_order INTEGER NOT NULL DEFAULT 0, post_type TEXT NOT NULL DEFAULT 'post', post_mime_type TEXT NOT NULL DEFAULT '', comment_count INTEGER NOT NULL DEFAULT 0, site_id INTEGER NOT NULL DEFAULT 1);
CREATE TABLE wp_users (ID INTEGER PRIMARY KEY AUTOINCREMENT, user_login TEXT NOT NULL DEFAULT '', user_pass TEXT NOT NULL DEFAULT '', user_nicename TEXT NOT NULL DEFAULT '', user_email TEXT NOT NULL DEFAULT '', user_url TEXT NOT NULL DEFAULT '', user_registered TEXT NOT NULL DEFAULT '', user_activation_key TEXT NOT NULL DEFAULT '', user_status INTEGER NOT NULL DEFAULT 0, display_name TEXT NOT NULL DEFAULT '');
CREATE TABLE wp_options (option_id INTEGER PRIMARY KEY AUTOINCREMENT, option_name TEXT NOT NULL DEFAULT '', option_value TEXT NOT NULL DEFAULT '', autoload TEXT NOT NULL DEFAULT 'yes');
CREATE UNIQUE INDEX idx_wp_options_option_name ON wp_options (option_name);
CREATE TABLE wl_backups (id TEXT PRIMARY KEY, filename TEXT NOT NULL, size INTEGER NOT NULL DEFAULT 0, checksum TEXT NOT NULL DEFAULT '', tables TEXT NOT NULL DEFAULT '{}', media_count INTEGER NOT NULL DEFAULT 0, media_missing INTEGER NOT NULL DEFAULT 0, verified INTEGER NOT NULL DEFAULT 0, verified_at TEXT, created_at TEXT NOT NULL, created_by INTEGER NOT NULL DEFAULT 0);
CREATE TABLE wl_jobs (id TEXT PRIMARY KEY, kind TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'queued', payload TEXT NOT NULL DEFAULT '{}', result TEXT, attempts INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, run_at TEXT);
CREATE TABLE wl_audit_log (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER, action TEXT NOT NULL, object_type TEXT, object_id TEXT, details TEXT, ip_hash TEXT, created_at TEXT NOT NULL);
";

const P12_SCHEMA: &str = "\nCREATE TABLE IF NOT EXISTS wl_sites (id INTEGER PRIMARY KEY AUTOINCREMENT, slug TEXT NOT NULL DEFAULT '', domain TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'public', created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '');\nCREATE UNIQUE INDEX IF NOT EXISTS idx_wl_sites_slug ON wl_sites (slug);\nINSERT OR IGNORE INTO wl_sites (id, slug, domain, title, status, created_at, updated_at) VALUES (1, '', '', 'Main Site', 'public', '', '');\nCREATE TABLE IF NOT EXISTS wl_site_options (site_id INTEGER NOT NULL, option_name TEXT NOT NULL, option_value TEXT NOT NULL DEFAULT '', PRIMARY KEY (site_id, option_name));\n";

impl SqliteDb {
    pub fn new() -> Self {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(SCHEMA).expect("schema");
        conn.execute_batch(P12_SCHEMA).expect("p12 schema");
        Self {
            conn: Mutex::new(conn),
        }
    }
}

#[async_trait(?Send)]
impl Db for SqliteDb {
    fn prepare(&self, sql: &str) -> wordless_db::Statement<'_> {
        wordless_db::Statement::new(self as &dyn Db, sql.to_string())
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql).map_err(|e| DbError::D1(e.to_string()))?;
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let rows = stmt
            .query_map(
                params_from_iter(params.iter().map(param_to_rusqlite)),
                |row| {
                    let mut obj = serde_json::Map::new();
                    for (idx, col) in cols.iter().enumerate() {
                        let v: rusqlite::types::Value = row.get(idx)?;
                        obj.insert(col.clone(), rusqlite_to_json(v));
                    }
                    Ok(Value::Object(obj))
                },
            )
            .map_err(|e| DbError::D1(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| DbError::D1(e.to_string()))?);
        }
        Ok(out)
    }

    async fn query_one(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, DbError> {
        let rows = self.query_all(sql, params).await?;
        Ok(rows.into_iter().next())
    }

    async fn exec(&self, sql: &str, params: &[Param]) -> Result<RunMeta, DbError> {
        let conn = self.conn.lock().unwrap();
        let changes = conn
            .execute(sql, params_from_iter(params.iter().map(param_to_rusqlite)))
            .map_err(|e| DbError::D1(e.to_string()))?;
        Ok(RunMeta {
            last_row_id: Some(conn.last_insert_rowid()),
            changes: changes as u64,
        })
    }

    async fn batch(&self, stmts: &[(String, Vec<Param>)]) -> Result<(), DbError> {
        for (sql, params) in stmts {
            self.exec(sql, params).await?;
        }
        Ok(())
    }
}

/// In-memory `ObjectStore` fake.
pub struct FakeStore {
    objects: Mutex<HashMap<String, (Vec<u8>, String)>>,
}

impl FakeStore {
    pub fn new() -> Self {
        Self {
            objects: Mutex::new(HashMap::new()),
        }
    }

    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.objects.lock().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }
}

impl Default for FakeStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait(?Send)]
impl ObjectStore for FakeStore {
    async fn count_objects(&self, limit: usize) -> Result<(usize, bool), WordlessError> {
        let n = self.objects.lock().unwrap().len();
        Ok((n.min(limit), n > limit))
    }

    async fn get_object(
        &self,
        key: &str,
    ) -> Result<Option<(Vec<u8>, Option<String>)>, WordlessError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .map(|(b, ct)| (b.clone(), Some(ct.clone()))))
    }

    async fn put_object(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<(), WordlessError> {
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_string(), (bytes, content_type.to_string()));
        Ok(())
    }

    async fn delete_object(&self, key: &str) -> Result<(), WordlessError> {
        self.objects.lock().unwrap().remove(key);
        Ok(())
    }

    async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, WordlessError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .map(|(b, ct)| ObjectMeta {
                key: key.to_string(),
                size: b.len() as u64,
                content_type: Some(ct.clone()),
                uploaded_at: None,
            }))
    }

    async fn list_objects(
        &self,
        prefix: &str,
        limit: usize,
        _cursor: Option<&str>,
    ) -> Result<ListPage, WordlessError> {
        let mut objects: Vec<ObjectMeta> = self
            .objects
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, (b, ct))| ObjectMeta {
                key: k.clone(),
                size: b.len() as u64,
                content_type: Some(ct.clone()),
                uploaded_at: None,
            })
            .collect();
        objects.sort_by(|a, b| a.key.cmp(&b.key));
        let truncated = objects.len() > limit;
        objects.truncate(limit);
        Ok(ListPage {
            objects,
            truncated,
            cursor: None,
        })
    }
}
