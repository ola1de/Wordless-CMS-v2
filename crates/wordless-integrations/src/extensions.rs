//! Extensions service — `wl_extensions` rows (port of v1
//! `src/modules/extensions/service.ts`).
//!
//! Installing an extension records it in `wl_extensions`;
//! activating one sets its status to `"active"`. Activation only
//! records the status: **installed code is never executed** —
//! running an extension needs the P6 sandboxed runtime, and the
//! compatibility report (`report_json`, see [`crate::compat`]) is
//! the truth about what may run. The M11 install path passes the
//! analyser's report JSON through [`InstallExtensionInput::report`];
//! a `None` report means "never analysed" (extensions installed
//! before M11, or installs with no PHP files).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wordless_db::{
    exec_run, query_all, query_one,
    row::{row_opt_text, row_text},
    Db, Param, Row,
};
use wordless_kernel::{
    error::{bad_request, not_found},
    time::now_iso,
    validate::require_string,
    WordlessError,
};

use crate::compat::{parse_compatibility_report, summarize_report};

/// One installed extension (v1 `ExtensionRecord`, wire shape:
/// camelCase).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionRecord {
    pub slug: String,
    pub name: String,
    pub kind: String,
    pub version: String,
    pub status: String,
    pub source: Option<String>,
    /// M11 compatibility report, or `None` when the extension was
    /// never analysed.
    pub report: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

/// Extension active/inactive status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionStatus {
    Active,
    Inactive,
}

impl ExtensionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExtensionStatus::Active => "active",
            ExtensionStatus::Inactive => "inactive",
        }
    }
}

/// Input to [`install_extension`] (v1 `InstallExtensionInput`).
#[derive(Debug, Clone, Default)]
pub struct InstallExtensionInput {
    pub slug: String,
    pub name: String,
    pub version: Option<String>,
    pub kind: Option<String>,
    pub source: Option<String>,
    pub status: Option<ExtensionStatus>,
    /// M11: report JSON from the PHP analyser (stored as JSON).
    /// `None` stores NULL ("not analysed"). A reinstall with
    /// `None` keeps the existing report (v1 `COALESCE` rule).
    pub report: Option<Value>,
}

fn row_to_record(row: &Row) -> ExtensionRecord {
    let report_json = row_opt_text(row, "report_json");
    ExtensionRecord {
        slug: row_text(row, "slug"),
        name: row_text(row, "name"),
        kind: row_text(row, "kind"),
        version: row_text(row, "version"),
        status: row_text(row, "status"),
        source: row_opt_text(row, "source"),
        report: report_json
            .as_deref()
            .and_then(|raw| parse_compatibility_report(Some(raw))),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// True when `slug` matches v1's
/// `^[a-z0-9]+(?:-[a-z0-9]+)*$` (no regex crate).
pub fn is_valid_slug(slug: &str) -> bool {
    if slug.is_empty() {
        return false;
    }
    slug.split('-').all(|seg| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// Record (or re-record) an extension in `wl_extensions`.
/// Reinstalling updates name/kind/version/status/source and the
/// report when one is supplied; a `None` report keeps the existing
/// one. The row's `id` is its slug, as in v1.
///
/// # Errors
/// * 400 on an invalid slug or name.
/// * 500 on a database failure.
pub async fn install_extension(
    db: &dyn Db,
    input: &InstallExtensionInput,
) -> Result<ExtensionRecord, WordlessError> {
    let slug = require_string(&Value::String(input.slug.clone()), "slug", 1, 100)?.to_lowercase();
    if !is_valid_slug(&slug) {
        return Err(bad_request(
            "Extension slug must be lowercase letters, digits and hyphens only.",
        ));
    }
    let name = require_string(&Value::String(input.name.clone()), "name", 1, 200)?;
    let version = input.version.clone().unwrap_or_else(|| "0.1.0".to_string());
    let kind = input.kind.clone().unwrap_or_else(|| "plugin".to_string());
    let status = input.status.unwrap_or(ExtensionStatus::Inactive);
    let now = now_iso();
    let report_json = input.report.as_ref().map(|r| r.to_string());
    exec_run(
        db,
        "INSERT INTO wl_extensions (id, slug, name, kind, version, status, source, report_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(slug) DO UPDATE SET \
           name = excluded.name, kind = excluded.kind, version = excluded.version, \
           status = excluded.status, source = excluded.source, \
           report_json = COALESCE(excluded.report_json, wl_extensions.report_json), \
           updated_at = excluded.updated_at",
        &[
            Param::Text(slug.clone()),
            Param::Text(slug.clone()),
            Param::Text(name),
            Param::Text(kind),
            Param::Text(version),
            Param::Text(status.as_str().to_string()),
            Param::from(input.source.clone()),
            Param::from(report_json),
            Param::Text(now.clone()),
            Param::Text(now),
        ],
    )
    .await
    .map_err(WordlessError::from)?;
    get_extension(db, &slug).await
}

/// Fetch one installed extension by slug.
///
/// # Errors
/// * 404 when the slug is not installed.
/// * 500 on a database failure.
pub async fn get_extension(db: &dyn Db, slug: &str) -> Result<ExtensionRecord, WordlessError> {
    let row: Option<Row> = query_one(
        db,
        "SELECT * FROM wl_extensions WHERE slug = ? LIMIT 1",
        &[Param::Text(slug.to_string())],
    )
    .await
    .map_err(WordlessError::from)?;
    match row {
        Some(row) => Ok(row_to_record(&row)),
        None => Err(not_found(&format!(
            "Extension \"{slug}\" is not installed."
        ))),
    }
}

/// All installed extensions, ordered by slug.
///
/// # Errors
/// Returns `Err` only on a database failure.
pub async fn list_extensions(db: &dyn Db) -> Result<Vec<ExtensionRecord>, WordlessError> {
    let rows: Vec<Row> = query_all(db, "SELECT * FROM wl_extensions ORDER BY slug ASC", &[])
        .await
        .map_err(WordlessError::from)?;
    Ok(rows.iter().map(row_to_record).collect())
}

/// One row of the Plugins page list (P4, §0.1 A): the installed
/// extension with its active state and a compatibility-verdict
/// *summary* — counts + overall verdict, not the full per-file
/// report. This is a read-only view over `wl_extensions` (+ the
/// stored compat report); nothing new is stored. The full report
/// stays on `GET /api/marketplace/:slug/report`, and activation
/// stays on the marketplace activate/deactivate routes (also aliased
/// under `/api/plugins/:slug/...`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    pub slug: String,
    pub name: String,
    pub kind: String,
    pub version: String,
    pub status: String,
    pub active: bool,
    /// Overall verdict: `compatible` | `partial` | `blocked` |
    /// `not-analysed` (from `summarize_report`; `not-analysed`
    /// when the extension was never analysed).
    pub verdict: String,
    pub converted: i64,
    pub partial: i64,
    pub blocked: i64,
    pub php_files: i64,
}

/// The installed-plugins list for `GET /api/plugins`.
pub async fn list_plugin_summaries(db: &dyn Db) -> Result<Vec<PluginSummary>, WordlessError> {
    let records = list_extensions(db).await?;
    Ok(records
        .iter()
        .map(|rec| {
            let summary = rec.report.as_ref().map(summarize_report);
            PluginSummary {
                slug: rec.slug.clone(),
                name: rec.name.clone(),
                kind: rec.kind.clone(),
                version: rec.version.clone(),
                status: rec.status.clone(),
                active: rec.status == "active",
                verdict: summary
                    .as_ref()
                    .map(|s| s.verdict.clone())
                    .unwrap_or_else(|| "not-analysed".to_string()),
                converted: summary.as_ref().map(|s| s.converted).unwrap_or(0),
                partial: summary.as_ref().map(|s| s.partial).unwrap_or(0),
                blocked: summary.as_ref().map(|s| s.blocked).unwrap_or(0),
                php_files: summary.as_ref().map(|s| s.php_files).unwrap_or(0),
            }
        })
        .collect())
}

/// Set an extension's status. Activation only records the status —
/// it does not load or run any code.
///
/// # Errors
/// * 404 when the slug is not installed.
/// * 500 on a database failure.
pub async fn set_extension_status(
    db: &dyn Db,
    slug: &str,
    status: ExtensionStatus,
) -> Result<ExtensionRecord, WordlessError> {
    let existing = get_extension(db, slug).await?;
    exec_run(
        db,
        "UPDATE wl_extensions SET status = ?, updated_at = ? WHERE slug = ?",
        &[
            Param::Text(status.as_str().to_string()),
            Param::Text(now_iso()),
            Param::Text(slug.to_string()),
        ],
    )
    .await
    .map_err(WordlessError::from)?;
    Ok(ExtensionRecord {
        status: status.as_str().to_string(),
        ..existing
    })
}

// ------------------------------------------------------------------ plugin settings (P4)
//
// Backing store: one `wp_options` row per plugin,
// `wl_plugin_settings_<slug>`, holding a JSON object. Chosen over a
// `wl_extensions` column because it needs no migration and no schema
// change — the options store is already JSON and namespaced per
// plugin. It is deliberately NOT the raw option keys the analyser
// migrates at install time: those are the plugin's runtime options
// (read by its converted code via `get_option`), while this store is
// the Plugins page's editable settings. The carried-over defaults
// (the compat report's top-level `optionDefaults`, from literal
// `get_option('key', default)` calls in the PHP) are the base layer;
// stored values override them on read. A stored key set to `null`
// via PUT deletes the override, so the default shows again.

/// The `wp_options` key holding one plugin's stored settings.
pub fn plugin_settings_key(slug: &str) -> String {
    format!("wl_plugin_settings_{slug}")
}

/// Validation caps for a settings PUT (abuse guards, not a schema:
// plugins declare their own keys, so the API validates shape only).
const MAX_PLUGIN_SETTING_KEYS: usize = 100;
const MAX_PLUGIN_SETTINGS_BYTES: usize = 64 * 1024;

/// The carried-over defaults for a plugin, from its compat report's
/// top-level `optionDefaults` (`{key, defaultValue, file}` per
/// entry). First occurrence per key wins (same dedup as the install
/// path's `migrate_option_defaults`). `None` / a missing array means
/// no defaults — an unanalysed plugin has an empty base layer.
pub fn plugin_defaults_from_report(report: Option<&Value>) -> serde_json::Map<String, Value> {
    let mut defaults = serde_json::Map::new();
    let Some(report) = report else {
        return defaults;
    };
    let Some(entries) = report.get("optionDefaults").and_then(Value::as_array) else {
        return defaults;
    };
    for entry in entries {
        let Some(key) = entry.get("key").and_then(Value::as_str) else {
            continue;
        };
        if key.is_empty() || defaults.contains_key(key) {
            continue;
        }
        let Some(value) = entry.get("defaultValue") else {
            continue;
        };
        defaults.insert(key.to_string(), value.clone());
    }
    defaults
}

/// The plugin's effective settings: stored overrides merged over the
/// carried-over defaults. A stored value that is not a JSON object
/// (e.g. written by hand via `/api/options`) is ignored, so a bad
/// row reads as "no overrides", never a 500.
///
/// # Errors
/// * 404 when the slug is not installed.
/// * 500 on a database failure.
pub async fn get_plugin_settings(db: &dyn Db, slug: &str) -> Result<Value, WordlessError> {
    let record = get_extension(db, slug).await?;
    let mut merged = plugin_defaults_from_report(record.report.as_ref());
    let stored: Value =
        wordless_db::options::get_option(db, &plugin_settings_key(slug), Value::Null)
            .await
            .map_err(WordlessError::from)?;
    if let Some(obj) = stored.as_object() {
        for (key, value) in obj {
            merged.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(merged))
}

/// Merge a settings patch into the stored settings and return the
/// effective settings (as `get_plugin_settings`). Keys present with
/// a `null` value delete that stored override; every other value
/// overwrites it. Keys absent from the patch are left alone.
///
/// # Errors
/// * 400 when the patch is not a JSON object, has more than 100
///   keys, is larger than 64 KiB serialised, or has an empty, >
///   200-char, or control-character key.
/// * 404 when the slug is not installed.
/// * 500 on a database failure.
pub async fn put_plugin_settings(
    db: &dyn Db,
    slug: &str,
    patch: &Value,
) -> Result<Value, WordlessError> {
    // 404 first: settings for a plugin that is not installed are meaningless.
    get_extension(db, slug).await?;
    let obj = patch
        .as_object()
        .ok_or_else(|| bad_request("Plugin settings must be a JSON object."))?;
    if obj.len() > MAX_PLUGIN_SETTING_KEYS {
        return Err(bad_request(
            "Plugin settings may have at most 100 keys per request.",
        ));
    }
    if patch.to_string().len() > MAX_PLUGIN_SETTINGS_BYTES {
        return Err(bad_request(
            "Plugin settings must be at most 64 KiB as JSON.",
        ));
    }
    for key in obj.keys() {
        if key.is_empty() || key.len() > 200 || key.chars().any(|c| c.is_control()) {
            return Err(bad_request(
                "Plugin setting keys must be non-empty, at most 200 characters, with no control characters.",
            ));
        }
    }
    let key = plugin_settings_key(slug);
    let stored: Value = wordless_db::options::get_option(db, &key, Value::Null)
        .await
        .map_err(WordlessError::from)?;
    let mut map = stored.as_object().cloned().unwrap_or_default();
    for (k, v) in obj {
        if v.is_null() {
            map.remove(k);
        } else {
            map.insert(k.clone(), v.clone());
        }
    }
    wordless_db::options::set_option(db, &key, &Value::Object(map))
        .await
        .map_err(WordlessError::from)?;
    get_plugin_settings(db, slug).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block_on, SqliteDb};
    use serde_json::json;

    fn input(slug: &str) -> InstallExtensionInput {
        InstallExtensionInput {
            slug: slug.to_string(),
            name: "Test Plugin".to_string(),
            version: Some("1.0.0".to_string()),
            kind: Some("plugin".to_string()),
            source: Some("marketplace:upload".to_string()),
            status: Some(ExtensionStatus::Inactive),
            report: None,
        }
    }

    #[test]
    fn install_get_list_and_status_round_trip() {
        let db = SqliteDb::new();
        let rec = block_on(install_extension(&db, &input("my-plugin"))).unwrap();
        assert_eq!(rec.slug, "my-plugin");
        assert_eq!(rec.status, "inactive");
        assert_eq!(rec.report, None);
        let fetched = block_on(get_extension(&db, "my-plugin")).unwrap();
        assert_eq!(fetched.name, "Test Plugin");
        let listed = block_on(list_extensions(&db)).unwrap();
        assert_eq!(listed.len(), 1);
        let active = block_on(set_extension_status(
            &db,
            "my-plugin",
            ExtensionStatus::Active,
        ))
        .unwrap();
        assert_eq!(active.status, "active");
        // Reinstall keeps the status it is given and updates the version.
        let mut again = input("my-plugin");
        again.version = Some("1.1.0".to_string());
        let reinstalled = block_on(install_extension(&db, &again)).unwrap();
        assert_eq!(reinstalled.version, "1.1.0");
        assert_eq!(reinstalled.status, "inactive");
    }

    #[test]
    fn plugin_summaries_carry_verdict_without_the_full_report() {
        let db = SqliteDb::new();
        block_on(install_extension(&db, &input("plain-plugin"))).unwrap();
        let mut analysed = input("analysed-plugin");
        analysed.report = Some(json!({
            "verdict": "compatible", "phpFiles": 1,
            "converted": 1, "partial": 0, "blocked": 0
        }));
        block_on(install_extension(&db, &analysed)).unwrap();
        let summaries = block_on(list_plugin_summaries(&db)).unwrap();
        assert_eq!(summaries.len(), 2);
        let plain = summaries.iter().find(|s| s.slug == "plain-plugin").unwrap();
        assert!(!plain.active);
        assert_eq!(plain.verdict, "not-analysed");
        assert_eq!(plain.php_files, 0);
        let done = summaries
            .iter()
            .find(|s| s.slug == "analysed-plugin")
            .unwrap();
        assert_eq!(done.verdict, "compatible");
        assert_eq!(done.converted, 1);
        assert_eq!(done.php_files, 1);
        // Activating flips the summary's active flag, nothing else.
        block_on(set_extension_status(
            &db,
            "analysed-plugin",
            ExtensionStatus::Active,
        ))
        .unwrap();
        let again = block_on(list_plugin_summaries(&db)).unwrap();
        let done = again.iter().find(|s| s.slug == "analysed-plugin").unwrap();
        assert!(done.active);
        assert_eq!(done.status, "active");
        assert_eq!(done.verdict, "compatible");
    }

    #[test]
    fn rejects_bad_slugs_and_unknown_slugs() {
        let db = SqliteDb::new();
        let mut bad = input("Bad_Slug!");
        let err = block_on(install_extension(&db, &bad)).unwrap_err();
        assert_eq!(err.status, 400);
        bad.slug = "UPPER".to_string();
        // Uppercase is lowercased by install, so this succeeds as "upper".
        assert_eq!(
            block_on(install_extension(&db, &bad)).unwrap().slug,
            "upper"
        );
        let err = block_on(get_extension(&db, "nope")).unwrap_err();
        assert_eq!(err.status, 404);
        assert!(err.message.contains("not installed"), "{}", err.message);
        let err = block_on(set_extension_status(&db, "nope", ExtensionStatus::Active)).unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[test]
    fn report_is_preserved_when_reinstalling_without_one() {
        let db = SqliteDb::new();
        let mut with_report = input("with-report");
        with_report.report = Some(json!({
            "phpFiles": 1, "totalFiles": 1, "convertible": 1, "partial": 0,
            "blocked": 0, "unparsed": 0, "skipped": 0, "verdict": "compatible",
            "topReasons": [], "files": [], "filesTruncated": false,
            "optionDefaults": [], "conversion": null, "analysedAt": "2026-11-19T00:00:00Z"
        }));
        let rec = block_on(install_extension(&db, &with_report)).unwrap();
        assert_eq!(rec.report.as_ref().unwrap()["verdict"], json!("compatible"));
        let plain = input("with-report");
        let rec = block_on(install_extension(&db, &plain)).unwrap();
        assert_eq!(
            rec.report.as_ref().unwrap()["verdict"],
            json!("compatible"),
            "reinstall without a report must keep the stored one"
        );
    }

    #[test]
    fn plugin_settings_merge_defaults_stored_and_delete() {
        let db = SqliteDb::new();
        let mut with_report = input("demo-plugin");
        with_report.report = Some(json!({
            "phpFiles": 1, "totalFiles": 1, "convertible": 1, "partial": 0,
            "blocked": 0, "unparsed": 0, "skipped": 0, "verdict": "compatible",
            "topReasons": [], "files": [], "filesTruncated": false,
            "optionDefaults": [
                {"key": "demo_enabled", "defaultValue": true, "file": "demo.php"},
                {"key": "demo_label", "defaultValue": "Demo", "file": "demo.php"},
                {"key": "demo_limit", "defaultValue": 10, "file": "demo.php"},
                // Duplicate key: first occurrence wins.
                {"key": "demo_enabled", "defaultValue": false, "file": "other.php"}
            ],
            "conversion": null, "analysedAt": "2026-11-19T00:00:00Z"
        }));
        block_on(install_extension(&db, &with_report)).unwrap();
        // GET returns the carried-over defaults, first occurrence wins.
        let settings = block_on(get_plugin_settings(&db, "demo-plugin")).unwrap();
        assert_eq!(settings["demo_enabled"], json!(true));
        assert_eq!(settings["demo_label"], json!("Demo"));
        assert_eq!(settings["demo_limit"], json!(10));
        // PUT merges: overrides a default, adds a new key, keeps the rest.
        let updated = block_on(put_plugin_settings(
            &db,
            "demo-plugin",
            &json!({"demo_label": "Changed", "demo_extra": 3}),
        ))
        .unwrap();
        assert_eq!(updated["demo_label"], json!("Changed"));
        assert_eq!(updated["demo_extra"], json!(3));
        assert_eq!(updated["demo_enabled"], json!(true));
        assert_eq!(updated["demo_limit"], json!(10));
        // A null PUT deletes the stored override — the default shows again.
        let updated = block_on(put_plugin_settings(
            &db,
            "demo-plugin",
            &json!({"demo_label": null}),
        ))
        .unwrap();
        assert_eq!(updated["demo_label"], json!("Demo"));
        assert_eq!(updated["demo_extra"], json!(3));
        // Null on a non-default key removes it entirely.
        let updated = block_on(put_plugin_settings(
            &db,
            "demo-plugin",
            &json!({"demo_extra": null}),
        ))
        .unwrap();
        assert!(!updated.as_object().unwrap().contains_key("demo_extra"));
        // PUT persists across a fresh GET.
        let again = block_on(get_plugin_settings(&db, "demo-plugin")).unwrap();
        assert_eq!(again, updated);
    }

    #[test]
    fn plugin_settings_validation_and_unknown_slugs() {
        let db = SqliteDb::new();
        block_on(install_extension(&db, &input("plain-plugin"))).unwrap();
        // No report means no defaults: an empty object until stored.
        assert_eq!(
            block_on(get_plugin_settings(&db, "plain-plugin")).unwrap(),
            json!({})
        );
        // A non-object patch is a 400.
        let err = block_on(put_plugin_settings(&db, "plain-plugin", &json!([1, 2]))).unwrap_err();
        assert_eq!(err.status, 400);
        // An empty key is a 400.
        let err = block_on(put_plugin_settings(&db, "plain-plugin", &json!({"": 1}))).unwrap_err();
        assert_eq!(err.status, 400);
        // Unknown slugs are 404 on both verbs.
        let err = block_on(get_plugin_settings(&db, "nope")).unwrap_err();
        assert_eq!(err.status, 404);
        let err = block_on(put_plugin_settings(&db, "nope", &json!({}))).unwrap_err();
        assert_eq!(err.status, 404);
        // A stored value that is not valid JSON reads as no overrides.
        // (Seed the row via set_option, then corrupt it with raw SQL.)
        block_on(async {
            wordless_db::options::set_option(
                &db,
                &plugin_settings_key("plain-plugin"),
                &"not-an-object",
            )
            .await
            .unwrap();
            db.prepare("UPDATE wp_options SET option_value = ? WHERE option_name = ?")
                .bind(vec![
                    wordless_db::Param::Text("{invalid json".to_string()),
                    wordless_db::Param::Text(plugin_settings_key("plain-plugin")),
                ])
                .run()
                .await
                .unwrap();
        });
        assert_eq!(
            block_on(get_plugin_settings(&db, "plain-plugin")).unwrap(),
            json!({}),
            "corrupt stored settings must read as empty, not 500"
        );
    }
}
