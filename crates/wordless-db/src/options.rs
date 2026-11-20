//! Options API — mirrors WordPress `get_option` / `set_option`,
//! except values are stored as JSON, NOT PHP-serialized strings.
//! (Port of v1 `src/modules/options/service.ts`. Lives here because
//! `wordless-auth` needs `get_option` for the session idle timeout
//! and the options table is plain D1 access; it moves to
//! `wordless-cms` in P2 along with the rest of the options module.)

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::row::row_text;
use crate::{Db, DbError, Param, Row};

/// Read an option, JSON-decoding its value. Returns `default` when
/// the row is missing or the value is not valid JSON.
pub async fn get_option<T: DeserializeOwned>(
    db: &dyn Db,
    key: &str,
    default: T,
) -> Result<T, DbError> {
    let row: Option<Row> = db
        .prepare("SELECT option_value FROM wp_options WHERE option_name = ? LIMIT 1")
        .bind(vec![Param::Text(key.to_string())])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Ok(default);
    };
    let raw = row_text(&row, "option_value");
    match serde_json::from_str::<T>(&raw) {
        Ok(v) => Ok(v),
        Err(_) => Ok(default),
    }
}

/// Write an option (insert or update), JSON-encoding the value.
pub async fn set_option<T: Serialize>(db: &dyn Db, key: &str, value: &T) -> Result<(), DbError> {
    let json = serde_json::to_string(value).map_err(|e| DbError::Deserialize(e.to_string()))?;
    db.prepare(
        "INSERT INTO wp_options (option_name, option_value, autoload) VALUES (?, ?, 'yes')
         ON CONFLICT(option_name) DO UPDATE SET option_value = excluded.option_value",
    )
    .bind(vec![Param::Text(key.to_string()), Param::Text(json)])
    .run()
    .await?;
    Ok(())
}
