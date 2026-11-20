//! Test-only in-memory SQLite `Db` (dev-dependency `rusqlite`).
//!
//! Mirrors the D1 schema subset this crate queries (migration
//! `0008_m8.sql`: `wl_products`, `wl_orders`, `wl_order_items`,
//! `wl_carts`, `wl_webhook_log`; plus `wp_options` for payment
//! selection, whose shape comes from migration `0001`). Values
//! round-trip as JSON values; integers stay integers (D1 returns JSON
//! numbers — the `row::` helpers accept both).

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
CREATE TABLE wl_products (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  slug TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  price_minor INTEGER NOT NULL DEFAULT 0,
  currency TEXT NOT NULL DEFAULT 'NGN',
  stock INTEGER NOT NULL DEFAULT -1,
  status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published')),
  image_id INTEGER NOT NULL DEFAULT 0,
  type TEXT NOT NULL DEFAULT 'simple' CHECK (type IN ('simple', 'variable')),
  sku TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_orders (
  id TEXT PRIMARY KEY,
  customer_name TEXT NOT NULL DEFAULT '',
  customer_email TEXT NOT NULL DEFAULT '',
  total_minor INTEGER NOT NULL DEFAULT 0,
  currency TEXT NOT NULL DEFAULT 'NGN',
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'paid', 'fulfilled', 'cancelled')),
  payment_provider TEXT NOT NULL DEFAULT '',
  payment_reference TEXT NOT NULL DEFAULT '',
  stock_applied INTEGER NOT NULL DEFAULT 0,
  coupon_code TEXT,
  discount_minor INTEGER NOT NULL DEFAULT 0,
  tax_minor INTEGER NOT NULL DEFAULT 0,
  shipping_minor INTEGER NOT NULL DEFAULT 0,
  shipping_method TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL DEFAULT 'wordless',
  source_id TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_order_items (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  order_id TEXT NOT NULL,
  product_id INTEGER NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  price_minor INTEGER NOT NULL DEFAULT 0,
  quantity INTEGER NOT NULL DEFAULT 1,
  variation_id INTEGER,
  variation_label TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
-- P8 tables the cart/order code reads (variation stock, coupon usage).
-- Full Woo schema lives in migration 0014_p8.sql; this is the subset
-- commerce touches.
CREATE TABLE wl_woo_variations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  product_id INTEGER NOT NULL,
  sku TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'published' CHECK (status IN ('draft', 'published')),
  price_minor INTEGER NOT NULL DEFAULT 0,
  sale_price_minor INTEGER NOT NULL DEFAULT 0,
  stock INTEGER NOT NULL DEFAULT -1,
  attributes_json TEXT NOT NULL DEFAULT '{}',
  image_id INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_woo_coupons (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  code TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  type TEXT NOT NULL DEFAULT 'fixed' CHECK (type IN ('fixed', 'percent')),
  amount_minor INTEGER NOT NULL DEFAULT 0,
  percent_bp INTEGER NOT NULL DEFAULT 0,
  min_order_minor INTEGER NOT NULL DEFAULT 0,
  max_uses INTEGER NOT NULL DEFAULT 0,
  usage_count INTEGER NOT NULL DEFAULT 0,
  expires_at TEXT,
  active INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_carts (
  id TEXT PRIMARY KEY,
  items TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_webhook_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider TEXT NOT NULL DEFAULT '',
  event TEXT NOT NULL DEFAULT '',
  reference TEXT NOT NULL DEFAULT '',
  result TEXT NOT NULL DEFAULT '' CHECK (result IN ('accepted', 'rejected', 'duplicate', 'ignored', 'error')),
  detail TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE TABLE wp_options (option_id INTEGER PRIMARY KEY AUTOINCREMENT, option_name TEXT NOT NULL DEFAULT '', option_value TEXT NOT NULL DEFAULT '', autoload TEXT NOT NULL DEFAULT 'yes');
CREATE UNIQUE INDEX idx_wp_options_option_name ON wp_options (option_name);
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
