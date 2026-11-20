//! Test-only in-memory SQLite `Db` + fakes (dev-dependency
//! `rusqlite`).
//!
//! Mirrors the D1 schema subset this crate queries (migrations
//! 0001 `wl_extensions` + 0010 `report_json`, 0008 `wl_mail_log`,
//! `wp_options`). Adapted from the `wordless-seo` test-support
//! pattern. Values round-trip as JSON values; integers stay
//! integers (D1 returns JSON numbers — the `row::` helpers accept
//! both).

use std::{cell::RefCell, collections::HashMap, sync::Mutex};

use async_trait::async_trait;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;
use wordless_db::{Db, DbError, Param, Row, RunMeta};
use wordless_kernel::{
    error::WordlessError,
    storage::{ListPage, ObjectMeta, ObjectStore},
    time::now_iso,
};

use crate::platform::{HttpClient, HttpError, HttpResponse};

/// Run an async block on the native test thread.
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures_executor::block_on(f)
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
CREATE TABLE wp_options (option_id INTEGER PRIMARY KEY AUTOINCREMENT, option_name TEXT NOT NULL DEFAULT '', option_value TEXT NOT NULL DEFAULT '', autoload TEXT NOT NULL DEFAULT 'yes');
CREATE UNIQUE INDEX idx_wp_options_option_name ON wp_options (option_name);
CREATE TABLE wl_extensions (
  id TEXT PRIMARY KEY, slug TEXT NOT NULL, name TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'plugin', version TEXT NOT NULL DEFAULT '0.1.0',
  status TEXT NOT NULL DEFAULT 'inactive', source TEXT, report_json TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE UNIQUE INDEX idx_wl_extensions_slug ON wl_extensions (slug);
CREATE INDEX idx_wl_extensions_status ON wl_extensions (status);
CREATE TABLE wl_mail_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT, provider TEXT NOT NULL DEFAULT 'mock',
  to_email TEXT NOT NULL DEFAULT '', subject TEXT NOT NULL DEFAULT '',
  body TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'logged',
  created_at TEXT NOT NULL);
CREATE INDEX idx_wl_mail_log_created ON wl_mail_log (created_at DESC);
";

pub struct SqliteDb {
    conn: Mutex<Connection>,
}

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

/// In-memory `ObjectStore` — real bytes, lexicographic listing with
/// cursor pagination, like R2 (same shape as the `wordless-media`
/// test fake).
#[allow(clippy::type_complexity)]
pub struct FakeStore {
    objects: RefCell<HashMap<String, (Vec<u8>, String, String)>>,
}

impl FakeStore {
    pub fn new() -> Self {
        Self {
            objects: RefCell::new(HashMap::new()),
        }
    }

    /// Synchronous test helper: does the key exist?
    pub fn contains(&self, key: &str) -> bool {
        self.objects.borrow().contains_key(key)
    }

    /// Synchronous test helper: number of stored objects.
    pub fn len(&self) -> usize {
        self.objects.borrow().len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
        let total = self.objects.borrow().len();
        Ok((total.min(limit), total > limit))
    }

    async fn get_object(
        &self,
        key: &str,
    ) -> Result<Option<(Vec<u8>, Option<String>)>, WordlessError> {
        Ok(self
            .objects
            .borrow()
            .get(key)
            .map(|(bytes, ct, _)| (bytes.clone(), Some(ct.clone()))))
    }

    async fn put_object(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<(), WordlessError> {
        self.objects.borrow_mut().insert(
            key.to_string(),
            (bytes, content_type.to_string(), now_iso()),
        );
        Ok(())
    }

    async fn delete_object(&self, key: &str) -> Result<(), WordlessError> {
        self.objects.borrow_mut().remove(key);
        Ok(())
    }

    async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, WordlessError> {
        Ok(self
            .objects
            .borrow()
            .get(key)
            .map(|(bytes, ct, uploaded)| ObjectMeta {
                key: key.to_string(),
                size: bytes.len() as u64,
                content_type: Some(ct.clone()),
                uploaded_at: Some(uploaded.clone()),
            }))
    }

    async fn list_objects(
        &self,
        prefix: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<ListPage, WordlessError> {
        let objects = self.objects.borrow();
        let mut keys: Vec<&String> = objects.keys().filter(|k| k.starts_with(prefix)).collect();
        keys.sort();
        let start = cursor.and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
        let limit = if limit == 0 { 1000 } else { limit };
        let end = (start + limit).min(keys.len());
        let page_objects: Vec<ObjectMeta> = keys[start..end]
            .iter()
            .map(|k| {
                let (bytes, ct, uploaded) = &objects[*k];
                ObjectMeta {
                    key: (*k).clone(),
                    size: bytes.len() as u64,
                    content_type: Some(ct.clone()),
                    uploaded_at: Some(uploaded.clone()),
                }
            })
            .collect();
        let truncated = end < keys.len();
        Ok(ListPage {
            objects: page_objects,
            truncated,
            cursor: if truncated {
                Some(end.to_string())
            } else {
                None
            },
        })
    }
}

/// One recorded fake HTTP call.
#[derive(Debug, Clone)]
pub struct FakeHttpCall {
    pub url: String,
    pub bearer: String,
    pub body: Value,
}

/// A fake [`HttpClient`] for tests: returns a fixed status/body and
/// records every call.
pub struct FakeHttpClient {
    status: u16,
    body: String,
    calls: RefCell<Vec<FakeHttpCall>>,
}

impl FakeHttpClient {
    /// Always answers 200 with an empty body.
    pub fn ok() -> Self {
        Self {
            status: 200,
            body: String::new(),
            calls: RefCell::new(Vec::new()),
        }
    }

    /// Always answers with the given non-2xx status and body.
    pub fn failing(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
            calls: RefCell::new(Vec::new()),
        }
    }

    /// Number of calls made.
    pub fn calls(&self) -> usize {
        self.calls.borrow().len()
    }

    /// The most recent call, if any.
    pub fn last_request(&self) -> Option<FakeHttpCall> {
        self.calls.borrow().last().cloned()
    }
}

#[async_trait(?Send)]
impl HttpClient for FakeHttpClient {
    async fn post_json(
        &self,
        url: &str,
        bearer_token: &str,
        body: &Value,
    ) -> Result<HttpResponse, HttpError> {
        self.calls.borrow_mut().push(FakeHttpCall {
            url: url.to_string(),
            bearer: bearer_token.to_string(),
            body: body.clone(),
        });
        Ok(HttpResponse {
            status: self.status,
            body: self.body.clone(),
        })
    }
}
