//! Conversion pipeline — a port of v1 `src/conversion/index.ts`.
//!
//! One function, [`convert_php`], runs the whole pipeline for a single
//! PHP file: safety scan → lex → parse → emit. It never executes the
//! source. The result is a [`ConversionReport`] (counts, converted /
//! unconverted items, warnings, the generated TypeScript) that the jobs
//! module stores in `wl_jobs.result`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ast::count_ast_nodes;
use crate::emitter::{
    emit_typescript, EmitOptions, HookRegistration, Transformed, UnconvertedItem,
};
use crate::lexer::LexComment;
use crate::parser::parse_php;
use crate::safety::{scan_safety, SafetyViolation};

/// The pipeline failed because the sandbox blocked the source.
#[derive(Debug, Clone)]
pub struct ConversionBlockedError {
    pub violations: Vec<SafetyViolation>,
}

impl std::fmt::Display for ConversionBlockedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.violations.len();
        let summary = self
            .violations
            .iter()
            .map(|v| format!("line {}: {}", v.line, v.construct))
            .collect::<Vec<_>>()
            .join("; ");
        write!(
            f,
            "Conversion blocked by the sandbox ({} violation{}): {}",
            n,
            if n == 1 { "" } else { "s" },
            summary
        )
    }
}

impl std::error::Error for ConversionBlockedError {}

/// The pipeline failed because the source could not be parsed.
#[derive(Debug, Clone, Error)]
#[error("{0}")]
pub struct ConversionParseError(pub String);

/// The pipeline's failure modes.
#[derive(Debug, Clone, Error)]
pub enum ConversionError {
    #[error(transparent)]
    Blocked(#[from] ConversionBlockedError),
    #[error(transparent)]
    Parse(#[from] ConversionParseError),
}

/// The conversion report for one file (v1 `ConversionReport`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionReport {
    pub source: ReportSource,
    #[serde(rename = "filesProcessed")]
    pub files_processed: usize,
    pub ast: ReportAst,
    pub converted: ReportConverted,
    pub unconverted: Vec<UnconvertedItem>,
    pub warnings: Vec<String>,
    pub transformed: Vec<Transformed>,
    pub output: ReportOutput,
    pub extension: ReportExtension,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportSource {
    pub filename: String,
    pub bytes: usize,
    pub lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportAst {
    pub nodes: usize,
    pub functions: usize,
    pub classes: usize,
    pub hooks: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportConverted {
    pub functions: Vec<String>,
    pub hooks: Vec<ConvertedHook>,
}

/// A converted hook, as stored in the report (no priority — v1 shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertedHook {
    pub tag: String,
    pub kind: String,
    #[serde(rename = "functionName")]
    pub function_name: String,
}

impl From<&HookRegistration> for ConvertedHook {
    fn from(h: &HookRegistration) -> Self {
        ConvertedHook {
            tag: h.tag.clone(),
            kind: h.kind.clone(),
            function_name: h.function_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportOutput {
    pub filename: String,
    pub bytes: usize,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportExtension {
    pub slug: String,
    pub name: String,
    pub version: String,
}

/// Options for [`convert_php`] (v1 `ConvertOptions`).
#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub slug: String,
    pub compat_import: Option<String>,
    pub source_label: Option<String>,
}

/// Plugin header fields recognised by WordPress (same allowlist as v1
/// `parsePluginHeader`).
const HEADER_KEYS: &[&str] = &[
    "Plugin Name",
    "Plugin URI",
    "Description",
    "Author",
    "Author URI",
    "Version",
    "Text Domain",
];

/// Extract WordPress plugin header fields from the first comment block
/// that names a plugin. Hand-rolled line matcher for the TS regex
/// `^\s*\*?\s*([A-Za-z ]+):\s*(.+?)\s*$`.
pub fn parse_plugin_header(comments: &[LexComment]) -> HashMap<String, String> {
    let mut fields: HashMap<String, String> = HashMap::new();
    for comment in comments {
        if !comment.text.contains("Plugin Name") {
            continue;
        }
        for line in comment.text.split('\n') {
            let mut s = line.trim_start();
            if let Some(rest) = s.strip_prefix('*') {
                s = rest.trim_start();
            }
            let Some(colon) = s.find(':') else {
                continue;
            };
            let key = s[..colon].trim();
            let value = s[colon + 1..].trim();
            if key.is_empty()
                || value.is_empty()
                || !key.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
            {
                continue;
            }
            if HEADER_KEYS.contains(&key) {
                fields.insert(key.to_string(), value.to_string());
            }
        }
        if !fields.is_empty() {
            break;
        }
    }
    fields
}

/// Convert one PHP source file to a Wordless extension module.
/// Returns [`ConversionError::Blocked`] (sandbox) or
/// [`ConversionError::Parse`] (syntax) on failure.
pub fn convert_php(
    source: &str,
    filename: &str,
    options: &ConvertOptions,
) -> Result<ConversionReport, ConversionError> {
    let started = wordless_kernel::time::now_ms();
    let lines = source.split('\n').count();

    // 1. Sandbox safety scan — blocked constructs fail the job, fast.
    let safety = scan_safety(source);
    if !safety.safe {
        return Err(ConversionBlockedError {
            violations: safety.violations,
        }
        .into());
    }

    // 2. Parse.
    let parsed = parse_php(source)
        .map_err(|e| ConversionParseError(format!("Could not parse {filename}: {e}")))?;
    let program = parsed.program;

    // 3. Emit.
    let header = parse_plugin_header(&parsed.comments);
    let emit_options = EmitOptions {
        filename: filename.to_string(),
        slug: options.slug.clone(),
        compat_import: options.compat_import.clone(),
        plugin_header: header.clone(),
        source_label: options
            .source_label
            .clone()
            .or_else(|| Some(filename.to_string())),
    };
    let emitted = emit_typescript(&program, &emit_options);

    // 4. Report.
    let mut function_count = 0usize;
    let mut class_count = 0usize;
    {
        use crate::ast::{Node, Stmt};
        crate::ast::walk_program(&program, &mut |node| match node {
            Node::Stmt(Stmt::Function { .. }) => function_count += 1,
            Node::Stmt(Stmt::Class { .. }) => class_count += 1,
            _ => {}
        });
    }

    Ok(ConversionReport {
        source: ReportSource {
            filename: filename.to_string(),
            bytes: source.len(),
            lines,
        },
        files_processed: 1,
        ast: ReportAst {
            nodes: count_ast_nodes(&program),
            functions: function_count,
            classes: class_count,
            hooks: emitted.report.hooks.len(),
        },
        converted: ReportConverted {
            functions: emitted.report.functions.clone(),
            hooks: emitted
                .report
                .hooks
                .iter()
                .map(ConvertedHook::from)
                .collect(),
        },
        unconverted: emitted.report.unconverted.clone(),
        warnings: emitted.report.warnings.clone(),
        transformed: emitted.report.transformed.clone(),
        output: ReportOutput {
            filename: format!("{}.ts", options.slug),
            bytes: emitted.code.len(),
            code: emitted.code,
        },
        extension: ReportExtension {
            slug: options.slug.clone(),
            name: header
                .get("Plugin Name")
                .cloned()
                .unwrap_or_else(|| options.slug.clone()),
            version: header
                .get("Version")
                .cloned()
                .unwrap_or_else(|| "0.1.0".to_string()),
        },
        duration_ms: wordless_kernel::time::now_ms().saturating_sub(started),
    })
}

/// Slug for a conversion output file, mirroring v1's `slugifyFilename`.
pub fn slugify_filename(filename: &str) -> String {
    // Strip a trailing ".php" (case-insensitive, as in the TS regex).
    // The suffix is ASCII when it matches, so the cut is a char boundary.
    let suffix = filename
        .get(filename.len().saturating_sub(4)..)
        .unwrap_or("");
    let base = if suffix.eq_ignore_ascii_case(".php") {
        filename.get(..filename.len() - 4).unwrap_or(filename)
    } else {
        filename
    };
    let base = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let base = base.trim_matches('-').to_string();
    // Collapse runs of '-' (the TS regex replaces each run of
    // non-alphanumerics with a single '-'; the char map above emits one
    // '-' per character, so collapse here).
    let mut collapsed = String::with_capacity(base.len());
    let mut last_dash = false;
    for c in base.chars() {
        if c == '-' {
            if last_dash {
                continue;
            }
            last_dash = true;
        } else {
            last_dash = false;
        }
        collapsed.push(c);
    }
    let collapsed = collapsed.trim_matches('-').to_string();
    if collapsed.is_empty() {
        "converted-plugin".to_string()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Candidate locations for the reference WordPress copy / v1 repo,
    /// relative to the crate dir first (cargo test runs with cwd at the
    /// package root), then absolute from $HOME.
    fn candidate_paths(rel: &str) -> Vec<PathBuf> {
        let mut out = vec![PathBuf::from(rel)];
        if let Ok(home) = std::env::var("HOME") {
            out.push(PathBuf::from(home).join("workspace/wordless").join(rel));
        }
        out
    }

    pub fn find_file(rel: &str) -> Option<PathBuf> {
        candidate_paths(rel).into_iter().find(|p| p.is_file())
    }

    #[test]
    fn parses_plugin_header_fields() {
        let comments = vec![LexComment {
            text: "/*\nPlugin Name: Hello Dolly\nPlugin URI: https://example.com/\nDescription: A song.\nAuthor: Matt Mullenweg\nVersion: 1.7.2\nText Domain: hello-dolly\n*/"
                .to_string(),
            pos: crate::ast::Pos::new(1, 1, 0),
        }];
        let header = parse_plugin_header(&comments);
        assert_eq!(
            header.get("Plugin Name").map(String::as_str),
            Some("Hello Dolly")
        );
        assert_eq!(header.get("Version").map(String::as_str), Some("1.7.2"));
        assert_eq!(
            header.get("Author").map(String::as_str),
            Some("Matt Mullenweg")
        );
        assert_eq!(
            header.get("Text Domain").map(String::as_str),
            Some("hello-dolly")
        );
        // Unknown keys are not collected.
        let comments = vec![LexComment {
            text: "/*\nPlugin Name: X\nRequires PHP: 8.0\n*/".to_string(),
            pos: crate::ast::Pos::new(1, 1, 0),
        }];
        let header = parse_plugin_header(&comments);
        assert_eq!(header.len(), 1);
    }

    #[test]
    fn slugifies_filenames_like_v1() {
        assert_eq!(slugify_filename("hello.php"), "hello");
        assert_eq!(slugify_filename("My Plugin.PHP"), "my-plugin");
        assert_eq!(slugify_filename("hello-dolly.php"), "hello-dolly");
        assert_eq!(slugify_filename(".php"), "converted-plugin");
    }

    #[test]
    fn converts_hello_dolly_end_to_end() {
        let Some(path) = find_file("reference/wordpress-7.1.5/wp-content/plugins/hello.php") else {
            return;
        };
        let source = std::fs::read_to_string(path).expect("hello.php readable");
        let report = convert_php(
            &source,
            "hello.php",
            &ConvertOptions {
                slug: "hello-dolly".to_string(),
                compat_import: None,
                source_label: None,
            },
        )
        .expect("hello dolly converts");
        assert_eq!(report.ast.functions, 3);
        assert_eq!(
            report.converted.functions,
            vec![
                "hello_dolly_get_lyric".to_string(),
                "hello_dolly".to_string(),
                "dolly_css".to_string()
            ]
        );
        assert_eq!(
            report.converted.hooks,
            vec![
                ConvertedHook {
                    tag: "admin_notices".to_string(),
                    kind: "action".to_string(),
                    function_name: "hello_dolly".to_string(),
                },
                ConvertedHook {
                    tag: "admin_head".to_string(),
                    kind: "action".to_string(),
                    function_name: "dolly_css".to_string(),
                },
            ]
        );
        assert!(report.unconverted.is_empty(), "{:?}", report.unconverted);
        assert!(report.transformed.iter().any(|t| t.what == "ABSPATH guard"));
        assert!(
            report
                .output
                .code
                .contains("export function helloDollyGetLyric"),
            "{}",
            report.output.code
        );
        assert!(
            report
                .output
                .code
                .contains("wp.add_action(\"admin_notices\""),
            "{}",
            report.output.code
        );
        assert!(
            report
                .output
                .code
                .contains("renderedOutput.push(helloDolly(wp))"),
            "{}",
            report.output.code
        );
        assert!(
            report.output.code.contains("wp.wptexturize"),
            "{}",
            report.output.code
        );
        assert!(
            report.output.code.contains("wp.get_user_locale()"),
            "{}",
            report.output.code
        );
        assert!(
            report.output.code.contains("__sprintf"),
            "{}",
            report.output.code
        );
        assert_eq!(report.extension.name, "Hello Dolly");
        assert_eq!(report.extension.version, "1.7.2");
    }

    #[test]
    fn hello_dolly_output_matches_checked_in_fixture_byte_for_byte() {
        let Some(hello) = find_file("reference/wordpress-7.1.5/wp-content/plugins/hello.php")
        else {
            return;
        };
        let Some(fixture) = find_file("wordless-cms/examples/hello-dolly/index.ts") else {
            return;
        };
        let source = std::fs::read_to_string(hello).expect("hello.php readable");
        let report = convert_php(
            &source,
            "hello.php",
            &ConvertOptions {
                slug: "hello-dolly".to_string(),
                compat_import: None,
                source_label: None,
            },
        )
        .expect("hello dolly converts");
        let checked_in = std::fs::read_to_string(fixture).expect("fixture readable");
        assert_eq!(checked_in, report.output.code);
    }

    #[test]
    fn blocked_source_fails_with_sandbox_error() {
        let err = convert_php(
            "<?php function f($x) { return eval($x); }",
            "bad.php",
            &ConvertOptions {
                slug: "bad".to_string(),
                compat_import: None,
                source_label: None,
            },
        )
        .expect_err("eval is blocked");
        match err {
            ConversionError::Blocked(e) => {
                assert!(e.violations.iter().any(|v| v.construct.contains("eval")));
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn unparseable_source_fails_with_parse_error() {
        let err = convert_php(
            "<?php $x = fn($a) => $a * 2;",
            "bad.php",
            &ConvertOptions {
                slug: "bad".to_string(),
                compat_import: None,
                source_label: None,
            },
        )
        .expect_err("arrow functions are parse errors");
        assert!(matches!(err, ConversionError::Parse(_)));
    }

    /// P6: the `if (!function_exists('x')) : function x() ... endif;`
    /// guard every WordPress theme uses. The guarded function must be
    /// hoisted to module level (PHP defines it at compile time), its
    /// add_action must register, and function_exists() must resolve
    /// statically instead of throwing at import time.
    #[test]
    fn guarded_functions_hoist_and_register_hooks() {
        let src = r#"<?php
if (!function_exists('my_setup')) :
    function my_setup() {
        add_theme_support('post-formats');
    }
endif;
add_action('after_setup_theme', 'my_setup');
if (function_exists('no_such_function')) {
    echo 'never';
}
"#;
        let report = convert_php(
            src,
            "functions.php",
            &ConvertOptions {
                slug: "guarded".to_string(),
                compat_import: None,
                source_label: None,
            },
        )
        .expect("guarded functions convert");
        assert_eq!(report.converted.functions, vec!["my_setup".to_string()]);
        assert_eq!(
            report.converted.hooks,
            vec![ConvertedHook {
                tag: "after_setup_theme".to_string(),
                kind: "action".to_string(),
                function_name: "my_setup".to_string(),
            }]
        );
        assert!(
            !report.output.code.contains("__unmapped_function_exists"),
            "function_exists must resolve statically:\n{}",
            report.output.code
        );
        assert!(
            report.output.code.contains("export function mySetup"),
            "guarded function hoisted to module level:\n{}",
            report.output.code
        );
        assert!(
            report
                .output
                .code
                .contains("wp.add_action(\"after_setup_theme\""),
            "hook registered in activate():\n{}",
            report.output.code
        );
        // Same-file guard resolves true; unknown name resolves false.
        assert!(
            report.output.code.contains("if ((!true))"),
            "guard evaluates to false (function exists):\n{}",
            report.output.code
        );
        assert!(
            report.output.code.contains("if (false)"),
            "unknown function_exists resolves false:\n{}",
            report.output.code
        );
    }

    /// P6: the real Twenty Twenty-Five functions.php — seven guarded
    /// functions, six hook registrations. Before the P6 converter fix
    /// this produced an empty activate() and a worker that threw
    /// __unmapped_function_exists at import.
    #[test]
    fn converts_tt25_functions_php() {
        let Some(path) =
            find_file("reference/wordpress-7.1.5/wp-content/themes/twentytwentyfive/functions.php")
        else {
            return;
        };
        let source = std::fs::read_to_string(path).expect("tt25 functions.php readable");
        let report = convert_php(
            &source,
            "functions.php",
            &ConvertOptions {
                slug: "twentytwentyfive".to_string(),
                compat_import: None,
                source_label: None,
            },
        )
        .expect("tt25 functions.php converts");
        assert_eq!(
            report.converted.functions,
            vec![
                "twentytwentyfive_post_format_setup".to_string(),
                "twentytwentyfive_editor_style".to_string(),
                "twentytwentyfive_enqueue_styles".to_string(),
                "twentytwentyfive_block_styles".to_string(),
                "twentytwentyfive_pattern_categories".to_string(),
                "twentytwentyfive_register_block_bindings".to_string(),
                "twentytwentyfive_format_binding".to_string(),
            ]
        );
        assert_eq!(report.converted.hooks.len(), 6);
        assert!(
            report
                .converted
                .hooks
                .iter()
                .any(|h| h.tag == "after_setup_theme"
                    && h.function_name == "twentytwentyfive_post_format_setup"),
            "hooks: {:?}",
            report.converted.hooks
        );
        assert!(
            !report.output.code.contains("__unmapped_function_exists"),
            "function_exists must resolve statically:\n{}",
            report.output.code
        );
        assert!(
            report.output.code.contains("wp.add_action(\"init\""),
            "init hooks registered:\n{}",
            report.output.code
        );
    }
}
