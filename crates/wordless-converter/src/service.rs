//! Converter service functions: pure JSON-in / JSON-out over the
//! conversion engine and the Elementor renderer. The `#[event(fetch)]`
//! wrapper in `lib.rs` only parses the request and serialises the
//! answer, so these functions are unit-testable on native targets —
//! including the P3 conversion proofs (Hello Dolly 3/3, TT25 23/76/0),
//! which now live here instead of in the main Worker.

use serde_json::{json, Value};
use wordless_conversion::{
    analyze_php, collect_option_defaults, convert_php as engine_convert_php, convert_php_files,
    summarize_theme_json_settings, ConversionError, ConvertOptions,
};
use wordless_elementor::render_elementor_data;

fn err(status: u16, message: impl Into<String>) -> (u16, Value) {
    (status, json!({ "ok": false, "error": message.into() }))
}

fn files_of(body: &Value) -> Option<Vec<(String, String)>> {
    let arr = body.get("files")?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let path = item.get("path")?.as_str()?.to_string();
        let source = item.get("source")?.as_str()?.to_string();
        out.push((path, source));
    }
    Some(out)
}

/// `GET /__convert/health`.
pub fn health() -> Value {
    json!({ "ok": true, "service": "wordless-converter", "version": env!("CARGO_PKG_VERSION") })
}

/// `POST /__convert/convert-php` — one file through the full pipeline.
pub fn convert_php(body: &Value) -> (u16, Value) {
    let Some(filename) = body.get("filename").and_then(|v| v.as_str()) else {
        return err(400, "convert-php needs a \"filename\" string.");
    };
    let Some(source) = body.get("source").and_then(|v| v.as_str()) else {
        return err(400, "convert-php needs a \"source\" string.");
    };
    let Some(slug) = body.get("slug").and_then(|v| v.as_str()) else {
        return err(400, "convert-php needs a \"slug\" string.");
    };
    match engine_convert_php(
        source,
        filename,
        &ConvertOptions {
            slug: slug.to_string(),
            compat_import: None,
            source_label: None,
        },
    ) {
        Ok(report) => (200, json!({ "ok": true, "report": report })),
        Err(ConversionError::Blocked(e)) => (
            200,
            json!({
                "ok": false,
                "error": e.to_string(),
                "blocked": e.violations.iter().map(|v| json!({
                    "construct": v.construct,
                    "reason": v.reason,
                    "line": v.line,
                })).collect::<Vec<_>>(),
            }),
        ),
        Err(ConversionError::Parse(e)) => (200, json!({ "ok": false, "error": e.to_string() })),
    }
}

/// `POST /__convert/analyze-files` — static analysis for a batch.
pub fn analyze_files(body: &Value) -> (u16, Value) {
    let Some(files) = files_of(body) else {
        return err(
            400,
            "analyze-files needs a \"files\" array of { path, source }.",
        );
    };
    let analyses: Vec<Value> = files
        .iter()
        .map(|(path, source)| match analyze_php(source, path) {
            Ok(analysis) => json!({ "path": path, "analysis": analysis, "error": null }),
            Err(e) => json!({ "path": path, "analysis": null, "error": e.message }),
        })
        .collect();
    (200, json!({ "ok": true, "analyses": analyses }))
}

/// `POST /__convert/convert-files` — a whole upload through
/// auto-convert, plus the literal option defaults found.
pub fn convert_files(body: &Value) -> (u16, Value) {
    let Some(files) = files_of(body) else {
        return err(
            400,
            "convert-files needs a \"files\" array of { path, source }.",
        );
    };
    let Some(slug) = body.get("slug").and_then(|v| v.as_str()) else {
        return err(400, "convert-files needs a \"slug\" string.");
    };
    let Some(kind) = body.get("kind").and_then(|v| v.as_str()) else {
        return err(
            400,
            "convert-files needs a \"kind\" string (\"plugin\" or \"theme\").",
        );
    };
    let (generated, report) = convert_php_files(&files, slug, kind);
    let defaults = collect_option_defaults(&files);
    (
        200,
        json!({ "ok": true, "generated": generated, "report": report, "defaults": defaults }),
    )
}

/// `POST /__convert/theme-settings` — the theme.json settings summary.
pub fn theme_settings(body: &Value) -> (u16, Value) {
    let Some(theme_json) = body.get("themeJson") else {
        return err(400, "theme-settings needs a \"themeJson\" object.");
    };
    let unsupported: Vec<String> = body
        .get("unsupported")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let settings = summarize_theme_json_settings(theme_json, &unsupported);
    (200, json!({ "ok": true, "settings": settings }))
}

/// `POST /__convert/render-elementor` — Elementor data JSON to HTML.
pub fn render_elementor(body: &Value) -> (u16, Value) {
    let Some(data) = body.get("data") else {
        return err(400, "render-elementor needs a \"data\" value.");
    };
    let html = render_elementor_data(data);
    (200, json!({ "ok": true, "html": html }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn find_file(rel: &str) -> Option<PathBuf> {
        let mut candidates = vec![PathBuf::from(rel)];
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(PathBuf::from(home).join("workspace/wordless").join(rel));
        }
        candidates.into_iter().find(|p| p.is_file())
    }

    fn find_dir(rel: &str) -> Option<PathBuf> {
        let mut candidates = vec![PathBuf::from(rel)];
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(PathBuf::from(home).join("workspace/wordless").join(rel));
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
    fn health_answers() {
        let h = health();
        assert_eq!(h["ok"], true);
        assert_eq!(h["service"], "wordless-converter");
    }

    #[test]
    fn hello_dolly_converts_3_functions_2_hooks() {
        let Some(path) = find_file("reference/wordpress-7.1.5/wp-content/plugins/hello.php") else {
            return;
        };
        let source = std::fs::read_to_string(path).expect("hello.php readable");
        let (status, body) = convert_php(&json!({
            "filename": "hello.php",
            "source": source,
            "slug": "hello-dolly",
        }));
        assert_eq!(status, 200);
        assert_eq!(body["ok"], true);
        assert_eq!(
            body["report"]["converted"]["functions"],
            json!(["hello_dolly_get_lyric", "hello_dolly", "dolly_css"])
        );
        assert_eq!(body["report"]["unconverted"], json!([]));
        assert_eq!(
            body["report"]["converted"]["hooks"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(body["report"]["extension"]["slug"], "hello-dolly");
        assert!(
            body["report"]["output"]["code"]
                .as_str()
                .unwrap_or("")
                .contains("export function helloDollyGetLyric"),
            "generated TS must carry the converted function"
        );
    }

    #[test]
    fn blocked_construct_fails_with_violations_not_a_transport_error() {
        let (status, body) = convert_php(&json!({
            "filename": "bad.php",
            "source": "<?php function f($x) { return eval($x); }",
            "slug": "bad",
        }));
        assert_eq!(status, 200, "application failure is ok:false, not HTTP 500");
        assert_eq!(body["ok"], false);
        assert!(
            body["error"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("eval"),
            "{}",
            body["error"]
        );
        assert!(
            body["blocked"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "violations must travel with the failure: {body}"
        );
    }

    #[test]
    fn analyze_files_reports_verdicts() {
        let (status, body) = analyze_files(&json!({
            "files": [
                { "path": "good.php", "source": "<?php function g() { return 1; }" },
                { "path": "bad.php", "source": "<?php function f( { " },
            ],
        }));
        assert_eq!(status, 200);
        assert_eq!(body["ok"], true);
        let analyses = body["analyses"].as_array().unwrap();
        assert_eq!(analyses.len(), 2);
        assert_eq!(analyses[0]["analysis"]["verdict"], "convertible");
        assert_eq!(
            analyses[0]["analysis"]["functions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(analyses[1]["analysis"].is_null());
        assert!(analyses[1]["error"].is_string());
    }

    #[test]
    fn twentytwentyfive_matches_v1_conversion_record() {
        // v1 M11 record (measured): 23 converted / 76 partial / 0 blocked
        // of 99 PHP files — now proven at the converter-service layer.
        let Some(dir) = find_dir("reference/wordpress-7.1.5/wp-content/themes/twentytwentyfive")
        else {
            return;
        };
        let files = collect_php_files(&dir);
        assert_eq!(files.len(), 99, "TT25 should have 99 PHP files");
        let (status, body) = convert_files(&json!({
            "files": files.iter().map(|(p, s)| json!({ "path": p, "source": s })).collect::<Vec<_>>(),
            "slug": "twentytwentyfive",
            "kind": "theme",
        }));
        assert_eq!(status, 200);
        assert_eq!(body["ok"], true);
        assert_eq!(body["report"]["converted"], 23);
        assert_eq!(body["report"]["partial"], 76);
        assert_eq!(body["report"]["blocked"], 0);
        assert_eq!(body["generated"].as_array().unwrap().len(), 99);
    }

    #[test]
    fn render_elementor_returns_html() {
        let (status, body) = render_elementor(&json!({
            "data": [{
                "id": "abc",
                "elType": "section",
                "elements": [{
                    "id": "col1",
                    "elType": "column",
                    "elements": [{
                        "id": "w1",
                        "elType": "widget",
                        "widgetType": "heading",
                        "settings": { "title": "Hello Elementor" },
                    }],
                }],
            }],
        }));
        assert_eq!(status, 200);
        assert_eq!(body["ok"], true);
        assert!(
            body["html"]
                .as_str()
                .unwrap_or("")
                .contains("Hello Elementor"),
            "{}",
            body["html"]
        );
    }

    #[test]
    fn missing_fields_are_400() {
        let (status, body) = convert_php(&json!({ "filename": "x.php" }));
        assert_eq!(status, 400);
        assert_eq!(body["ok"], false);
    }
}
