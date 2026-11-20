//! Test-only SQLite `Db` implementation (rusqlite, in-memory).
//!
//! Applies migrations 0001 + 0002 + 0009 so service tests run the
//! real SQL against a real SQLite database — the same statements D1
//! executes. 0001 carries `wp_users`, `wp_usermeta`, `wl_sessions`
//! and `wl_audit_log`; 0009 adds `wl_login_attempts` (the lockout
//! store the guarded login reads). Never compiled into the Worker
//! (dev-dependency only, `#[cfg(test)]`).
//!
//! NOTE on paths: this file lives in `crates/wordless-auth/src/`, so
//! the workspace root is `../../../` — `../../../migrations/0001_init.sql`
//! is `wordless-rust/migrations/0001_init.sql`.

use std::cell::RefCell;

use async_trait::async_trait;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, Connection};
use serde_json::json;
use wordless_db::{Db, DbError, Param, Row, RunMeta};

pub(crate) struct TestDb {
    conn: RefCell<Connection>,
}

fn param_to_sql(p: &Param) -> SqlValue {
    match p {
        Param::Null => SqlValue::Null,
        Param::Integer(i) => SqlValue::Integer(*i),
        Param::Real(f) => SqlValue::Real(*f),
        Param::Text(s) => SqlValue::Text(s.clone()),
        Param::Blob(b) => SqlValue::Blob(b.clone()),
    }
}

fn sql_to_json(v: &SqlValue) -> serde_json::Value {
    match v {
        SqlValue::Null => serde_json::Value::Null,
        SqlValue::Integer(i) => json!(*i),
        SqlValue::Real(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        SqlValue::Text(s) => json!(s),
        // No blob columns exist in the schema; map defensively to null.
        SqlValue::Blob(_) => serde_json::Value::Null,
    }
}

impl TestDb {
    pub(crate) fn new() -> Self {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(include_str!("../../../migrations/0001_init.sql"))
            .expect("migration 0001");
        conn.execute_batch(include_str!("../../../migrations/0002_m2.sql"))
            .expect("migration 0002");
        conn.execute_batch(include_str!("../../../migrations/0009_m9.sql"))
            .expect("migration 0009");
        Self {
            conn: RefCell::new(conn),
        }
    }
}

#[async_trait(?Send)]
impl Db for TestDb {
    fn prepare(&self, sql: &str) -> wordless_db::Statement<'_> {
        wordless_db::Statement::new(self, sql.to_string())
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(sql).map_err(|e| DbError::D1(e.to_string()))?;
        let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = stmt
            .query(params_from_iter(params.iter().map(param_to_sql)))
            .map_err(|e| DbError::D1(e.to_string()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| DbError::D1(e.to_string()))? {
            let mut obj = serde_json::Map::new();
            for (i, name) in names.iter().enumerate() {
                let v: SqlValue = row.get(i).map_err(|e| DbError::D1(e.to_string()))?;
                obj.insert(name.clone(), sql_to_json(&v));
            }
            out.push(serde_json::Value::Object(obj));
        }
        Ok(out)
    }

    async fn query_one(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, DbError> {
        let rows = self.query_all(sql, params).await?;
        Ok(rows.into_iter().next())
    }

    async fn exec(&self, sql: &str, params: &[Param]) -> Result<RunMeta, DbError> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(sql).map_err(|e| DbError::D1(e.to_string()))?;
        let changes = stmt
            .execute(params_from_iter(params.iter().map(param_to_sql)))
            .map_err(|e| DbError::D1(e.to_string()))? as u64;
        Ok(RunMeta {
            last_row_id: Some(conn.last_insert_rowid()),
            changes,
        })
    }

    async fn batch(&self, stmts: &[(String, Vec<Param>)]) -> Result<(), DbError> {
        let mut conn = self.conn.borrow_mut();
        let tx = conn.transaction().map_err(|e| DbError::D1(e.to_string()))?;
        for (sql, params) in stmts {
            tx.execute(sql, params_from_iter(params.iter().map(param_to_sql)))
                .map_err(|e| DbError::D1(e.to_string()))?;
        }
        tx.commit().map_err(|e| DbError::D1(e.to_string()))?;
        Ok(())
    }
}
