//! Test-only SQLite `Db` implementation (rusqlite, in-memory).
//!
//! Applies migrations 0001 + 0002 so service tests run the real SQL
//! against a real SQLite database — the same statements D1 executes.
//! Never compiled into the Worker (dev-dependency only, `#[cfg(test)]`).
//!
//! NOTE on paths: this file lives in `crates/wordless-cms/src/`, so the
//! workspace root is `../../../` — `../../../migrations/0001_init.sql`
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
        // P12: apply every migration in order, exactly as D1 does
        // in production — 0018 touches tables created as late as
        // 0016 (wl_seo_audit_runs) and 0007 (wl_404_log), so applying
        // only 0001+0002+0018 fails.
        for (name, sql) in [
            ("0001", include_str!("../../../migrations/0001_init.sql")),
            ("0002", include_str!("../../../migrations/0002_m2.sql")),
            ("0005", include_str!("../../../migrations/0005_m5.sql")),
            ("0006", include_str!("../../../migrations/0006_m6.sql")),
            ("0007", include_str!("../../../migrations/0007_m7.sql")),
            ("0008", include_str!("../../../migrations/0008_m8.sql")),
            ("0009", include_str!("../../../migrations/0009_m9.sql")),
            ("0010", include_str!("../../../migrations/0010_m11.sql")),
            ("0011", include_str!("../../../migrations/0011_p3.sql")),
            ("0012", include_str!("../../../migrations/0012_p6.sql")),
            ("0013", include_str!("../../../migrations/0013_p7.sql")),
            ("0014", include_str!("../../../migrations/0014_p8.sql")),
            ("0015", include_str!("../../../migrations/0015_p9.sql")),
            ("0016", include_str!("../../../migrations/0016_p10.sql")),
            ("0017", include_str!("../../../migrations/0017_p11.sql")),
            ("0018", include_str!("../../../migrations/0018_p12.sql")),
        ] {
            conn.execute_batch(sql)
                .unwrap_or_else(|e| panic!("migration {name}: {e}"));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_tables_exist() {
        let db = TestDb::new();
        let count: i64 = futures_executor::block_on(async {
            let row: Option<Row> = db
                .prepare("SELECT COUNT(*) AS total FROM sqlite_master WHERE type = 'table' AND name LIKE 'wp_%'")
                .bind(vec![])
                .first()
                .await
                .unwrap();
            wordless_db::row::row_int(&row.unwrap(), "total")
        });
        // 0001 creates 12 wp_* tables.
        assert_eq!(count, 12);
    }

    #[test]
    fn last_row_id_and_changes() {
        let db = TestDb::new();
        futures_executor::block_on(async {
            let meta = db
                .prepare("INSERT INTO wp_options (option_name, option_value, autoload) VALUES (?, ?, 'yes')")
                .bind(vec![Param::Text("t".into()), Param::Text("1".into())])
                .run()
                .await
                .unwrap();
            assert_eq!(meta.last_row_id, Some(1));
            assert_eq!(meta.changes, 1);
            let meta = db
                .prepare("UPDATE wp_options SET option_value = ? WHERE option_name = ?")
                .bind(vec![Param::Text("2".into()), Param::Text("t".into())])
                .run()
                .await
                .unwrap();
            assert_eq!(meta.changes, 1);
        });
    }
}
