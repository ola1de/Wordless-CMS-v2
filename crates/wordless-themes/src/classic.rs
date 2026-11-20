//! Classic-theme analysis (P2, P3 analyser) — port of v1 `src/modules/themes/classic.ts`.
//!
//! A classic theme is a style.css header + PHP templates. Wordless cannot
//! execute PHP, so a classic theme is imported for its stylesheet plus a
//! per-file static analysis — never converted wholesale. This module
//! parses the style.css header, scans template files for the
//! WordPress template tags they use, and runs the PHP analyser for
//! the per-file verdicts — in the wordless-converter Worker since the
//! 2026-11-20 split (see `analyze_classic_theme`, which takes the
//! converter client).
//! The M11 auto-conversion record on the import report is set by the
//! import itself (see `registry::import_classic_theme`), not here.

use serde::{Deserialize, Serialize};
use wordless_conversion_jobs::{
    FileAnalysisJson, HookRefJson, OptionDefaultJson, UnconvertibleFeatureJson,
};
use wordless_kernel::converter::ConverterClient;
use wordless_kernel::WordlessError;

/// The parsed style.css header block (Theme Name, Version, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleCssHeader {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    #[serde(rename = "themeUri")]
    pub theme_uri: String,
    #[serde(rename = "authorUri")]
    pub author_uri: String,
    #[serde(rename = "textDomain")]
    pub text_domain: String,
    pub tags: Vec<String>,
    #[serde(rename = "requiresWordPress")]
    pub requires_wordpress: String,
    #[serde(rename = "requiresPhp")]
    pub requires_php: String,
    /// Raw header fields as parsed (key → value).
    pub raw: serde_json::Map<String, serde_json::Value>,
}

/// Parse the style.css header block (Theme Name, Version, …).
/// Returns `None` when there is no `/* … */` block or no `Theme Name:`.
pub fn parse_style_css_header(css: &str) -> Option<StyleCssHeader> {
    let start = css.find("/*")?;
    let end = css[start + 2..].find("*/")?;
    let block = &css[start + 2..start + 2 + end];
    let mut raw = serde_json::Map::new();
    for line in block.split('\n') {
        // v1: /^\s*([A-Za-z][A-Za-z ]*?):\s*(.*)\s*$/
        let trimmed = line.trim_start();
        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon].trim_end();
        if key.is_empty() || !key.starts_with(|c: char| c.is_ascii_alphabetic()) {
            continue;
        }
        if !key.chars().all(|c| c.is_ascii_alphabetic() || c == ' ') {
            continue;
        }
        let value = trimmed[colon + 1..].trim();
        raw.insert(
            key.trim().to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    let name = raw
        .get("Theme Name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return None;
    }
    let get = |key: &str| {
        raw.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let tags: Vec<String> = get("Tags")
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    Some(StyleCssHeader {
        name,
        version: {
            let v = get("Version");
            if v.is_empty() {
                "0.1.0".to_string()
            } else {
                v
            }
        },
        author: get("Author"),
        description: get("Description"),
        theme_uri: get("Theme URI"),
        author_uri: get("Author URI"),
        text_domain: get("Text Domain"),
        tags,
        requires_wordpress: get("Requires at least"),
        requires_php: get("Requires PHP"),
        raw,
    })
}

/// WordPress template tags whose presence says what a template does.
pub const TEMPLATE_TAGS: &[&str] = &[
    "get_header",
    "get_footer",
    "get_sidebar",
    "get_template_part",
    "have_posts",
    "the_post",
    "the_title",
    "the_content",
    "the_excerpt",
    "the_permalink",
    "the_date",
    "the_author",
    "the_category",
    "the_tags",
    "comments_template",
    "wp_head",
    "wp_footer",
    "wp_nav_menu",
    "get_search_form",
    "bloginfo",
    "wp_enqueue_style",
    "wp_enqueue_script",
    "add_action",
    "add_filter",
    "register_nav_menu",
    "add_theme_support",
];

/// Scan one PHP source for literal template-tag calls (static, no
/// execution). Matches v1's `\b<tag>\s*\(` scan.
pub fn find_template_tags(source: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for tag in TEMPLATE_TAGS {
        if has_tag_call(source, tag) {
            found.push(tag.to_string());
        }
    }
    found
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn has_tag_call(source: &str, tag: &str) -> bool {
    let bytes = source.as_bytes();
    let tag_bytes = tag.as_bytes();
    let mut i = 0;
    while i + tag_bytes.len() <= bytes.len() {
        if &bytes[i..i + tag_bytes.len()] == tag_bytes {
            let before_ok = i == 0 || !is_word_char(bytes[i - 1] as char);
            let mut j = i + tag_bytes.len();
            while j < bytes.len()
                && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n' || bytes[j] == b'\r')
            {
                j += 1;
            }
            if before_ok && j < bytes.len() && bytes[j] == b'(' {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// One PHP file's report — v1's `ClassicFileReport` shape: the
/// analyser's `FileAnalysis` spread flat (`...analysis`) plus the
/// template tags. (P5 F10: the P3 port nested the analysis under an
/// `analysis` key, which v1 does not have, so every flat field read
/// as missing in the parity harness.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassicFileReport {
    pub file: String,
    pub lines: usize,
    pub bytes: usize,
    pub tokens: usize,
    pub nodes: usize,
    pub functions: Vec<String>,
    pub classes: Vec<String>,
    pub hooks: Vec<HookRefJson>,
    #[serde(rename = "wpCalls")]
    pub wp_calls: Vec<String>,
    #[serde(rename = "optionKeys")]
    pub option_keys: Vec<String>,
    #[serde(rename = "optionDefaults")]
    pub option_defaults: Vec<OptionDefaultJson>,
    #[serde(rename = "httpCalls")]
    pub http_calls: Vec<String>,
    pub features: Vec<String>,
    pub unconvertible: Vec<UnconvertibleFeatureJson>,
    /// Template tags found in this file (literal scan, static only).
    #[serde(rename = "templateTags")]
    pub template_tags: Vec<String>,
    pub verdict: String,
    pub reasons: Vec<String>,
}

/// The classic-theme import report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassicReport {
    pub header: StyleCssHeader,
    /// style.css bytes (the imported stylesheet).
    #[serde(rename = "cssBytes")]
    pub css_bytes: usize,
    pub files: Vec<ClassicFileReport>,
    pub convertible: usize,
    pub partial: usize,
    pub blocked: usize,
    /// M11 auto-conversion record (set by the import, not the analyser).
    pub conversion: Option<serde_json::Value>,
    /// What the import did and did not do.
    pub notes: Vec<String>,
}

/// Analyse a classic theme's files. `files` maps relative paths
/// (style.css, index.php, …) to contents. Returns `None` when there is
/// no style.css or no usable header.
///
/// Split (2026-11-20): the PHP analysis runs in the wordless-converter
/// Worker — one `analyze_files` round trip for the whole theme, not
/// one call per file. A converter failure is an `Err` (the import
/// surfaces it); a file the parser cannot handle gets v1's fallback
/// `FileAnalysis` (verdict "partial", reason "could not be fully
/// parsed; needs manual review.") — explicit, never silent.
pub async fn analyze_classic_theme(
    files: &[(String, String)],
    converter: &dyn ConverterClient,
) -> Result<Option<ClassicReport>, WordlessError> {
    let css = match files.iter().find(|(p, _)| p == "style.css") {
        Some((_, css)) => css.clone(),
        None => return Ok(None),
    };
    let Some(header) = parse_style_css_header(&css) else {
        return Ok(None);
    };
    let mut paths: Vec<&(String, String)> = files.iter().collect();
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    let php_files: Vec<(String, String)> = paths
        .iter()
        .filter(|(p, _)| p.to_lowercase().ends_with(".php"))
        .map(|(p, s)| ((*p).clone(), (*s).clone()))
        .collect();
    let analyses = converter
        .analyze_files(&php_files)
        .await
        .map_err(|e| e.to_wordless())?;
    let mut by_path: std::collections::HashMap<String, (Option<FileAnalysisJson>, Option<String>)> =
        std::collections::HashMap::new();
    for item in analyses.as_array().cloned().unwrap_or_default() {
        let Some(path) = item
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            continue;
        };
        let analysis = match item.get("analysis") {
            Some(v) if !v.is_null() => serde_json::from_value::<FileAnalysisJson>(v.clone()).ok(),
            _ => None,
        };
        let error = item
            .get("error")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        by_path.insert(path, (analysis, error));
    }
    let mut reports: Vec<ClassicFileReport> = Vec::new();
    for (path, source) in paths {
        if !path.to_lowercase().ends_with(".php") {
            continue;
        }
        // P3: real verdicts from the PHP analyser (v1 `analyzePhp`).
        // A file the parser cannot handle gets v1's fallback
        // FileAnalysis (verdict "partial", reason "could not be fully
        // parsed; needs manual review.") — explicit, never silent.
        let analysis = match by_path.get(path.as_str()).and_then(|(a, _)| a.as_ref()) {
            Some(a) => a.clone(),
            None => {
                let reason: String = by_path
                    .get(path.as_str())
                    .and_then(|(_, e)| e.clone())
                    .map(|m| m.chars().take(200).collect())
                    .unwrap_or_else(|| {
                        "could not be fully parsed; needs manual review.".to_string()
                    });
                FileAnalysisJson {
                    file: (*path).clone(),
                    lines: source.split('\n').count(),
                    bytes: source.len(),
                    tokens: 0,
                    nodes: 0,
                    functions: Vec::new(),
                    classes: Vec::new(),
                    hooks: Vec::new(),
                    wp_calls: Vec::new(),
                    option_keys: Vec::new(),
                    option_defaults: Vec::new(),
                    http_calls: Vec::new(),
                    features: Vec::new(),
                    unconvertible: vec![UnconvertibleFeatureJson {
                        kind: "parse-error".to_string(),
                        reason,
                        line: 1,
                    }],
                    verdict: "partial".to_string(),
                    reasons: vec!["could not be fully parsed; needs manual review.".to_string()],
                }
            }
        };
        let verdict = match analysis.verdict.as_str() {
            "convertible" => "convertible",
            "blocked" => "blocked",
            _ => "partial",
        }
        .to_string();
        reports.push(ClassicFileReport {
            file: analysis.file.clone(),
            lines: analysis.lines,
            bytes: analysis.bytes,
            tokens: analysis.tokens,
            nodes: analysis.nodes,
            functions: analysis.functions.clone(),
            classes: analysis.classes.clone(),
            hooks: analysis.hooks.clone(),
            wp_calls: analysis.wp_calls.clone(),
            option_keys: analysis.option_keys.clone(),
            option_defaults: analysis.option_defaults.clone(),
            http_calls: analysis.http_calls.clone(),
            features: analysis.features.clone(),
            unconvertible: analysis.unconvertible.clone(),
            template_tags: find_template_tags(source),
            verdict,
            reasons: analysis.reasons.clone(),
        });
    }
    let convertible = reports
        .iter()
        .filter(|r| r.verdict == "convertible")
        .count();
    let partial = reports.iter().filter(|r| r.verdict == "partial").count();
    let blocked = reports.iter().filter(|r| r.verdict == "blocked").count();
    Ok(Some(ClassicReport {
        header,
        css_bytes: css.len(),
        files: reports,
        convertible,
        partial,
        blocked,
        conversion: None,
        notes: vec![
            "Classic themes are imported for their stylesheet and analysis only: PHP templates are never executed.".to_string(),
            "The style.css is served as the theme stylesheet; templates are not converted to block templates in M5.".to_string(),
            "Template tags used by the PHP files are listed per file — they show what a future block-template conversion would need to reproduce.".to_string(),
        ],
    }))
}

/// Render the classic report as markdown (docs + admin display).
pub fn render_classic_report(report: &ClassicReport) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "# Classic theme — {} (import report)",
        report.header.name
    ));
    lines.push(String::new());
    lines.push(format!(
        "Version {}{}.",
        report.header.version,
        if report.header.author.is_empty() {
            String::new()
        } else {
            format!(" by {}", report.header.author)
        }
    ));
    if !report.header.description.is_empty() {
        lines.push(report.header.description.clone());
    }
    lines.push(String::new());
    lines.push(format!(
        "- Stylesheet imported: **{} bytes** of style.css.",
        report.css_bytes
    ));
    lines.push(format!(
        "- PHP files analysed: **{}** — convertible **{}** · partial **{}** · blocked **{}**.",
        report.files.len(),
        report.convertible,
        report.partial,
        report.blocked
    ));
    lines.push(String::new());
    for note in &report.notes {
        lines.push(format!("- {note}"));
    }
    lines.push(String::new());
    lines.push("| File | Lines | Verdict | Template tags | Why |".to_string());
    lines.push("| --- | ---: | --- | --- | --- |".to_string());
    for f in &report.files {
        lines.push(format!(
            "| `{}` | {} | {} | {} | {} |",
            f.file,
            f.lines,
            f.verdict,
            if f.template_tags.is_empty() {
                "—".to_string()
            } else {
                f.template_tags.join(", ")
            },
            f.reasons.join(" ")
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_converter::TestConverter;
    use futures_executor::block_on;

    const STYLE_CSS: &str = "/*\nTheme Name: Classic Sample\nTheme URI: https://example.test/classic-sample\nAuthor: Wordless Test\nDescription: A small synthetic classic theme written for the Wordless M5 test suite. Original code, not a copy of any WordPress theme.\nVersion: 1.0.0\nLicense: MIT\nText Domain: classic-sample\nTags: blog, classic\n*/\nbody { background: #fff; color: #222; }\n.site-header { border-bottom: 1px solid #ddd; }\n";

    const INDEX_PHP: &str = "<?php\nget_header();\nif ( have_posts() ) {\n\twhile ( have_posts() ) {\n\t\tthe_post();\n\t\tthe_title( '<h2>', '</h2>' );\n\t\tthe_content();\n\t}\n}\nget_footer();\n";

    const SINGLE_PHP: &str = "<?php\nget_header();\nthe_post();\nthe_title( '<h1>', '</h1>' );\nthe_content();\ncomments_template();\nget_footer();\n";

    const FUNCTIONS_PHP: &str = "<?php\nfunction classic_sample_setup() {\n\tadd_theme_support( 'title-tag' );\n\tregister_nav_menu( 'primary', 'Primary Menu' );\n}\nadd_action( 'after_setup_theme', 'classic_sample_setup' );\n\nfunction classic_sample_assets() {\n\twp_enqueue_style( 'classic-sample-style', get_stylesheet_uri() );\n}\nadd_action( 'wp_enqueue_scripts', 'classic_sample_assets' );\n";

    fn classic_sample_files() -> Vec<(String, String)> {
        vec![
            ("style.css".to_string(), STYLE_CSS.to_string()),
            ("index.php".to_string(), INDEX_PHP.to_string()),
            ("single.php".to_string(), SINGLE_PHP.to_string()),
            ("functions.php".to_string(), FUNCTIONS_PHP.to_string()),
        ]
    }

    #[test]
    fn parses_style_css_header() {
        let header = parse_style_css_header(STYLE_CSS).unwrap();
        assert_eq!(header.name, "Classic Sample");
        assert_eq!(header.version, "1.0.0");
        assert_eq!(header.text_domain, "classic-sample");
        assert_eq!(header.tags, vec!["blog", "classic"]);
    }

    #[test]
    fn header_requires_theme_name() {
        assert!(parse_style_css_header("body { color: red; }").is_none());
        assert!(parse_style_css_header("/*\nVersion: 1.0\n*/").is_none());
    }

    #[test]
    fn finds_template_tags_in_index() {
        let tags = find_template_tags(INDEX_PHP);
        for tag in [
            "get_header",
            "have_posts",
            "the_post",
            "the_title",
            "the_content",
            "get_footer",
        ] {
            assert!(tags.contains(&tag.to_string()), "{tags:?}");
        }
        assert!(!tags.contains(&"get_sidebar".to_string()));
    }

    #[test]
    fn analyses_classic_fixture_tags_and_hooks() {
        let report = block_on(analyze_classic_theme(
            &classic_sample_files(),
            &TestConverter,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(report.files.len(), 3);
        let index = report.files.iter().find(|f| f.file == "index.php").unwrap();
        assert!(index.template_tags.contains(&"get_header".to_string()));
        assert!(index.template_tags.contains(&"have_posts".to_string()));
        assert!(index.template_tags.contains(&"the_content".to_string()));
        let functions = report
            .files
            .iter()
            .find(|f| f.file == "functions.php")
            .unwrap();
        // Hook names (after_setup_theme, wp_enqueue_scripts) are analyser
        // data in v1 — P3 here; the literal tag scan finds the calls.
        assert!(functions.template_tags.contains(&"add_action".to_string()));
        assert!(functions
            .template_tags
            .contains(&"wp_enqueue_style".to_string()));
        assert!(functions
            .template_tags
            .contains(&"add_theme_support".to_string()));
        assert!(functions
            .template_tags
            .contains(&"register_nav_menu".to_string()));
        // P3: real analyser verdicts — the fixture's three files are
        // all plain procedural template code, so all convert.
        assert_eq!(report.convertible, 3);
        assert_eq!(report.partial, 0);
        assert_eq!(report.blocked, 0);
        assert!(report.files.iter().all(|f| f.verdict == "convertible"));
        assert!(report.files.iter().all(|f| f.tokens > 0 && f.nodes > 0));
        assert!(report.conversion.is_none());
    }

    #[test]
    fn renders_markdown_report() {
        let report = block_on(analyze_classic_theme(
            &classic_sample_files(),
            &TestConverter,
        ))
        .unwrap()
        .unwrap();
        let md = render_classic_report(&report);
        assert!(
            md.contains("# Classic theme — Classic Sample (import report)"),
            "{md}"
        );
        assert!(md.contains("Version 1.0.0 by Wordless Test."), "{md}");
        assert!(md.contains("| `index.php` |"), "{md}");
    }
}
