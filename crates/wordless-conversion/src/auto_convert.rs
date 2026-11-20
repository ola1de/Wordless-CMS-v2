//! Auto-convert on upload — a port of v1
//! `src/modules/conversion/auto-convert.ts` (Milestone 11).
//!
//! When a theme or plugin ZIP is uploaded, Wordless converts it, not
//! just analyses it: every PHP file goes through the M4 engine
//! ([`convert_php`]) and the generated TypeScript is stored in R2 under
//! `<prefix>/converted/<path>.ts`, next to the original files. Settings
//! travel too: literal `get_option()` defaults found in the PHP are
//! carried over as Wordless options (only when the option does not
//! already exist), and a theme's `theme.json` settings are reported as
//! applied by the theme engine (CSS presets) or skipped with a reason.
//!
//! Honest boundary (same as v1): "exact, nothing lost" conversion of
//! arbitrary PHP is NOT possible. Every upload produces a per-file
//! record (converted / partial / blocked + reason) and a
//! settings-migration report — nothing is silent, and nothing is ever
//! labelled "fully converted" while the record shows partial or blocked
//! items. Converted code is stored, never executed.

use serde::{Deserialize, Serialize};

use crate::analysis::{analyze_php, OptionDefault};
use crate::emitter::UnconvertedItem;
use crate::pipeline::{convert_php, ConversionError, ConvertOptions};
use wordless_db::{query_one, Db, Param, Row};

/// Worst case per upload: 500 PHP files converted, the rest skipped.
pub const MAX_CONVERT_PHP_FILES: usize = 500;
/// Per-file conversion records stored in the report.
pub const MAX_CONVERT_REPORT_FILES: usize = 500;
/// Per-file conversion cap (same as the analyser's cap).
pub const MAX_CONVERT_PHP_BYTES: usize = 1024 * 1024;
/// Files the analyser looks at for option defaults (v1 compat-report).
pub const MAX_ANALYSED_PHP_FILES: usize = 2000;

/// Per-file auto-convert status (v1 `AutoConvertFileStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoConvertFileStatus {
    Converted,
    Partial,
    Blocked,
}

/// One file's conversion record (v1 `AutoConvertedFile`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoConvertedFile {
    pub file: String,
    pub status: AutoConvertFileStatus,
    #[serde(rename = "functionsConverted")]
    pub functions_converted: Vec<String>,
    #[serde(rename = "hooksConverted")]
    pub hooks_converted: Vec<String>,
    pub unconverted: Vec<UnconvertedItem>,
    /// Human-readable summary of what happened to this file.
    pub reason: String,
    /// R2 path of the stored TypeScript (null when nothing was generated).
    #[serde(rename = "generatedPath")]
    pub generated_path: Option<String>,
}

/// One settings-migration entry (v1 `SettingsMigrationEntry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsMigrationEntry {
    pub key: String,
    pub value: serde_json::Value,
    pub source: SettingsSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingsSource {
    #[serde(rename = "php-default")]
    PhpDefault,
    #[serde(rename = "theme.json")]
    ThemeJson,
}

/// Settings migration report (v1 `SettingsMigrationReport`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsMigrationReport {
    #[serde(rename = "carriedOver")]
    pub carried_over: Vec<SettingsMigrationEntry>,
    pub skipped: Vec<SettingsMigrationEntry>,
}

/// The auto-convert report for one upload (v1 `AutoConvertReport`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoConvertReport {
    /// `"theme"` or `"plugin"`.
    pub kind: String,
    pub slug: String,
    #[serde(rename = "phpFiles")]
    pub php_files: usize,
    pub converted: usize,
    pub partial: usize,
    pub blocked: usize,
    pub skipped: usize,
    pub files: Vec<AutoConvertedFile>,
    #[serde(rename = "filesTruncated")]
    pub files_truncated: bool,
    pub settings: SettingsMigrationReport,
    #[serde(rename = "convertedAt")]
    pub converted_at: String,
}

fn is_php_file(path: &str) -> bool {
    path.to_lowercase().ends_with(".php")
}

/// Convert every PHP file in an extracted ZIP. Pure except for no I/O:
/// returns the generated TypeScript per file (keyed by R2 path
/// `converted/<path>.ts`); the caller stores it. Nothing here executes
/// PHP.
pub fn convert_php_files(
    files: &[(String, String)],
    slug: &str,
    kind: &str,
) -> (Vec<(String, String)>, AutoConvertReport) {
    let mut php_paths: Vec<&str> = files
        .iter()
        .map(|(p, _)| p.as_str())
        .filter(|p| is_php_file(p))
        .collect();
    php_paths.sort();
    let mut generated: Vec<(String, String)> = Vec::new();
    let mut records: Vec<AutoConvertedFile> = Vec::new();
    let mut skipped = 0usize;
    let mut converted_count = 0usize;

    for path in &php_paths {
        let Some((_, text)) = files.iter().find(|(p, _)| p == path) else {
            continue;
        };
        if converted_count >= MAX_CONVERT_PHP_FILES || text.len() > MAX_CONVERT_PHP_BYTES {
            skipped += 1;
            records.push(AutoConvertedFile {
                file: (*path).to_string(),
                status: AutoConvertFileStatus::Blocked,
                functions_converted: Vec::new(),
                hooks_converted: Vec::new(),
                unconverted: Vec::new(),
                reason: if text.len() > MAX_CONVERT_PHP_BYTES {
                    "Skipped: file is over the 1 MB per-file conversion cap.".to_string()
                } else {
                    format!(
                        "Skipped: only the first {MAX_CONVERT_PHP_FILES} PHP files are converted per upload."
                    )
                },
                generated_path: None,
            });
            continue;
        }
        converted_count += 1;
        match convert_php(
            text,
            path,
            &ConvertOptions {
                slug: slug.to_string(),
                compat_import: None,
                source_label: Some(format!("{slug}/{path}")),
            },
        ) {
            Ok(converted) => {
                // Mirror the TS `replace(/\.php$/i, ".ts")`.
                let generated_path = if path.to_lowercase().ends_with(".php") {
                    format!("converted/{}.ts", &path[..path.len() - 4])
                } else {
                    format!("converted/{path}.ts")
                };
                generated.push((generated_path.clone(), converted.output.code.clone()));
                let partial = !converted.unconverted.is_empty() || converted.ast.classes > 0;
                let reason = if partial {
                    let mut r = format!(
                        "{} item{} could not be converted",
                        converted.unconverted.len(),
                        if converted.unconverted.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    );
                    if converted.ast.classes > 0 {
                        r.push_str(&format!(
                            "; {} class{} need{} a class runtime",
                            converted.ast.classes,
                            if converted.ast.classes == 1 { "" } else { "es" },
                            if converted.ast.classes == 1 { "s" } else { "" }
                        ));
                    }
                    r.push('.');
                    r
                } else {
                    let mut r = format!(
                        "Converted {} function{}",
                        converted.converted.functions.len(),
                        if converted.converted.functions.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    );
                    if !converted.converted.hooks.is_empty() {
                        r.push_str(&format!(
                            " and {} hook{}",
                            converted.converted.hooks.len(),
                            if converted.converted.hooks.len() == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ));
                    }
                    r.push('.');
                    r
                };
                records.push(AutoConvertedFile {
                    file: (*path).to_string(),
                    status: if partial {
                        AutoConvertFileStatus::Partial
                    } else {
                        AutoConvertFileStatus::Converted
                    },
                    functions_converted: converted.converted.functions.clone(),
                    hooks_converted: converted
                        .converted
                        .hooks
                        .iter()
                        .map(|h| format!("{}:{}", h.kind, h.tag))
                        .collect(),
                    unconverted: converted.unconverted.clone(),
                    reason,
                    generated_path: Some(generated_path),
                });
            }
            Err(ConversionError::Blocked(e)) => {
                records.push(AutoConvertedFile {
                    file: (*path).to_string(),
                    status: AutoConvertFileStatus::Blocked,
                    functions_converted: Vec::new(),
                    hooks_converted: Vec::new(),
                    unconverted: e
                        .violations
                        .iter()
                        .map(|v| UnconvertedItem {
                            kind: v.construct.clone(),
                            name: None,
                            reason: v.reason.clone(),
                            line: v.line,
                        })
                        .collect(),
                    reason: format!(
                        "Blocked by the conversion sandbox: {}.",
                        e.violations
                            .iter()
                            .map(|v| v.construct.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    generated_path: None,
                });
            }
            Err(ConversionError::Parse(e)) => {
                let msg = e.0.chars().take(200).collect::<String>();
                records.push(AutoConvertedFile {
                    file: (*path).to_string(),
                    status: AutoConvertFileStatus::Blocked,
                    functions_converted: Vec::new(),
                    hooks_converted: Vec::new(),
                    unconverted: Vec::new(),
                    reason: format!("Could not be parsed: {msg}"),
                    generated_path: None,
                });
            }
        }
    }

    let count = |s: AutoConvertFileStatus| records.iter().filter(|r| r.status == s).count();
    let converted_n = count(AutoConvertFileStatus::Converted);
    let partial_n = count(AutoConvertFileStatus::Partial);
    let blocked_n = count(AutoConvertFileStatus::Blocked);
    let rank = |s: AutoConvertFileStatus| match s {
        AutoConvertFileStatus::Blocked => 0,
        AutoConvertFileStatus::Partial => 1,
        AutoConvertFileStatus::Converted => 2,
    };
    let mut ordered = records;
    ordered.sort_by(|a, b| {
        rank(a.status)
            .cmp(&rank(b.status))
            .then_with(|| a.file.cmp(&b.file))
    });
    let files_truncated = ordered.len() > MAX_CONVERT_REPORT_FILES;
    let report = AutoConvertReport {
        kind: kind.to_string(),
        slug: slug.to_string(),
        php_files: php_paths.len(),
        converted: converted_n,
        partial: partial_n,
        blocked: blocked_n,
        skipped,
        files: ordered.into_iter().take(MAX_CONVERT_REPORT_FILES).collect(),
        files_truncated,
        settings: SettingsMigrationReport::default(),
        converted_at: wordless_kernel::time::now_iso(),
    };
    (generated, report)
}

/// Collect literal option defaults from PHP files (analysis only, no
/// conversion). Unparseable files are skipped here — the conversion
/// record already says so.
pub fn collect_option_defaults(files: &[(String, String)]) -> Vec<OptionDefault> {
    let mut out: Vec<OptionDefault> = Vec::new();
    let mut php_paths: Vec<&str> = files
        .iter()
        .map(|(p, _)| p.as_str())
        .filter(|p| is_php_file(p))
        .collect();
    php_paths.sort();
    let mut analysed = 0usize;
    for path in php_paths {
        let Some((_, text)) = files.iter().find(|(p, _)| p == path) else {
            continue;
        };
        if analysed >= MAX_ANALYSED_PHP_FILES || text.len() > MAX_CONVERT_PHP_BYTES {
            continue;
        }
        analysed += 1;
        if let Ok(a) = analyze_php(text, path) {
            out.extend(a.option_defaults);
        }
    }
    out
}

/// Carry literal option defaults over as Wordless options. Only options
/// that do not already exist are written; existing ones are reported
/// as skipped, never overwritten. Non-literal defaults are never
/// collected, so they never appear here.
pub async fn migrate_option_defaults(
    db: &dyn Db,
    defaults: &[OptionDefault],
) -> Result<SettingsMigrationReport, wordless_db::DbError> {
    let mut carried_over: Vec<SettingsMigrationEntry> = Vec::new();
    let mut skipped: Vec<SettingsMigrationEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = Default::default();
    for def in defaults {
        if !seen.insert(def.key.clone()) {
            continue;
        }
        // `INSERT OR IGNORE` (same as v1 `addOption`): returns whether
        // a row was actually inserted. Uses the primitive `db.exec`
        // (not `exec_run`, which needs `prepare`) so this also works
        // against fakes that only implement the primitive methods.
        let json = serde_json::to_string(&def.default_value).unwrap_or_else(|_| "null".to_string());
        let meta = db
            .exec(
                "INSERT OR IGNORE INTO wp_options (option_name, option_value, autoload) VALUES (?, ?, 'yes')",
                &[
                    Param::Text(def.key.clone()),
                    Param::Text(json),
                ],
            )
            .await?;
        if meta.changes > 0 {
            carried_over.push(SettingsMigrationEntry {
                key: def.key.clone(),
                value: def.default_value.clone(),
                source: SettingsSource::PhpDefault,
                reason: None,
            });
        } else {
            skipped.push(SettingsMigrationEntry {
                key: def.key.clone(),
                value: def.default_value.clone(),
                source: SettingsSource::PhpDefault,
                reason: Some("Option already exists — kept the existing value.".to_string()),
            });
        }
    }
    Ok(SettingsMigrationReport {
        carried_over,
        skipped,
    })
}

/// Summarise a theme.json's settings as a migration report: the theme
/// engine applies these as CSS (presets), so they count as carried
/// over; settings paths the engine reports as unsupported count as
/// skipped. Counts come from the parsed theme.json object itself.
pub fn summarize_theme_json_settings(
    theme_json: &serde_json::Value,
    unsupported: &[String],
) -> SettingsMigrationReport {
    let mut carried_over: Vec<SettingsMigrationEntry> = Vec::new();
    let mut skipped: Vec<SettingsMigrationEntry> = Vec::new();
    if let Some(settings) = theme_json.get("settings").and_then(|s| s.as_object()) {
        let count = |v: Option<&serde_json::Value>| -> usize {
            v.and_then(|x| x.as_array()).map(|a| a.len()).unwrap_or(0)
        };
        let palette = settings
            .get("color")
            .and_then(|c| c.as_object())
            .and_then(|c| c.get("palette"));
        let font_sizes = settings
            .get("typography")
            .and_then(|t| t.as_object())
            .and_then(|t| t.get("fontSizes"));
        let spacing_sizes = settings
            .get("spacing")
            .and_then(|s| s.as_object())
            .and_then(|s| s.get("spacingSizes"));
        if count(palette) > 0 {
            carried_over.push(SettingsMigrationEntry {
                key: "theme.json color palette".to_string(),
                value: serde_json::Value::from(count(palette)),
                source: SettingsSource::ThemeJson,
                reason: None,
            });
        }
        if count(font_sizes) > 0 {
            carried_over.push(SettingsMigrationEntry {
                key: "theme.json font sizes".to_string(),
                value: serde_json::Value::from(count(font_sizes)),
                source: SettingsSource::ThemeJson,
                reason: None,
            });
        }
        if count(spacing_sizes) > 0 {
            carried_over.push(SettingsMigrationEntry {
                key: "theme.json spacing sizes".to_string(),
                value: serde_json::Value::from(count(spacing_sizes)),
                source: SettingsSource::ThemeJson,
                reason: None,
            });
        }
        if settings
            .get("layout")
            .map(|l| l.is_object())
            .unwrap_or(false)
        {
            carried_over.push(SettingsMigrationEntry {
                key: "theme.json layout widths".to_string(),
                value: serde_json::Value::from(1),
                source: SettingsSource::ThemeJson,
                reason: None,
            });
        }
    }
    for path in unsupported {
        skipped.push(SettingsMigrationEntry {
            key: format!("theme.json {path}"),
            value: serde_json::Value::from(0),
            source: SettingsSource::ThemeJson,
            reason: Some("Not supported by the Wordless theme engine — ignored.".to_string()),
        });
    }
    SettingsMigrationReport {
        carried_over,
        skipped,
    }
}

/// Merge a theme.json summary into a conversion report's settings.
pub fn merge_settings_reports(
    into: SettingsMigrationReport,
    extra: SettingsMigrationReport,
) -> SettingsMigrationReport {
    let mut carried_over = into.carried_over;
    carried_over.extend(extra.carried_over);
    let mut skipped = into.skipped;
    skipped.extend(extra.skipped);
    SettingsMigrationReport {
        carried_over,
        skipped,
    }
}

/// Read an option's existence without decoding it (used by tests and
/// by callers that need the v1 `addOption` boolean shape).
pub async fn option_exists(db: &dyn Db, key: &str) -> Result<bool, wordless_db::DbError> {
    let row: Option<Row> = query_one(
        db,
        "SELECT option_value FROM wp_options WHERE option_name = ? LIMIT 1",
        &[Param::Text(key.to_string())],
    )
    .await?;
    Ok(row.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;

    const CONVERTIBLE_PHP: &str = r#"<?php
if (!defined('ABSPATH')) exit;
function my_plugin_init() {
  add_action('init', 'my_plugin_init');
  return get_option('my_plugin_option', 'hello');
}
"#;

    const CLASS_PHP: &str = r#"<?php
class My_Widget {
  public function render() {
    echo 'hello';
  }
}
"#;

    const EVAL_PHP: &str = r#"<?php
function run_code($code) {
  eval($code);
}
"#;

    fn files(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(p, s)| (p.to_string(), s.to_string()))
            .collect()
    }

    #[test]
    fn converts_clean_procedural_file_and_generates_typescript() {
        let (generated, report) = convert_php_files(
            &files(&[("my-plugin.php", CONVERTIBLE_PHP)]),
            "my-plugin",
            "plugin",
        );
        assert_eq!(report.converted, 1);
        assert_eq!(report.partial, 0);
        assert_eq!(report.blocked, 0);
        assert_eq!(report.files[0].status, AutoConvertFileStatus::Converted);
        assert!(report.files[0]
            .functions_converted
            .contains(&"my_plugin_init".to_string()));
        assert_eq!(
            report.files[0].generated_path.as_deref(),
            Some("converted/my-plugin.ts")
        );
        let code = generated
            .iter()
            .find(|(p, _)| p == "converted/my-plugin.ts")
            .map(|(_, c)| c.clone());
        assert!(code.is_some());
        assert!(code.unwrap().contains("my_plugin_init"));
    }

    #[test]
    fn marks_class_files_partial_and_eval_files_blocked_with_reasons() {
        let (generated, report) = convert_php_files(
            &files(&[("a.php", CLASS_PHP), ("b.php", EVAL_PHP)]),
            "mixed",
            "plugin",
        );
        assert_eq!(report.converted, 0);
        assert_eq!(report.partial, 1);
        assert_eq!(report.blocked, 1);
        let a = report.files.iter().find(|f| f.file == "a.php").unwrap();
        assert_eq!(a.status, AutoConvertFileStatus::Partial);
        assert!(a.reason.contains("class runtime"), "{}", a.reason);
        let b = report.files.iter().find(|f| f.file == "b.php").unwrap();
        assert_eq!(b.status, AutoConvertFileStatus::Blocked);
        assert!(b.reason.contains("sandbox"), "{}", b.reason);
        assert!(b.generated_path.is_none());
        assert!(generated.iter().any(|(p, _)| p == "converted/a.ts"));
        assert!(!generated.iter().any(|(p, _)| p == "converted/b.ts"));
    }

    #[test]
    fn marks_unparseable_php_blocked_instead_of_crashing() {
        let (_, report) = convert_php_files(
            &files(&[("bad.php", "<?php $x = fn($a) => $a * 2;")]),
            "bad",
            "plugin",
        );
        assert_eq!(report.blocked, 1);
        assert_eq!(report.files[0].status, AutoConvertFileStatus::Blocked);
        assert!(
            report.files[0].reason.contains("Could not be parsed"),
            "{}",
            report.files[0].reason
        );
    }

    #[test]
    fn ignores_non_php_files_and_reports_zero_php_files() {
        let (_, report) =
            convert_php_files(&files(&[("readme.txt", "hello")]), "assets-only", "plugin");
        assert_eq!(report.php_files, 0);
        assert!(report.files.is_empty());
    }

    #[test]
    fn collects_literal_defaults_and_carries_them_over_as_options() {
        let db = SqliteDb::new();
        let defaults = collect_option_defaults(&files(&[("p.php", CONVERTIBLE_PHP)]));
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].key, "my_plugin_option");
        assert_eq!(
            defaults[0].default_value,
            serde_json::Value::String("hello".to_string())
        );
        assert_eq!(defaults[0].file, "p.php");
        let result = block_on(migrate_option_defaults(&db, &defaults)).expect("migrate");
        assert_eq!(result.carried_over.len(), 1);
        assert_eq!(result.carried_over[0].key, "my_plugin_option");
        assert!(result.skipped.is_empty());
        let got: serde_json::Value = block_on(wordless_db::options::get_option(
            &db,
            "my_plugin_option",
            serde_json::Value::Null,
        ))
        .expect("read");
        assert_eq!(got, serde_json::Value::String("hello".to_string()));
    }

    #[test]
    fn never_overwrites_an_option_that_already_exists() {
        let db = SqliteDb::new();
        block_on(wordless_db::options::set_option(
            &db,
            "my_plugin_option",
            &"existing",
        ))
        .expect("seed");
        let result = block_on(migrate_option_defaults(
            &db,
            &[OptionDefault {
                key: "my_plugin_option".to_string(),
                default_value: serde_json::Value::String("hello".to_string()),
                file: "p.php".to_string(),
            }],
        ))
        .expect("migrate");
        assert!(result.carried_over.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert!(result.skipped[0]
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("already exists"));
        let got: serde_json::Value = block_on(wordless_db::options::get_option(
            &db,
            "my_plugin_option",
            serde_json::Value::Null,
        ))
        .expect("read");
        assert_eq!(got, serde_json::Value::String("existing".to_string()));
    }

    #[test]
    fn collects_number_and_boolean_defaults_ignores_non_literal_ones() {
        let defaults = collect_option_defaults(&files(&[(
            "p.php",
            "<?php $a = get_option('num_opt', 42); $b = get_option('bool_opt', true); $c = get_option('dyn_opt', SOME_CONST); $d = get_option($dynamic_key, 'x');",
        )]));
        let mut keys: Vec<&str> = defaults.iter().map(|d| d.key.as_str()).collect();
        keys.sort();
        assert_eq!(keys, vec!["bool_opt", "num_opt"]);
        assert_eq!(
            defaults
                .iter()
                .find(|d| d.key == "num_opt")
                .map(|d| d.default_value.clone()),
            Some(serde_json::Value::from(42))
        );
        assert_eq!(
            defaults
                .iter()
                .find(|d| d.key == "bool_opt")
                .map(|d| d.default_value.clone()),
            Some(serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn summarizes_theme_json_settings() {
        let theme_json = serde_json::json!({
            "settings": {
                "color": { "palette": [{ "slug": "a" }, { "slug": "b" }, { "slug": "c" }] },
                "typography": { "fontSizes": [{ "slug": "s" }, { "slug": "m" }] },
                "spacing": { "spacingSizes": [{ "slug": "10" }] },
                "layout": { "contentSize": "640px" }
            }
        });
        let result = summarize_theme_json_settings(
            &theme_json,
            &["settings.typography.writingMode".to_string()],
        );
        let keys: Vec<&str> = result.carried_over.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "theme.json color palette",
                "theme.json font sizes",
                "theme.json spacing sizes",
                "theme.json layout widths"
            ]
        );
        assert_eq!(result.carried_over[0].value, serde_json::Value::from(3));
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(
            result.skipped[0].key,
            "theme.json settings.typography.writingMode"
        );
    }
}
