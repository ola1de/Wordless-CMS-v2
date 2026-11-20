//! Test-only in-memory SQLite `Db` (dev-dependency `rusqlite`).
//!
//! Mirrors the D1 schema subset this crate queries (migration
//! `0012_p6.sql`: `wl_extension_instances`, `wl_extension_runs`,
//! `wl_extension_logs`). Values round-trip as JSON values; integers
//! stay integers (D1 returns JSON numbers — the `row::` helpers
//! accept both).
//!
//! Adapted from `wordless-commerce/src/test_support.rs` (same
//! pattern). One difference: `exec` applies multi-statement SQL via
//! `execute_batch` when no parameters are given, so tests can apply
//! [`SCHEMA_P6`] in one call.

use std::sync::Mutex;

use async_trait::async_trait;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;
use wordless_db::{Db, DbError, Param, Row, RunMeta};

/// The P6 tables (migration `0012_p6.sql`). Keep in sync.
pub const SCHEMA_P6: &str = "
CREATE TABLE wl_extension_instances (
  slug TEXT PRIMARY KEY,
  version TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'loaded' CHECK (status IN ('loaded', 'disabled')),
  permissions_json TEXT NOT NULL DEFAULT '[]',
  script_key TEXT NOT NULL DEFAULT '',
  host_token_hash TEXT NOT NULL DEFAULT '',
  token_epoch INTEGER NOT NULL DEFAULT 0,
  dev_url TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_extension_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL,
  hook TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'ok' CHECK (status IN ('ok', 'error', 'timeout', 'denied', 'skipped_disabled')),
  duration_ms INTEGER NOT NULL DEFAULT 0,
  output_summary TEXT,
  error TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_wl_extension_runs_slug ON wl_extension_runs (slug);
CREATE TABLE wl_extension_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL,
  level TEXT NOT NULL DEFAULT 'info' CHECK (level IN ('info', 'warn', 'error', 'denied')),
  message TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE INDEX idx_wl_extension_logs_slug ON wl_extension_logs (slug);
";

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

const P12_SCHEMA: &str = "\nCREATE TABLE IF NOT EXISTS wl_sites (id INTEGER PRIMARY KEY AUTOINCREMENT, slug TEXT NOT NULL DEFAULT '', domain TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'public', created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '');\nCREATE UNIQUE INDEX IF NOT EXISTS idx_wl_sites_slug ON wl_sites (slug);\nINSERT OR IGNORE INTO wl_sites (id, slug, domain, title, status, created_at, updated_at) VALUES (1, '', '', 'Main Site', 'public', '', '');\nCREATE TABLE IF NOT EXISTS wl_site_options (site_id INTEGER NOT NULL, option_name TEXT NOT NULL, option_value TEXT NOT NULL DEFAULT '', PRIMARY KEY (site_id, option_name));\n";

impl SqliteDb {
    pub fn new() -> Self {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        Self {
            conn: Mutex::new(conn),
        }
    }
}

pub fn test_db() -> SqliteDb {
    SqliteDb::new()
}

pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures_executor::block_on(f)
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
        if params.is_empty() {
            // Multi-statement SQL (schema application) goes through
            // execute_batch; rusqlite's execute() would silently run
            // only the first statement.
            conn.execute_batch(sql)
                .map_err(|e| DbError::D1(e.to_string()))?;
            return Ok(RunMeta {
                last_row_id: Some(conn.last_insert_rowid()),
                changes: 0,
            });
        }
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
