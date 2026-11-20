//! Options API — port of v1 `src/modules/options/service.ts`.
//!
//! Mirrors WordPress `get_option` / `set_option` / `add_option` /
//! `delete_option`, except values are stored as JSON, NOT
//! PHP-serialized strings. The storage itself lives in
//! `wordless-db::options`; this module adds v1's key validation
//! (`requireString(key, "key", { max: 191 })`) and the typed
//! get/set/add/delete surface the routes use.

use serde::de::DeserializeOwned;
use serde::Serialize;
use wordless_db::Db;
use wordless_kernel::error::WordlessError;

use crate::common::require_str;

fn valid_key(key: &str) -> Result<String, WordlessError> {
    require_str(key, "key", 1, 191)
}

/// Read an option, JSON-decoding its value. Returns `default` when the
/// row is missing or the value is not valid JSON.
pub async fn get_option<T: DeserializeOwned>(
    db: &dyn Db,
    key: &str,
    default: T,
) -> Result<T, WordlessError> {
    let name = valid_key(key)?;
    Ok(wordless_db::options::get_option(db, &name, default).await?)
}

/// Write an option (insert or update), JSON-encoding the value.
pub async fn set_option<T: Serialize>(
    db: &dyn Db,
    key: &str,
    value: &T,
) -> Result<(), WordlessError> {
    let name = valid_key(key)?;
    Ok(wordless_db::options::set_option(db, &name, value).await?)
}

/// Insert only if the option does not exist yet. Returns `false` if it
/// already exists.
pub async fn add_option<T: Serialize>(
    db: &dyn Db,
    key: &str,
    value: &T,
) -> Result<bool, WordlessError> {
    let name = valid_key(key)?;
    let json = serde_json::to_string(value).map_err(|e| {
        WordlessError::new(
            "internal_error",
            500,
            &format!("Option serialisation failed: {e}"),
        )
    })?;
    let meta = db
        .prepare("INSERT OR IGNORE INTO wp_options (option_name, option_value, autoload) VALUES (?, ?, 'yes')")
        .bind(vec![
            wordless_db::Param::Text(name),
            wordless_db::Param::Text(json),
        ])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

/// Delete an option. Returns `true` when a row was removed.
pub async fn delete_option(db: &dyn Db, key: &str) -> Result<bool, WordlessError> {
    let name = valid_key(key)?;
    let meta = db
        .prepare("DELETE FROM wp_options WHERE option_name = ?")
        .bind(vec![wordless_db::Param::Text(name)])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

/// Raw (undecoded) option value, for the `GET /api/options/:key` route,
/// which returns `{ key, value }` with `value` the decoded JSON or
/// `null` when the option does not exist.
pub async fn get_option_value(db: &dyn Db, key: &str) -> Result<serde_json::Value, WordlessError> {
    let name = valid_key(key)?;
    Ok(
        wordless_db::options::get_option::<serde_json::Value>(db, &name, serde_json::Value::Null)
            .await?,
    )
}
