//! Compatibility reports (port of v1 `compat-report.ts`, M11).
//!
//! When a plugin ZIP is installed via the marketplace, the M4 PHP
//! analyser runs over its PHP files and the result is stored as a
//! per-plugin compatibility report on the `wl_extensions` row
//! (`report_json`, migration 0010). The report is a static map, not
//! a promise: a "compatible" verdict means every analysed file is
//! procedural code the converter can translate; "partial" means some
//! files need a class runtime or manual work; "blocked" means some
//! files call an external API, use `$wpdb`, or contain a construct
//! the sandbox blocks. **Installed code is NEVER executed** — this
//! analysis only parses source text.
//!
//! # Who analyses?
//!
//! Not this crate. The PHP analyser lives in the parallel
//! `wordless-conversion` crate (P3); the worker wiring calls it and
//! passes its report JSON to [`store_compat_report`]. This crate
//! only stores, reads, parses, and summarises report JSON — so the
//! report shape it accepts is the analyser's shape:
//! `files[]` entries with `path`/`verdict`/
//! `functions_converted`/`functions_total`/`hooks_converted`/
//! `hooks_total`/`blocking_reasons`/`warnings`, plus top-level
//! `converted`/`partial`/`blocked` counts. The older v1 stored
//! shape (`phpFiles`, `convertible`, `verdict`, `topReasons`, …)
//! is accepted too — [`summarize_report`] reads both.
//!
//! Honest limits (from v1): the analyser parses PHP; it does not
//! run it, so runtime behaviour is NOT verified. A plugin whose
//! files are all "convertible" is still inert after install:
//! activation only records the status; running the plugin needs
//! the conversion pipeline to produce a Wordless extension first.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wordless_db::{exec_run, query_one, Db, Param, Row};
use wordless_kernel::{
    error::{bad_request, not_found},
    time::now_iso,
    WordlessError,
};

/// Worst case per install: 2000 PHP files analysed, the rest
/// skipped (v1 `MAX_ANALYSED_PHP_FILES`).
pub const MAX_ANALYSED_PHP_FILES: usize = 2000;
/// A single PHP file over this size is skipped by the analyser
/// (v1 `MAX_ANALYSED_PHP_BYTES`).
pub const MAX_ANALYSED_PHP_BYTES: usize = 1024 * 1024;
/// Per-file verdicts stored in the report (v1 `MAX_REPORT_FILES`).
pub const MAX_REPORT_FILES: usize = 500;
/// Blocking reasons listed in the report (v1 `MAX_TOP_REASONS`).
pub const MAX_TOP_REASONS: usize = 8;

/// One top blocking reason, with up to 5 example files (v1
/// `PluginBlockingReason`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopReason {
    pub kind: String,
    pub count: i64,
    /// Up to 5 example files where this reason occurs.
    pub files: Vec<String>,
}

/// Summary of a compatibility report, computed by
/// [`summarize_report`] from either the analyser's shape or the
/// stored v1 shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    /// Files fully convertible.
    pub converted: i64,
    /// Files needing a class runtime or manual work.
    pub partial: i64,
    /// Files with a blocking construct.
    pub blocked: i64,
    /// Files the parser could not read.
    pub unparsed: i64,
    /// PHP files found (top-level `phpFiles`, else `files[].len()`).
    pub php_files: i64,
    /// All files in the ZIP (`totalFiles`, else `php_files`).
    pub total_files: i64,
    /// Overall verdict: `compatible` | `partial` | `blocked` |
    /// `not-analysed` (top-level `verdict` when present, else
    /// derived from the counts).
    pub verdict: String,
    /// Top blocking reasons, most frequent first (max 8).
    pub top_reasons: Vec<TopReason>,
}

/// Parse a stored report (or `None` when absent/invalid — "not
/// analysed"). v1 `parseCompatibilityReport`: the JSON must be an
/// object with a string `verdict` and a numeric `phpFiles`.
pub fn parse_compatibility_report(raw: Option<&str>) -> Option<Value> {
    let raw = raw?;
    if raw.is_empty() {
        return None;
    }
    let parsed: Value = serde_json::from_str(raw).ok()?;
    if !parsed.is_object() {
        return None;
    }
    parsed.get("verdict").and_then(Value::as_str)?;
    parsed.get("phpFiles").and_then(Value::as_i64)?;
    Some(parsed)
}

/// Store a compatibility report for an installed extension
/// (`wl_extensions.report_json`). The report is the analyser's JSON
/// (see the module docs); it is stored verbatim — this function
/// does not analyse anything.
///
/// # Errors
/// * 400 when `report` is not a JSON object.
/// * 404 when `slug` is not installed.
/// * 500 on a database failure.
pub async fn store_compat_report(
    db: &dyn Db,
    slug: &str,
    report: &Value,
) -> Result<(), WordlessError> {
    if !report.is_object() {
        return Err(bad_request("Compatibility report must be a JSON object."));
    }
    let meta = exec_run(
        db,
        "UPDATE wl_extensions SET report_json = ?, updated_at = ? WHERE slug = ?",
        &[
            Param::Text(report.to_string()),
            Param::Text(now_iso()),
            Param::Text(slug.to_string()),
        ],
    )
    .await
    .map_err(WordlessError::from)?;
    if meta.changes == 0 {
        return Err(not_found(&format!(
            "Extension \"{slug}\" is not installed."
        )));
    }
    Ok(())
}

/// Read the stored compatibility report for an installed extension,
/// or `None` when the extension is not installed, was never
/// analysed, or the stored JSON is invalid. A database failure also
/// surfaces as `None` — this signature is fixed by the P3 task;
/// callers that need the distinction (404 vs 500) use
/// [`crate::extensions::get_extension`].
pub async fn get_compat_report(db: &dyn Db, slug: &str) -> Option<Value> {
    let row: Option<Row> = query_one(
        db,
        "SELECT report_json FROM wl_extensions WHERE slug = ? LIMIT 1",
        &[Param::Text(slug.to_string())],
    )
    .await
    .ok()?;
    let row = row?;
    let raw = row.get("report_json").and_then(Value::as_str)?;
    parse_compatibility_report(Some(raw))
}

fn count_field(report: &Value, names: &[&str]) -> Option<i64> {
    for name in names {
        if let Some(n) = report.get(*name).and_then(Value::as_i64) {
            return Some(n);
        }
    }
    None
}

/// Map one per-file verdict string onto a bucket. The analyser
/// emits `convertible`/`partial`/`blocked`/`unparsed` (v1
/// `PluginFileVerdictKind`); `compatible` is accepted as a synonym
/// for `convertible`.
fn verdict_bucket(verdict: &str) -> Option<&'static str> {
    match verdict {
        "convertible" | "compatible" => Some("converted"),
        "partial" => Some("partial"),
        "blocked" => Some("blocked"),
        "unparsed" => Some("unparsed"),
        _ => None,
    }
}

/// Summarise a compatibility report (analyser shape or stored v1
/// shape) into counts, an overall verdict, and the top blocking
/// reasons. Pure function — no I/O.
///
/// Counts prefer the report's top-level numbers when present (they
/// are authoritative when `files[]` was truncated to 500 entries);
/// otherwise they are computed from `files[]`. Blocking reasons
/// prefer a top-level `topReasons` array when present, otherwise
/// they are derived from each file's `blocking_reasons` (falling
/// back to `reasons`).
pub fn summarize_report(report: &Value) -> ReportSummary {
    let files: &[Value] = report
        .get("files")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let mut converted = 0i64;
    let mut partial = 0i64;
    let mut blocked = 0i64;
    let mut unparsed = 0i64;
    for file in files {
        if let Some(v) = file.get("verdict").and_then(Value::as_str) {
            match verdict_bucket(v) {
                Some("converted") => converted += 1,
                Some("partial") => partial += 1,
                Some("blocked") => blocked += 1,
                Some("unparsed") => unparsed += 1,
                _ => {}
            }
        }
    }
    // Top-level counts win when present (v1 shape uses
    // `convertible`; the analyser shape uses `converted`).
    if let Some(n) = count_field(report, &["converted", "convertible"]) {
        converted = n;
    }
    if let Some(n) = count_field(report, &["partial"]) {
        partial = n;
    }
    if let Some(n) = count_field(report, &["blocked"]) {
        blocked = n;
    }
    if let Some(n) = count_field(report, &["unparsed"]) {
        unparsed = n;
    }
    let php_files = count_field(report, &["phpFiles"]).unwrap_or(files.len() as i64);
    let total_files = count_field(report, &["totalFiles"]).unwrap_or(php_files);

    let verdict = match report.get("verdict").and_then(Value::as_str) {
        Some(v) => v.to_string(),
        None => {
            if php_files == 0 {
                "not-analysed".to_string()
            } else if blocked > 0 || unparsed > 0 {
                "blocked".to_string()
            } else if partial > 0 {
                "partial".to_string()
            } else {
                "compatible".to_string()
            }
        }
    };

    // Top reasons: stored `topReasons` first, else derive.
    let mut top_reasons: Vec<TopReason> = Vec::new();
    if let Some(arr) = report.get("topReasons").and_then(Value::as_array) {
        for entry in arr.iter().take(MAX_TOP_REASONS) {
            let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
            let count = entry.get("count").and_then(Value::as_i64).unwrap_or(0);
            let files_vec: Vec<String> = entry
                .get("files")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            if !kind.is_empty() {
                top_reasons.push(TopReason {
                    kind: kind.to_string(),
                    count,
                    files: files_vec,
                });
            }
        }
    } else {
        // Derive from per-file blocking_reasons / reasons.
        let mut counts: Vec<(String, i64, Vec<String>)> = Vec::new();
        for file in files {
            let path = file
                .get("path")
                .or_else(|| file.get("file"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let reasons = file
                .get("blocking_reasons")
                .or_else(|| file.get("reasons"))
                .and_then(Value::as_array);
            if let Some(reasons) = reasons {
                for reason in reasons {
                    let kind = match reason {
                        Value::String(s) => s.clone(),
                        Value::Object(_) => reason
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        _ => String::new(),
                    };
                    if kind.is_empty() {
                        continue;
                    }
                    match counts.iter_mut().find(|(k, _, _)| k == &kind) {
                        Some((_, c, fs)) => {
                            *c += 1;
                            if fs.len() < 5 && !path.is_empty() && !fs.iter().any(|f| f == path) {
                                fs.push(path.to_string());
                            }
                        }
                        None => {
                            let fs = if path.is_empty() {
                                Vec::new()
                            } else {
                                vec![path.to_string()]
                            };
                            counts.push((kind, 1, fs));
                        }
                    }
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        top_reasons = counts
            .into_iter()
            .take(MAX_TOP_REASONS)
            .map(|(kind, count, files)| TopReason { kind, count, files })
            .collect();
    }

    ReportSummary {
        converted,
        partial,
        blocked,
        unparsed,
        php_files,
        total_files,
        verdict,
        top_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        extensions::{install_extension, InstallExtensionInput},
        test_support::{block_on, SqliteDb},
    };
    use serde_json::json;

    fn analyser_report() -> Value {
        // Shape the wordless-conversion analyser produces (per the
        // P3 task): files[] with path/verdict/function + hook
        // counts, blocking_reasons, warnings, plus top-level counts.
        json!({
            "converted": 1, "partial": 1, "blocked": 1,
            "files": [
                { "path": "a.php", "verdict": "convertible",
                  "functions_converted": 3, "functions_total": 3,
                  "hooks_converted": 2, "hooks_total": 2,
                  "blocking_reasons": [], "warnings": [] },
                { "path": "b.php", "verdict": "partial",
                  "functions_converted": 1, "functions_total": 2,
                  "hooks_converted": 0, "hooks_total": 1,
                  "blocking_reasons": ["class"], "warnings": ["class runtime needed"] },
                { "path": "c.php", "verdict": "blocked",
                  "functions_converted": 0, "functions_total": 1,
                  "hooks_converted": 0, "hooks_total": 0,
                  "blocking_reasons": ["eval", "external-api"], "warnings": [] }
            ]
        })
    }

    #[test]
    fn summarize_accepts_the_analyser_shape() {
        let summary = summarize_report(&analyser_report());
        assert_eq!(summary.converted, 1);
        assert_eq!(summary.partial, 1);
        assert_eq!(summary.blocked, 1);
        assert_eq!(summary.unparsed, 0);
        assert_eq!(summary.php_files, 3);
        assert_eq!(summary.verdict, "blocked");
        let kinds: Vec<&str> = summary
            .top_reasons
            .iter()
            .map(|r| r.kind.as_str())
            .collect();
        assert!(kinds.contains(&"class"), "{kinds:?}");
        assert!(kinds.contains(&"eval"), "{kinds:?}");
        let class = summary
            .top_reasons
            .iter()
            .find(|r| r.kind == "class")
            .unwrap();
        assert_eq!(class.files, vec!["b.php"]);
    }

    #[test]
    fn summarize_derives_counts_and_verdict_without_top_level_numbers() {
        let report = json!({
            "files": [
                { "path": "a.php", "verdict": "convertible", "blocking_reasons": [] },
                { "path": "b.php", "verdict": "partial", "blocking_reasons": ["class"] },
                { "path": "c.php", "verdict": "unparsed", "blocking_reasons": ["parse-error"] }
            ]
        });
        let summary = summarize_report(&report);
        assert_eq!(summary.converted, 1);
        assert_eq!(summary.partial, 1);
        assert_eq!(summary.unparsed, 1);
        assert_eq!(summary.verdict, "blocked", "unparsed forces blocked");
        assert_eq!(summary.php_files, 3);
    }

    #[test]
    fn summarize_reads_the_stored_v1_shape() {
        let report = json!({
            "phpFiles": 2, "totalFiles": 5, "convertible": 1, "partial": 1,
            "blocked": 0, "unparsed": 0, "skipped": 0, "verdict": "partial",
            "topReasons": [{ "kind": "class", "count": 1, "files": ["b.php"] }],
            "files": [], "filesTruncated": false,
            "optionDefaults": [], "conversion": null,
            "analysedAt": "2026-11-19T00:00:00Z"
        });
        let summary = summarize_report(&report);
        assert_eq!(summary.converted, 1);
        assert_eq!(summary.partial, 1);
        assert_eq!(summary.total_files, 5);
        assert_eq!(summary.verdict, "partial");
        assert_eq!(summary.top_reasons.len(), 1);
        assert_eq!(summary.top_reasons[0].kind, "class");
    }

    #[test]
    fn summarize_reports_no_php_files_as_not_analysed() {
        let summary = summarize_report(&json!({ "files": [] }));
        assert_eq!(summary.verdict, "not-analysed");
        assert_eq!(summary.php_files, 0);
    }

    #[test]
    fn store_and_get_round_trip_through_d1() {
        let db = SqliteDb::new();
        block_on(install_extension(
            &db,
            &InstallExtensionInput {
                slug: "mixed-plugin".to_string(),
                name: "Mixed".to_string(),
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(block_on(get_compat_report(&db, "mixed-plugin")), None);
        // get_compat_report requires the v1 stored shape (verdict +
        // phpFiles); store the analyser shape enriched the way the
        // worker wiring stores it (with the v1 envelope fields).
        let mut stored = analyser_report();
        stored["phpFiles"] = json!(3);
        stored["totalFiles"] = json!(3);
        stored["verdict"] = json!("blocked");
        stored["topReasons"] = json!([{ "kind": "class", "count": 1, "files": ["b.php"] }]);
        block_on(store_compat_report(&db, "mixed-plugin", &stored)).unwrap();
        let fetched = block_on(get_compat_report(&db, "mixed-plugin")).unwrap();
        assert_eq!(fetched["verdict"], json!("blocked"));
        assert_eq!(summarize_report(&fetched).blocked, 1);
        // Unknown slug: None, and store is a 404.
        assert_eq!(block_on(get_compat_report(&db, "nope")), None);
        let err = block_on(store_compat_report(&db, "nope", &stored)).unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[test]
    fn parse_rejects_non_reports() {
        assert_eq!(parse_compatibility_report(None), None);
        assert_eq!(parse_compatibility_report(Some("")), None);
        assert_eq!(parse_compatibility_report(Some("not json")), None);
        assert_eq!(
            parse_compatibility_report(Some("{\"verdict\": \"partial\"}")),
            None
        );
        assert!(
            parse_compatibility_report(Some("{\"verdict\": \"partial\", \"phpFiles\": 1}"))
                .is_some()
        );
    }
}
