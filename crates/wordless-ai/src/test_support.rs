//! Test-only in-memory SQLite `Db` (dev-dependency `rusqlite`).
//!
//! Mirrors the D1 schema subset these crates query (migrations
//! 0001/0002/0005/0006/0008/0009: `wp_posts`, `wp_postmeta`,
//! `wp_options`, `wp_users`, `wp_usermeta`, `wp_terms`,
//! `wp_term_taxonomy`, `wp_term_relationships`, `wp_comments`,
//! `wl_jobs`, `wl_ai_approvals`, `wl_ai_memory`, `wl_extensions`,
//! `wl_products`, `wl_orders`, `wl_backups`, `wl_sessions`,
//! `wl_login_attempts`, `wl_themes`, `wl_media_derivatives`).
//! Values round-trip as JSON values; integers stay integers (D1
//! returns JSON numbers — the `row::` helpers accept both).
//!
//! Adapted from `wordless-seo/src/test_support.rs` (same pattern).

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
CREATE TABLE wp_posts (
  ID INTEGER PRIMARY KEY AUTOINCREMENT, post_author INTEGER NOT NULL DEFAULT 0,
  post_date TEXT, post_date_gmt TEXT, post_content TEXT NOT NULL DEFAULT '',
  post_title TEXT NOT NULL DEFAULT '', post_excerpt TEXT NOT NULL DEFAULT '',
  post_status TEXT NOT NULL DEFAULT 'publish', comment_status TEXT NOT NULL DEFAULT 'open',
  ping_status TEXT NOT NULL DEFAULT 'open', post_password TEXT NOT NULL DEFAULT '',
  post_name TEXT NOT NULL DEFAULT '', post_modified TEXT, post_modified_gmt TEXT,
  post_content_filtered TEXT NOT NULL DEFAULT '', post_parent INTEGER NOT NULL DEFAULT 0,
  guid TEXT NOT NULL DEFAULT '', menu_order INTEGER NOT NULL DEFAULT 0,
  post_type TEXT NOT NULL DEFAULT 'post', post_mime_type TEXT NOT NULL DEFAULT '',
  comment_count INTEGER NOT NULL DEFAULT 0, site_id INTEGER NOT NULL DEFAULT 1);
CREATE TABLE wp_postmeta (meta_id INTEGER PRIMARY KEY AUTOINCREMENT, post_id INTEGER NOT NULL DEFAULT 0, meta_key TEXT, meta_value TEXT, site_id INTEGER NOT NULL DEFAULT 1);
CREATE TABLE wp_users (ID INTEGER PRIMARY KEY AUTOINCREMENT, user_login TEXT NOT NULL DEFAULT '', user_pass TEXT NOT NULL DEFAULT '', user_nicename TEXT NOT NULL DEFAULT '', user_email TEXT NOT NULL DEFAULT '', user_url TEXT NOT NULL DEFAULT '', user_registered TEXT NOT NULL DEFAULT '', user_activation_key TEXT NOT NULL DEFAULT '', user_status INTEGER NOT NULL DEFAULT 0, display_name TEXT NOT NULL DEFAULT '');
CREATE TABLE wp_usermeta (umeta_id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL DEFAULT 0, meta_key TEXT, meta_value TEXT, site_id INTEGER NOT NULL DEFAULT 1);
CREATE TABLE wp_terms (term_id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL DEFAULT '', slug TEXT NOT NULL DEFAULT '', term_group INTEGER NOT NULL DEFAULT 0, site_id INTEGER NOT NULL DEFAULT 1);
CREATE TABLE wp_term_taxonomy (term_taxonomy_id INTEGER PRIMARY KEY AUTOINCREMENT, term_id INTEGER NOT NULL DEFAULT 0, taxonomy TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '', parent INTEGER NOT NULL DEFAULT 0, count INTEGER NOT NULL DEFAULT 0, site_id INTEGER NOT NULL DEFAULT 1);
CREATE TABLE wp_term_relationships (object_id INTEGER NOT NULL DEFAULT 0, term_taxonomy_id INTEGER NOT NULL DEFAULT 0, term_order INTEGER NOT NULL DEFAULT 0, site_id INTEGER NOT NULL DEFAULT 1, PRIMARY KEY (object_id, term_taxonomy_id));
CREATE TABLE wp_comments (comment_ID INTEGER PRIMARY KEY AUTOINCREMENT, comment_post_ID INTEGER NOT NULL DEFAULT 0, comment_author TEXT NOT NULL DEFAULT '', comment_author_email TEXT NOT NULL DEFAULT '', comment_author_url TEXT NOT NULL DEFAULT '', comment_author_IP TEXT NOT NULL DEFAULT '', comment_date TEXT NOT NULL DEFAULT '', comment_date_gmt TEXT NOT NULL DEFAULT '', comment_content TEXT NOT NULL DEFAULT '', comment_karma INTEGER NOT NULL DEFAULT 0, comment_approved TEXT NOT NULL DEFAULT '1', comment_agent TEXT NOT NULL DEFAULT '', comment_type TEXT NOT NULL DEFAULT 'comment', comment_parent INTEGER NOT NULL DEFAULT 0, user_id INTEGER NOT NULL DEFAULT 0, site_id INTEGER NOT NULL DEFAULT 1);
CREATE TABLE wp_options (option_id INTEGER PRIMARY KEY AUTOINCREMENT, option_name TEXT NOT NULL DEFAULT '', option_value TEXT NOT NULL DEFAULT '', autoload TEXT NOT NULL DEFAULT 'yes');
CREATE UNIQUE INDEX idx_wp_options_option_name ON wp_options (option_name);
CREATE TABLE wl_jobs (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'queued',
  payload TEXT NOT NULL DEFAULT '{}', result TEXT, attempts INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL, run_at TEXT);
CREATE INDEX idx_wl_jobs_kind ON wl_jobs (kind);
CREATE TABLE wl_ai_approvals (
  id TEXT PRIMARY KEY, task_id TEXT NOT NULL, agent TEXT NOT NULL, action TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending', created_by INTEGER NOT NULL, decided_by INTEGER,
  decided_at TEXT, reason TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE INDEX idx_wl_ai_approvals_status ON wl_ai_approvals (status, created_at);
CREATE TABLE wl_ai_memory (
  id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL,
  key TEXT NOT NULL UNIQUE, value TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'system', updated_at TEXT NOT NULL);
CREATE TABLE wl_extensions (
  id TEXT PRIMARY KEY, slug TEXT NOT NULL, name TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'plugin', version TEXT NOT NULL DEFAULT '0.1.0',
  status TEXT NOT NULL DEFAULT 'inactive', source TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE UNIQUE INDEX idx_wl_extensions_slug ON wl_extensions (slug);
CREATE TABLE wl_products (
  id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, slug TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '', price_minor INTEGER NOT NULL DEFAULT 0,
  currency TEXT NOT NULL DEFAULT 'NGN', stock INTEGER NOT NULL DEFAULT -1,
  status TEXT NOT NULL DEFAULT 'draft', image_id INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE wl_orders (
  id TEXT PRIMARY KEY, customer_name TEXT NOT NULL DEFAULT '',
  customer_email TEXT NOT NULL DEFAULT '', total_minor INTEGER NOT NULL DEFAULT 0,
  currency TEXT NOT NULL DEFAULT 'NGN', status TEXT NOT NULL DEFAULT 'pending',
  payment_provider TEXT NOT NULL DEFAULT '', payment_reference TEXT NOT NULL DEFAULT '',
  stock_applied INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE wl_backups (
  id TEXT PRIMARY KEY, filename TEXT NOT NULL, size INTEGER NOT NULL DEFAULT 0,
  checksum TEXT NOT NULL DEFAULT '', tables TEXT NOT NULL DEFAULT '{}',
  media_count INTEGER NOT NULL DEFAULT 0, media_missing INTEGER NOT NULL DEFAULT 0,
  verified INTEGER NOT NULL DEFAULT 0, verified_at TEXT, created_at TEXT NOT NULL,
  created_by INTEGER NOT NULL DEFAULT 0);
CREATE TABLE wl_sessions (
  id TEXT PRIMARY KEY, user_id INTEGER NOT NULL, token_hash TEXT NOT NULL,
  created_at TEXT NOT NULL, expires_at TEXT NOT NULL, ip_hash TEXT, user_agent TEXT,
  revoked INTEGER NOT NULL DEFAULT 0, last_seen_at TEXT);
CREATE TABLE wl_login_attempts (
  key TEXT PRIMARY KEY, attempts INTEGER NOT NULL DEFAULT 0,
  first_at TEXT NOT NULL, last_at TEXT NOT NULL, locked_until TEXT,
  lockout_count INTEGER NOT NULL DEFAULT 0);
CREATE TABLE wl_themes (
  slug TEXT PRIMARY KEY, name TEXT NOT NULL, version TEXT NOT NULL DEFAULT '0.1.0',
  author TEXT NOT NULL DEFAULT '', kind TEXT NOT NULL DEFAULT 'block',
  status TEXT NOT NULL DEFAULT 'inactive', theme_json TEXT,
  stylesheet TEXT NOT NULL DEFAULT '', stylesheet_hash TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL DEFAULT 'import', report_json TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE wl_media_derivatives (
  id INTEGER PRIMARY KEY AUTOINCREMENT, media_id INTEGER NOT NULL,
  width INTEGER NOT NULL, format TEXT NOT NULL, object_key TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL,
  UNIQUE (media_id, width, format));
CREATE TABLE wl_audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER,
  action TEXT NOT NULL, object_type TEXT, object_id TEXT, details TEXT,
  ip_hash TEXT, created_at TEXT NOT NULL);
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

    /// Insert a post row directly (mirrors `insertPostRow` defaults).
    /// Returns the new post ID.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_post(
        &self,
        title: &str,
        content: &str,
        excerpt: &str,
        status: &str,
        slug: &str,
        post_type: &str,
        date: &str,
        author_id: i64,
    ) -> i64 {
        let meta = self
            .prepare(
                "INSERT INTO wp_posts (post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt, post_status, post_name, post_modified, post_modified_gmt, post_parent, post_type, post_mime_type, comment_status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, '', 'open')",
            )
            .bind(vec![
                Param::Integer(author_id),
                Param::Text(date.to_string()),
                Param::Text(date.to_string()),
                Param::Text(content.to_string()),
                Param::Text(title.to_string()),
                Param::Text(excerpt.to_string()),
                Param::Text(status.to_string()),
                Param::Text(slug.to_string()),
                Param::Text(date.to_string()),
                Param::Text(date.to_string()),
                Param::Text(post_type.to_string()),
            ])
            .run()
            .await
            .expect("insert post");
        meta.last_row_id.expect("post id")
    }

    pub async fn set_option_raw(&self, name: &str, json_value: &str) {
        self.prepare(
            "INSERT INTO wp_options (option_name, option_value, autoload) VALUES (?, ?, 'yes') ON CONFLICT(option_name) DO UPDATE SET option_value = excluded.option_value",
        )
        .bind(vec![Param::Text(name.to_string()), Param::Text(json_value.to_string())])
        .run()
        .await
        .expect("set option");
    }

    /// Insert a user row directly. Returns the new user ID.
    pub async fn insert_user(&self, login: &str, email: &str, display_name: &str) -> i64 {
        let meta = self
            .prepare(
                "INSERT INTO wp_users (user_login, user_pass, user_nicename, user_email, user_registered, display_name) VALUES (?, 'pbkdf2$100000$c2FsdA$hash', ?, ?, '2026-01-01T00:00:00.000Z', ?)",
            )
            .bind(vec![
                Param::Text(login.to_string()),
                Param::Text(login.to_string()),
                Param::Text(email.to_string()),
                Param::Text(display_name.to_string()),
            ])
            .run()
            .await
            .expect("insert user");
        meta.last_row_id.expect("user id")
    }

    /// Set a usermeta value (JSON-encoded, as the auth crate stores it).
    /// The real schema has no unique index on (user_id, meta_key), so
    /// this deletes then inserts, like the auth crate does.
    pub async fn set_user_meta_raw(&self, user_id: i64, key: &str, json_value: &str) {
        self.prepare("DELETE FROM wp_usermeta WHERE user_id = ? AND meta_key = ?")
            .bind(vec![Param::Integer(user_id), Param::Text(key.to_string())])
            .run()
            .await
            .expect("delete user meta");
        self.prepare("INSERT INTO wp_usermeta (user_id, meta_key, meta_value) VALUES (?, ?, ?)")
            .bind(vec![
                Param::Integer(user_id),
                Param::Text(key.to_string()),
                Param::Text(json_value.to_string()),
            ])
            .run()
            .await
            .expect("set user meta");
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
