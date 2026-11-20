//! WORDLESS v2 D1 adapter.
//!
//! The `Db` interface mirrors v1 `src/db.ts`: `prepare / bind / all /
//! first / run / batch`. Every service talks to D1 only through it, so
//! the port is "same SQL, same bound parameters, same row mapping".
//! Values are always bound parameters — never interpolated.
//!
//! `Row` is a `serde_json::Value` object: D1 returns plain JS objects,
//! and the `row::` helpers read them the way v1's `asText` / `asInt`
//! do (D1 numbers arrive as JSON numbers, i.e. f64).

pub mod d1;
pub mod options;
pub mod row;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

pub use d1::D1Db;

/// One D1 row (a JSON object).
pub type Row = Value;

/// A bound parameter.
#[derive(Debug, Clone)]
pub enum Param {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl From<i64> for Param {
    fn from(v: i64) -> Self {
        Param::Integer(v)
    }
}

impl From<i32> for Param {
    fn from(v: i32) -> Self {
        Param::Integer(v as i64)
    }
}

impl From<bool> for Param {
    fn from(v: bool) -> Self {
        Param::Integer(if v { 1 } else { 0 })
    }
}

impl From<String> for Param {
    fn from(v: String) -> Self {
        Param::Text(v)
    }
}

impl From<&str> for Param {
    fn from(v: &str) -> Self {
        Param::Text(v.to_string())
    }
}

impl From<Option<String>> for Param {
    fn from(v: Option<String>) -> Self {
        match v {
            Some(s) => Param::Text(s),
            None => Param::Null,
        }
    }
}

/// Metadata from a write (`run`).
#[derive(Debug, Clone, Default)]
pub struct RunMeta {
    pub last_row_id: Option<i64>,
    pub changes: u64,
}

#[derive(Debug, Error)]
pub enum DbError {
    #[error("D1 error: {0}")]
    D1(String),
    #[error("row deserialization failed: {0}")]
    Deserialize(String),
}

impl From<DbError> for wordless_kernel::WordlessError {
    fn from(e: DbError) -> Self {
        // Database failures are 500s; the message is safe to surface —
        // it comes from the platform, not from user input — but it must
        // never contain a bound value (D1 errors do not echo them).
        wordless_kernel::internal_error(&format!("Database error: {e}"))
    }
}

/// A prepared statement. Created by `Db::prepare`, parameters are
/// attached with `bind`, then exactly one of `all` / `first` / `run`
/// executes it.
pub struct Statement<'d> {
    db: &'d dyn Db,
    sql: String,
    params: Vec<Param>,
}

impl<'d> Statement<'d> {
    /// Public so test fakes in other crates can construct statements
    /// (their `Db::prepare` must return a `Statement`). The P1
    /// `pub(crate)` broke every external `FakeDb` — fixed in P2.
    pub fn new(db: &'d dyn Db, sql: String) -> Self {
        Self {
            db,
            sql,
            params: Vec::new(),
        }
    }

    pub fn bind(mut self, params: Vec<Param>) -> Self {
        self.params = params;
        self
    }

    pub async fn all<T: DeserializeOwned>(&self) -> Result<Vec<T>, DbError> {
        let rows = self.db.query_all(&self.sql, &self.params).await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value::<T>(row).map_err(|e| DbError::Deserialize(e.to_string()))
            })
            .collect()
    }

    pub async fn first<T: DeserializeOwned>(&self) -> Result<Option<T>, DbError> {
        let row = self.db.query_one(&self.sql, &self.params).await?;
        match row {
            Some(row) => Ok(Some(
                serde_json::from_value::<T>(row)
                    .map_err(|e| DbError::Deserialize(e.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    pub async fn run(&self) -> Result<RunMeta, DbError> {
        self.db.exec(&self.sql, &self.params).await
    }
}

/// The D1 interface. Object-safe so handlers can hold `Rc<dyn Db>`.
///
/// NOTE: this trait is `?Send` on purpose. The `worker` crate's D1
/// methods return `JsFuture`, which is `!Send` (it holds an
/// `Rc<RefCell<…>>`), so no future that awaits D1 can be `Send`.
/// That matches the Workers runtime: one isolate is single-threaded
/// and `wasm-bindgen-futures` runs handlers with `spawn_local`.
/// Nothing in the worker ever crosses a thread, so `Rc` (not `Arc`)
/// is the right pointer for it.
#[async_trait(?Send)]
pub trait Db {
    fn prepare(&self, sql: &str) -> Statement<'_>;

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError>;
    async fn query_one(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, DbError>;
    async fn exec(&self, sql: &str, params: &[Param]) -> Result<RunMeta, DbError>;
    /// Run statements atomically where the platform supports it (D1
    /// batch is one transaction). Callers must chunk large lists
    /// themselves — D1 limits a batch.
    async fn batch(&self, stmts: &[(String, Vec<Param>)]) -> Result<(), DbError>;
}

/// Convenience wrappers mirroring v1's `queryAll` / `queryOne` / `execRun`.
pub async fn query_all<T: DeserializeOwned>(
    db: &dyn Db,
    sql: &str,
    params: &[Param],
) -> Result<Vec<T>, DbError> {
    db.prepare(sql).bind(params.to_vec()).all::<T>().await
}

pub async fn query_one<T: DeserializeOwned>(
    db: &dyn Db,
    sql: &str,
    params: &[Param],
) -> Result<Option<T>, DbError> {
    db.prepare(sql).bind(params.to_vec()).first::<T>().await
}

pub async fn exec_run(db: &dyn Db, sql: &str, params: &[Param]) -> Result<RunMeta, DbError> {
    db.prepare(sql).bind(params.to_vec()).run().await
}
