//! The D1 implementation of the `Db` trait, over `worker::d1`.
//!
//! Parameter binding converts `Param` to `JsValue` (integers as JS
//! numbers, blobs as `Uint8Array`). Rows come back as `JsValue` and
//! are converted with `serde-wasm-bindgen` into `serde_json::Value`
//! (numbers become JSON numbers, i.e. f64 — see `row::` helpers).
//!
//! `D1Db::stub()` is a `Db` that fails every query: unit tests that
//! need a `dyn Db` without a Worker use it (routes that never touch
//! the database, e.g. `/api/health`).

use async_trait::async_trait;
use serde_json::Value;
use wasm_bindgen::JsValue;
use worker::d1::{D1Database, D1PreparedStatement};

use crate::{Db, DbError, Param, Row, RunMeta, Statement};

fn param_to_js(param: &Param) -> JsValue {
    match param {
        Param::Null => JsValue::NULL,
        Param::Integer(n) => JsValue::from_f64(*n as f64),
        Param::Real(f) => JsValue::from_f64(*f),
        Param::Text(s) => JsValue::from_str(s),
        Param::Blob(bytes) => js_sys::Uint8Array::from(bytes.as_slice()).into(),
    }
}

fn bind_all(stmt: D1PreparedStatement, params: &[Param]) -> Result<D1PreparedStatement, DbError> {
    let js: Vec<JsValue> = params.iter().map(param_to_js).collect();
    stmt.bind(&js).map_err(|e| DbError::D1(e.to_string()))
}

/// D1-backed `Db`. Held behind `Rc` by the worker entry (one handle
/// per request); `D1Database` itself is not `Clone`.
pub struct D1Db {
    db: Option<D1Database>,
}

impl D1Db {
    pub fn new(db: D1Database) -> Self {
        Self { db: Some(db) }
    }

    /// A `Db` that fails every query. For unit tests only.
    pub fn stub() -> Self {
        Self { db: None }
    }

    fn require_db(&self) -> Result<&D1Database, DbError> {
        self.db.as_ref().ok_or_else(|| {
            DbError::D1("D1 database is not available (stub Db used outside a Worker)".to_string())
        })
    }
}

#[async_trait(?Send)]
impl Db for D1Db {
    fn prepare(&self, sql: &str) -> Statement<'_> {
        Statement::new(self as &dyn Db, sql.to_string())
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError> {
        let db = self.require_db()?;
        let stmt = bind_all(db.prepare(sql), params)?;
        let result = stmt.all().await.map_err(|e| DbError::D1(e.to_string()))?;
        let rows: Vec<Value> = result.results().map_err(|e| DbError::D1(e.to_string()))?;
        Ok(rows)
    }

    async fn query_one(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, DbError> {
        let db = self.require_db()?;
        let stmt = bind_all(db.prepare(sql), params)?;
        let row: Option<Value> = stmt
            .first(None)
            .await
            .map_err(|e| DbError::D1(e.to_string()))?;
        Ok(row)
    }

    async fn exec(&self, sql: &str, params: &[Param]) -> Result<RunMeta, DbError> {
        let db = self.require_db()?;
        let stmt = bind_all(db.prepare(sql), params)?;
        let result = stmt.run().await.map_err(|e| DbError::D1(e.to_string()))?;
        let meta = result.meta().map_err(|e| DbError::D1(e.to_string()))?;
        Ok(RunMeta {
            last_row_id: meta.as_ref().and_then(|m| m.last_row_id),
            changes: meta
                .as_ref()
                .and_then(|m| m.changes)
                .map(|c| c as u64)
                .unwrap_or(0),
        })
    }

    async fn batch(&self, stmts: &[(String, Vec<Param>)]) -> Result<(), DbError> {
        let db = self.require_db()?;
        let mut prepared = Vec::with_capacity(stmts.len());
        for (sql, params) in stmts {
            prepared.push(bind_all(db.prepare(sql), params)?);
        }
        db.batch(prepared)
            .await
            .map_err(|e| DbError::D1(e.to_string()))?;
        Ok(())
    }
}
