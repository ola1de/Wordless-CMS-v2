//! WORDLESS v2 P3 — PHP → TypeScript conversion engine.
//!
//! A port of the v1 conversion engine (`wordless-cms/src/conversion/`,
//! Milestone 4) plus the M11 auto-convert module
//! (`src/modules/conversion/auto-convert.ts`) and the conversion jobs
//! service (`src/modules/conversion/service.ts`). It never executes PHP:
//! every step is static analysis over a hand-written lexer and
//! recursive-descent parser (grown out of the P0 prototype in
//! `~/workspace/wordless/rust-prototype/`, whose AST is a superset of
//! the TypeScript AST — it also parses files the TS parser rejects,
//! e.g. `instanceof`, `foreach … as &$v`, and `=&`).
//!
//! # Pipeline
//!
//! 1. [`safety::scan_safety`] — token-level sandbox scan. Blocked
//!    constructs (`eval`, `exec`, `shell_exec`, `system`, backticks,
//!    `create_function`, dynamic includes, untrusted `unserialize`,
//!    …) fail the job outright.
//! 2. [`parser::parse_php`] — lex + parse into the [`ast`] tree.
//! 3. [`emitter::emit_typescript`] — AST → TypeScript module. Every
//!    PHP function becomes a TS function taking the WordPress API
//!    (`wp`) as its first parameter; `add_action` / `add_filter`
//!    become hook registrations in `activate()`; `echo` / `printf`
//!    output is collected into a returned string. Unmapped calls become
//!    throwing `__unmapped_<name>` helpers and are listed in the report.
//! 4. [`analysis::analyze_php`] — per-file static analysis (functions,
//!    classes, hooks, WP calls, option defaults, verdict).
//!
//! # Public API (fixed contract — the marketplace crate calls these)
//!
//! - [`analyze_php_file`] / [`analyze_plugin`] — per-file and
//!   per-plugin conversion verdicts and counts.
//! - [`convert_php_file`] — one file to TypeScript + its report.
//! - [`check_safety`] — blocking reasons for a source file, empty
//!   when safe.
//! - [`pipeline::convert_php`] — the full pipeline with the detailed
//!   [`pipeline::ConversionReport`] (what the jobs service stores).
//! - [`auto_convert::convert_php_files`] — a whole upload: generated
//!   TypeScript per file plus the v1-shaped auto-convert report.
//! - [`auto_convert::migrate_option_defaults`] — carry literal
//!   `get_option()` defaults over as Wordless options, never
//!   overwriting existing values. (The job-record half of this module
//!   — `create/get/list/run_conversion_job` plus the settings
//!   migration — moved to `wordless-conversion-jobs` in the 2026-11-20
//!   split; this crate keeps the engine only.)
//!
//! # Honest limits
//!
//! - Classes are analysed, not converted: Wordless has no class
//!   runtime yet, so any file defining a class is `Partial` at best.
//! - `switch` is not translated (PHP loose comparison differs from
//!   TypeScript); `global`, `$this`, superglobals, method/static
//!   calls, `new`, and dynamic calls are reported, not translated.
//! - The "generated TS still typechecks" check is done by exact-string
//!   assertions against the v1 fixture (see the tests): `tsc` cannot
//!   run inside `cargo test`.

pub mod analysis;
pub mod ast;
pub mod auto_convert;
pub mod emitter;
pub mod lexer;
pub mod parser;
pub mod pipeline;
pub mod safety;

#[cfg(test)]
mod test_support;

use serde::{Deserialize, Serialize};

pub use analysis::{
    analyze_php, Convertibility, FileAnalysis, HookRef, OptionDefault, UnconvertibleFeature,
};
pub use auto_convert::{
    collect_option_defaults, convert_php_files, migrate_option_defaults,
    summarize_theme_json_settings, AutoConvertFileStatus, AutoConvertReport, AutoConvertedFile,
    SettingsMigrationReport,
};
pub use emitter::{emit_typescript, EmitOptions, EmitReport};
pub use pipeline::{convert_php, ConversionReport, ConvertOptions};
pub use safety::{check_safety, scan_safety, SafetyResult, SafetyViolation};

pub use pipeline::ConversionError;

/// Per-file conversion verdict (the contract enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileVerdict {
    /// Converted with nothing unconverted and no classes.
    Converted,
    /// Converted, but some constructs need manual work (or the file
    /// defines classes, which are analysed but not converted).
    Partial,
    /// Not converted: blocked by the sandbox, unparseable, or skipped
    /// by the per-upload caps.
    Blocked,
}

/// The per-file conversion record (the contract struct).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReport {
    /// File path, exactly as supplied.
    pub path: String,
    pub verdict: FileVerdict,
    /// Top-level PHP functions emitted as TypeScript functions.
    pub functions_converted: usize,
    /// Top-level PHP functions found in the file.
    pub functions_total: usize,
    /// Hook registrations translated into `activate()`.
    pub hooks_converted: usize,
    /// Hook registrations found in the file (literal tag).
    pub hooks_total: usize,
    /// Why the file is not fully converted (empty when converted).
    /// For blocked files these are the sandbox / parse reasons; for
    /// partial files the unconverted items, as
    /// `line {line}: {kind} — {reason}`.
    pub blocking_reasons: Vec<String>,
    /// Non-fatal translation warnings from the emitter.
    pub warnings: Vec<String>,
}

/// The per-plugin conversion record (the contract struct).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginReport {
    /// Per-file records, ordered blocked → partial → converted, then
    /// by path (the v1 auto-convert record order).
    pub files: Vec<FileReport>,
    /// Files with verdict [`FileVerdict::Converted`].
    pub converted: usize,
    /// Files with verdict [`FileVerdict::Partial`].
    pub partial: usize,
    /// Files with verdict [`FileVerdict::Blocked`].
    pub blocked: usize,
}

/// One converted file (the contract struct).
#[derive(Debug, Clone)]
pub struct ConvertedFile {
    /// The generated TypeScript module.
    pub typescript: String,
    /// The per-file conversion record.
    pub report: FileReport,
}

/// Analyse one PHP file: safety scan → parse → emit, and summarise the
/// result as a [`FileReport`].
///
/// Verdict rules (the v1 M11 auto-convert record semantics, so the
/// numbers agree with the v1 import record):
/// - `Blocked` — the safety scan found a blocked construct, or the
///   file could not be parsed.
/// - `Partial` — conversion succeeded but something could not be
///   converted (any unconverted item) or the file defines a class.
/// - `Converted` — everything converted, no classes.
///
/// `functions_total` / `hooks_total` come from the static analysis;
/// `functions_converted` / `hooks_converted` from the emit report, so
/// a hook whose callback is not defined in the file counts in the
/// total but not in the converted count.
pub fn analyze_php_file(path: &str, source: &str) -> FileReport {
    let safety_reasons = check_safety(source);
    let analysis: Option<FileAnalysis> = analyze_php(source, path).ok();
    let functions_total = analysis.as_ref().map(|a| a.functions.len()).unwrap_or(0);
    let hooks_total = analysis.as_ref().map(|a| a.hooks.len()).unwrap_or(0);

    if !safety_reasons.is_empty() {
        return FileReport {
            path: path.to_string(),
            verdict: FileVerdict::Blocked,
            functions_converted: 0,
            functions_total,
            hooks_converted: 0,
            hooks_total,
            blocking_reasons: safety_reasons,
            warnings: Vec::new(),
        };
    }

    match convert_php(
        source,
        path,
        &ConvertOptions {
            slug: pipeline::slugify_filename(path),
            compat_import: None,
            source_label: None,
        },
    ) {
        Ok(report) => {
            let verdict = if !report.unconverted.is_empty() || report.ast.classes > 0 {
                FileVerdict::Partial
            } else {
                FileVerdict::Converted
            };
            FileReport {
                path: path.to_string(),
                verdict,
                functions_converted: report.converted.functions.len(),
                functions_total,
                hooks_converted: report.converted.hooks.len(),
                hooks_total,
                blocking_reasons: report
                    .unconverted
                    .iter()
                    .map(|u| format!("line {}: {} — {}", u.line, u.kind, u.reason))
                    .collect(),
                warnings: report.warnings.clone(),
            }
        }
        Err(ConversionError::Blocked(e)) => FileReport {
            path: path.to_string(),
            verdict: FileVerdict::Blocked,
            functions_converted: 0,
            functions_total,
            hooks_converted: 0,
            hooks_total,
            blocking_reasons: vec![e.to_string()],
            warnings: Vec::new(),
        },
        Err(ConversionError::Parse(e)) => FileReport {
            path: path.to_string(),
            verdict: FileVerdict::Blocked,
            functions_converted: 0,
            functions_total,
            hooks_converted: 0,
            hooks_total,
            blocking_reasons: vec![e.to_string()],
            warnings: Vec::new(),
        },
    }
}

/// Convert one PHP file to a TypeScript module plus its [`FileReport`].
///
/// The extension slug is derived from the filename (v1
/// `slugifyFilename`); for the Hello Dolly fixture the v1 report used
/// the explicit slug `"hello-dolly"` — use [`convert_php`] directly
/// when a specific slug is needed. A safety-blocked or unparseable
/// file produces an empty module and a `Blocked` report instead of
/// failing: the caller decides what to do with it.
pub fn convert_php_file(path: &str, source: &str) -> ConvertedFile {
    let report = analyze_php_file(path, source);
    let typescript = match report.verdict {
        FileVerdict::Blocked => String::new(),
        _ => convert_php(
            source,
            path,
            &ConvertOptions {
                slug: pipeline::slugify_filename(path),
                compat_import: None,
                source_label: None,
            },
        )
        .map(|r| r.output.code)
        .unwrap_or_default(),
    };
    ConvertedFile { typescript, report }
}

/// Analyse a whole plugin/theme (a list of `(path, source)` pairs) and
/// return the per-plugin conversion record.
///
/// Files are converted in sorted-path order; at most 500 PHP files are
/// converted per call and files over 1 MB are recorded as skipped
/// (`Blocked`), mirroring the v1 auto-convert caps. Non-PHP files are
/// ignored. The returned files are ordered blocked → partial →
/// converted, then by path, exactly like the v1 record.
pub fn analyze_plugin(files: &[(String, String)]) -> PluginReport {
    let mut php: Vec<&(String, String)> = files
        .iter()
        .filter(|(p, _)| p.to_lowercase().ends_with(".php"))
        .collect();
    php.sort_by(|a, b| a.0.cmp(&b.0));
    let mut reports: Vec<FileReport> = Vec::new();
    let mut converted_count = 0usize;
    for (path, source) in &php {
        if converted_count >= auto_convert::MAX_CONVERT_PHP_FILES
            || source.len() > auto_convert::MAX_CONVERT_PHP_BYTES
        {
            reports.push(FileReport {
                path: (*path).clone(),
                verdict: FileVerdict::Blocked,
                functions_converted: 0,
                functions_total: analyze_php(source, path)
                    .map(|a| a.functions.len())
                    .unwrap_or(0),
                hooks_converted: 0,
                hooks_total: analyze_php(source, path)
                    .map(|a| a.hooks.len())
                    .unwrap_or(0),
                blocking_reasons: vec![if source.len() > auto_convert::MAX_CONVERT_PHP_BYTES {
                    "Skipped: file is over the 1 MB per-file conversion cap.".to_string()
                } else {
                    format!(
                        "Skipped: only the first {} PHP files are converted per upload.",
                        auto_convert::MAX_CONVERT_PHP_FILES
                    )
                }],
                warnings: Vec::new(),
            });
            continue;
        }
        converted_count += 1;
        reports.push(analyze_php_file(path, source));
    }
    let rank = |v: FileVerdict| match v {
        FileVerdict::Blocked => 0,
        FileVerdict::Partial => 1,
        FileVerdict::Converted => 2,
    };
    reports.sort_by(|a, b| {
        rank(a.verdict)
            .cmp(&rank(b.verdict))
            .then_with(|| a.path.cmp(&b.path))
    });
    let converted = reports
        .iter()
        .filter(|r| r.verdict == FileVerdict::Converted)
        .count();
    let partial = reports
        .iter()
        .filter(|r| r.verdict == FileVerdict::Partial)
        .count();
    let blocked = reports
        .iter()
        .filter(|r| r.verdict == FileVerdict::Blocked)
        .count();
    PluginReport {
        files: reports,
        converted,
        partial,
        blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// In-repo vendored fixtures (CI / clean checkout): `tests/fixtures/`
    /// at the workspace root. Maps the legacy local paths (a sibling
    /// WordPress copy / v1 repo under ~/workspace/wordless) to the
    /// vendored copies, so these tests actually run in CI instead of
    /// silently returning early when the files are absent.
    fn vendored_path(rel: &str) -> Option<PathBuf> {
        let name = if rel.ends_with("wp-content/plugins/hello.php") {
            "wordpress/hello.php"
        } else if rel.ends_with("twentytwentyfive/functions.php") {
            "wordpress/twentytwentyfive/functions.php"
        } else if rel.ends_with("examples/hello-dolly/index.ts") {
            "wordless/hello-dolly-index.ts"
        } else if rel.ends_with("wp-content/themes/twentytwentyfive") {
            "wordpress/twentytwentyfive"
        } else {
            return None;
        };
        Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures")
                .join(name),
        )
    }

    fn find_file(rel: &str) -> Option<PathBuf> {
        let mut candidates = vec![PathBuf::from(rel)];
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(PathBuf::from(home).join("workspace/wordless").join(rel));
        }
        if let Some(v) = vendored_path(rel) {
            candidates.push(v);
        }
        candidates.into_iter().find(|p| p.is_file())
    }

    fn find_dir(rel: &str) -> Option<PathBuf> {
        let mut candidates = vec![PathBuf::from(rel)];
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(PathBuf::from(home).join("workspace/wordless").join(rel));
        }
        if let Some(v) = vendored_path(rel) {
            candidates.push(v);
        }
        candidates.into_iter().find(|p| p.is_dir())
    }

    fn collect_php_files(dir: &std::path::Path) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|e| e == "php").unwrap_or(false) {
                    let rel = p
                        .strip_prefix(dir)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/");
                    if let Ok(src) = std::fs::read_to_string(&p) {
                        out.push((rel, src));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn hello_dolly_contract_report() {
        let Some(path) = find_file("reference/wordpress-7.1.5/wp-content/plugins/hello.php") else {
            return;
        };
        let source = std::fs::read_to_string(path).expect("hello.php readable");
        let report = analyze_php_file("hello.php", &source);
        assert_eq!(report.verdict, FileVerdict::Converted);
        assert_eq!(report.functions_converted, 3);
        assert_eq!(report.functions_total, 3);
        assert_eq!(report.hooks_converted, 2);
        assert_eq!(report.hooks_total, 2);
        assert!(
            report.blocking_reasons.is_empty(),
            "{:?}",
            report.blocking_reasons
        );
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        let converted = convert_php_file("hello.php", &source);
        assert_eq!(converted.report, report);
        assert!(
            converted
                .typescript
                .contains("export function helloDollyGetLyric"),
            "{}",
            converted.typescript
        );
        assert!(
            converted
                .typescript
                .contains("wp.add_action(\"admin_notices\""),
            "{}",
            converted.typescript
        );
    }

    #[test]
    fn blocked_file_contract_report() {
        let report = analyze_php_file("bad.php", "<?php function f($x) { return eval($x); }");
        assert_eq!(report.verdict, FileVerdict::Blocked);
        assert_eq!(report.functions_converted, 0);
        assert_eq!(report.functions_total, 1);
        assert!(!report.blocking_reasons.is_empty());
        assert!(report.blocking_reasons[0].contains("eval"));

        let converted = convert_php_file("bad.php", "<?php function f($x) { return eval($x); }");
        assert!(converted.typescript.is_empty());
        assert_eq!(converted.report.verdict, FileVerdict::Blocked);
    }

    #[test]
    fn class_file_is_partial_not_blocked() {
        let report = analyze_php_file(
            "class.php",
            "<?php class Foo { public function bar() { return 1; } }",
        );
        assert_eq!(report.verdict, FileVerdict::Partial);
        assert!(!report.blocking_reasons.is_empty());
    }

    #[test]
    fn twentytwentyfive_matches_v1_conversion_record() {
        // v1 M11 record (measured): 23 converted / 76 partial / 0 blocked
        // of 99 PHP files.
        let Some(dir) = find_dir("reference/wordpress-7.1.5/wp-content/themes/twentytwentyfive")
        else {
            return;
        };
        let files = collect_php_files(&dir);
        assert_eq!(files.len(), 99, "TT25 should have 99 PHP files");
        let report = analyze_plugin(&files);
        assert_eq!(
            report.converted,
            23,
            "converted mismatch: {:?}",
            report
                .files
                .iter()
                .filter(|f| f.verdict == FileVerdict::Converted)
                .map(|f| &f.path)
                .collect::<Vec<_>>()
        );
        assert_eq!(report.partial, 76);
        assert_eq!(
            report.blocked,
            0,
            "blocked files: {:?}",
            report
                .files
                .iter()
                .filter(|f| f.verdict == FileVerdict::Blocked)
                .map(|f| (&f.path, &f.blocking_reasons))
                .collect::<Vec<_>>()
        );
        assert_eq!(report.files.len(), 99);
    }
}
