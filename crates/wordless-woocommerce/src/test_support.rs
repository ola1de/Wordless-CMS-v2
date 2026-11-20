//! Test-only SQLite `Db` implementation (rusqlite, in-memory).
//! Same pattern as `wordless-social::test_support`: applies the real
//! migrations (0001 for wp_options, 0008 for the P3 commerce tables,
//! 0014 for the P8 Woo tables) so service tests run the real SQL
//! against a real SQLite database — the same statements D1 executes.
//! Never compiled into the Worker (dev-dependency only, `#[cfg(test)]`).

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
        SqlValue::Blob(_) => serde_json::Value::Null,
    }
}

impl TestDb {
    pub(crate) fn new() -> Self {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(include_str!("../../../migrations/0001_init.sql"))
            .expect("migration 0001");
        conn.execute_batch(include_str!("../../../migrations/0008_m8.sql"))
            .expect("migration 0008");
        conn.execute_batch(include_str!("../../../migrations/0014_p8.sql"))
            .expect("migration 0014");
        Self {
            conn: RefCell::new(conn),
        }
    }

    /// Run raw SQL (test setup only).
    pub(crate) fn exec_batch_sql(&self, sql: &str) {
        self.conn.borrow().execute_batch(sql).expect("test sql");
    }
}

#[async_trait(?Send)]
impl Db for TestDb {
    fn prepare(&self, sql: &str) -> wordless_db::Statement<'_> {
        wordless_db::Statement::new(self as &dyn Db, sql.to_string())
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(sql).map_err(|e| DbError::D1(e.to_string()))?;
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let rows = stmt
            .query_map(params_from_iter(params.iter().map(param_to_sql)), |row| {
                let mut obj = serde_json::Map::new();
                for (idx, col) in cols.iter().enumerate() {
                    let v: SqlValue = row.get(idx)?;
                    obj.insert(col.clone(), sql_to_json(&v));
                }
                Ok(serde_json::Value::Object(obj))
            })
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
        let conn = self.conn.borrow();
        let changes = conn
            .execute(sql, params_from_iter(params.iter().map(param_to_sql)))
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
