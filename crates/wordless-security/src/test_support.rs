//! Test-only in-memory SQLite `Db` (dev-dependency `rusqlite`).
//!
//! Adapted from `wordless-seo/src/test_support.rs` (same pattern) for
//! the tables this crate reads: `wp_users`, `wp_usermeta`,
//! `wp_options`, `wl_audit_log` (migration 0001), `wl_login_attempts`
//! and `wl_sessions` (migration 0009 — `wl_sessions` gains
//! `last_seen_at` there; the column is included so the schema matches
//! the real one).

use std::sync::Mutex;

use async_trait::async_trait;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;
use wordless_db::{Db, DbError, Param, Row, RunMeta};

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
CREATE TABLE wp_users (ID INTEGER PRIMARY KEY AUTOINCREMENT, user_login TEXT NOT NULL DEFAULT '', user_pass TEXT NOT NULL DEFAULT '', user_nicename TEXT NOT NULL DEFAULT '', user_email TEXT NOT NULL DEFAULT '', user_url TEXT NOT NULL DEFAULT '', user_registered TEXT NOT NULL DEFAULT '', user_activation_key TEXT NOT NULL DEFAULT '', user_status INTEGER NOT NULL DEFAULT 0, display_name TEXT NOT NULL DEFAULT '');
CREATE TABLE wp_usermeta (umeta_id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL DEFAULT 0, meta_key TEXT, meta_value TEXT, site_id INTEGER NOT NULL DEFAULT 1);
CREATE TABLE wp_options (option_id INTEGER PRIMARY KEY AUTOINCREMENT, option_name TEXT NOT NULL DEFAULT '', option_value TEXT NOT NULL DEFAULT '', autoload TEXT NOT NULL DEFAULT 'yes');
CREATE UNIQUE INDEX idx_wp_options_option_name ON wp_options (option_name);
CREATE TABLE wl_audit_log (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER, action TEXT NOT NULL, object_type TEXT, object_id TEXT, details TEXT, ip_hash TEXT, created_at TEXT NOT NULL);
CREATE TABLE wl_login_attempts (key TEXT PRIMARY KEY, attempts INTEGER NOT NULL DEFAULT 0, first_at TEXT NOT NULL, last_at TEXT NOT NULL, locked_until TEXT, lockout_count INTEGER NOT NULL DEFAULT 0);
CREATE TABLE wl_sessions (id TEXT PRIMARY KEY, user_id INTEGER NOT NULL, token_hash TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL, ip_hash TEXT, user_agent TEXT, revoked INTEGER NOT NULL DEFAULT 0, last_seen_at TEXT);
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
