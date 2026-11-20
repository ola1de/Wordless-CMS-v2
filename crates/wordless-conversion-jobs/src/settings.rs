//! Settings migration (moved from `wordless-conversion`'s
//! `auto_convert.rs` in the 2026-11-20 split).
//!
//! Carry literal `get_option()` defaults over as Wordless options.
//! Only options that do not already exist are written; existing ones
//! are reported as skipped, never overwritten. Non-literal defaults
//! are never collected, so they never appear here.
//!
//! The defaults arrive as JSON from the converter Worker
//! (`{ "key", "defaultValue", "file" }` per entry — the
//! `OptionDefault` wire shape); this module only does the DB writes
//! and returns the migration report as JSON
//! (`{ "carriedOver": [...], "skipped": [...] }`, the
//! `SettingsMigrationReport` wire shape, entry `source` `"php-default"`).

use serde_json::{json, Value};
use wordless_db::{Db, DbError, Param};

/// Carry literal option defaults over as Wordless options. Returns the
/// migration report as JSON.
pub async fn migrate_option_defaults(db: &dyn Db, defaults: &[Value]) -> Result<Value, DbError> {
    let mut carried_over: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = Default::default();
    for def in defaults {
        let Some(key) = def.get("key").and_then(|k| k.as_str()) else {
            continue;
        };
        if !seen.insert(key.to_string()) {
            continue;
        }
        let default_value = def.get("defaultValue").cloned().unwrap_or(Value::Null);
        // `INSERT OR IGNORE` (same as v1 `addOption`): returns whether
        // a row was actually inserted. Uses the primitive `db.exec`
        // (not `exec_run`, which needs `prepare`) so this also works
        // against fakes that only implement the primitive methods.
        let json = serde_json::to_string(&default_value).unwrap_or_else(|_| "null".to_string());
        let meta = db
            .exec(
                "INSERT OR IGNORE INTO wp_options (option_name, option_value, autoload) VALUES (?, ?, 'yes')",
                &[
                    Param::Text(key.to_string()),
                    Param::Text(json),
                ],
            )
            .await?;
        if meta.changes > 0 {
            carried_over.push(json!({
                "key": key,
                "value": default_value,
                "source": "php-default",
            }));
        } else {
            skipped.push(json!({
                "key": key,
                "value": default_value,
                "source": "php-default",
                "reason": "Option already exists — kept the existing value.",
            }));
        }
    }
    Ok(json!({ "carriedOver": carried_over, "skipped": skipped }))
}
