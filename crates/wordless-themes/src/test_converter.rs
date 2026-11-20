//! Test-only converter client (split, 2026-11-20).
//!
//! Theme-import tests need a converter, but there is no running
//! converter Worker in `cargo test`. This client calls the real
//! conversion engine in-process (dev-dependency `wordless-conversion`
//! / `wordless-elementor`) and speaks the same JSON contract as the
//! Worker client, so the tests prove the conversion record end to end
//! without a network hop. Dev-dependencies never link into the
//! release WASM — the production path still goes through the
//! converter Worker.

use async_trait::async_trait;
use serde_json::{json, Value};
use wordless_kernel::converter::{ConverterClient, ConverterError};

/// In-process converter for tests: the real engine, same JSON shapes
/// as the `wordless-converter` Worker endpoints.
pub struct TestConverter;

#[async_trait(?Send)]
impl ConverterClient for TestConverter {
    async fn convert_php(
        &self,
        filename: &str,
        source: &str,
        slug: &str,
    ) -> Result<Value, ConverterError> {
        match wordless_conversion::convert_php(
            source,
            filename,
            &wordless_conversion::ConvertOptions {
                slug: slug.to_string(),
                compat_import: None,
                source_label: None,
            },
        ) {
            Ok(report) => serde_json::to_value(report)
                .map_err(|e| ConverterError::new(format!("Failed to serialise report: {e}"))),
            Err(wordless_conversion::ConversionError::Blocked(e)) => Err(ConverterError::blocked(
                e.to_string(),
                json!(e
                    .violations
                    .iter()
                    .map(|v| json!({
                        "construct": v.construct,
                        "reason": v.reason,
                        "line": v.line,
                    }))
                    .collect::<Vec<_>>()),
            )),
            Err(wordless_conversion::ConversionError::Parse(e)) => {
                Err(ConverterError::new(e.to_string()))
            }
        }
    }

    async fn analyze_files(&self, files: &[(String, String)]) -> Result<Value, ConverterError> {
        let analyses: Vec<Value> = files
            .iter()
            .map(
                |(path, source)| match wordless_conversion::analyze_php(source, path) {
                    Ok(analysis) => json!({ "path": path, "analysis": analysis, "error": null }),
                    Err(e) => json!({ "path": path, "analysis": null, "error": e.message }),
                },
            )
            .collect();
        Ok(Value::Array(analyses))
    }

    async fn convert_files(
        &self,
        files: &[(String, String)],
        slug: &str,
        kind: &str,
    ) -> Result<Value, ConverterError> {
        let (generated, report) = wordless_conversion::convert_php_files(files, slug, kind);
        let defaults = wordless_conversion::collect_option_defaults(files);
        Ok(json!({ "generated": generated, "report": report, "defaults": defaults }))
    }

    async fn theme_settings(
        &self,
        theme_json: &Value,
        unsupported: &[String],
    ) -> Result<Value, ConverterError> {
        let settings = wordless_conversion::summarize_theme_json_settings(theme_json, unsupported);
        serde_json::to_value(settings)
            .map_err(|e| ConverterError::new(format!("Failed to serialise settings: {e}")))
    }

    async fn render_elementor(&self, data: &Value) -> Result<String, ConverterError> {
        Ok(wordless_elementor::render_elementor_data(data))
    }
}
