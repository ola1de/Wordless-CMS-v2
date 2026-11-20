//! WORDLESS v2 conversion job records (split, 2026-11-20).
//!
//! Moved out of `wordless-conversion` when the conversion engine was
//! split into its own Worker: the job *records* (a row in `wl_jobs`,
//! kind `'conversion'`) stay in the main Worker, but the pipeline
//! itself runs in `wordless-converter` and is reached through the
//! kernel's [`ConverterClient`] trait. This crate links no conversion
//! code — only `wordless-kernel` and `wordless-db` — so the main
//! Worker's WASM stays under the free-tier 3 MiB limit.
//!
//! The API is the same as before the split: [`create_conversion_job`],
//! [`get_conversion_job`], [`list_conversion_jobs`],
//! [`run_conversion_job`]. One deliberate change: the stored report is
//! now a [`serde_json::Value`] instead of the pipeline's
//! `ConversionReport` struct — the converter returns the report as
//! JSON and the main Worker stores it verbatim, so the API response is
//! byte-identical without linking the pipeline's types.
//!
//! Also home to [`migrate_option_defaults`]: carrying literal
//! `get_option()` defaults over as Wordless options (only when the
//! option does not already exist). The defaults themselves arrive as
//! JSON from the converter; this function only does the DB writes.

pub mod jobs;
pub mod settings;

#[cfg(test)]
mod test_support;

pub use jobs::{
    create_conversion_job, get_conversion_job, list_conversion_jobs, run_conversion_job,
    ConversionJobList, ConversionJobStatus, ConversionJobView, CreateConversionJobInput,
    CONVERSION_FIXTURES, MAX_PHP_BYTES,
};
pub use settings::migrate_option_defaults;

/// The wire shape of a `FileAnalysis` (the converter's `analyze_php`
/// result), mirrored here so the main Worker's crates can read it
/// without linking the conversion engine. Field-for-field the same
/// serde shape as `wordless_conversion::analysis::FileAnalysis`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileAnalysisJson {
    pub file: String,
    pub lines: usize,
    pub bytes: usize,
    pub tokens: usize,
    pub nodes: usize,
    #[serde(default)]
    pub functions: Vec<String>,
    #[serde(default)]
    pub classes: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<HookRefJson>,
    #[serde(default, rename = "wpCalls")]
    pub wp_calls: Vec<String>,
    #[serde(default, rename = "optionKeys")]
    pub option_keys: Vec<String>,
    #[serde(default, rename = "optionDefaults")]
    pub option_defaults: Vec<OptionDefaultJson>,
    #[serde(default, rename = "httpCalls")]
    pub http_calls: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub unconvertible: Vec<UnconvertibleFeatureJson>,
    /// `"convertible" | "partial" | "blocked"`.
    pub verdict: String,
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// One hook reference (`wordless_conversion::HookRef` wire shape).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HookRefJson {
    pub tag: String,
    /// `"action"` or `"filter"`.
    pub kind: String,
}

/// One feature the converter cannot translate as-is
/// (`wordless_conversion::UnconvertibleFeature` wire shape).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UnconvertibleFeatureJson {
    pub kind: String,
    pub reason: String,
    pub line: usize,
}

/// A literal scalar default from a `get_option()` / `add_option()` call
/// (`wordless_conversion::OptionDefault` wire shape).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OptionDefaultJson {
    pub key: String,
    /// Literal scalar default (string / number / bool only).
    #[serde(rename = "defaultValue")]
    pub default_value: serde_json::Value,
    pub file: String,
}
