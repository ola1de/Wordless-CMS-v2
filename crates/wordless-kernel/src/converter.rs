//! Platform-neutral interface to the converter Worker (split, 2026-11-20).
//!
//! The PHP → TypeScript conversion engine (`wordless-conversion`,
//! `wordless-elementor`) no longer lives in the main Worker: the
//! combined WASM binary outgrew the Cloudflare free-tier 3 MiB limit
//! (P7: 3,204,402 bytes, 101.9%). The engine runs in a second Worker
//! (`wordless-converter`) with its own 3 MiB budget, bound to the main
//! Worker as a service binding (production) or reached over plain HTTP
//! at `WORDLESS_CONVERTER_URL` (local dev — the P6 `dev_url` pattern).
//!
//! The kernel defines the trait; the worker entry implements it over
//! the real transport. Services only ever see this interface, so the
//! transport stays replaceable and the logic stays testable — the same
//! shape as [`crate::storage::ObjectStore`].
//!
//! Contract (all methods speak JSON — the converter returns the exact
//! serde shapes of the conversion crate's structs, so responses are
//! byte-identical to the old in-process calls):
//!
//! - [`ConverterClient::convert_php`] — one file through the full
//!   pipeline; returns the `ConversionReport` JSON on success.
//! - [`ConverterClient::analyze_files`] — per-file static analysis for
//!   a batch; returns the analyses array (`[{ "path", "analysis" |
//!   null, "error" | null }]`, one round trip for a whole upload, not
//!   one per file).
//! - [`ConverterClient::convert_files`] — a whole upload; returns
//!   `{ "generated": [[path, code], ...], "report":
//!   AutoConvertReport, "defaults": [OptionDefault, ...] }`.
//! - [`ConverterClient::theme_settings`] — the theme.json settings
//!   summary (`SettingsMigrationReport` JSON).
//! - [`ConverterClient::render_elementor`] — Elementor `_elementor_data`
//!   JSON to HTML.
//!
//! A transport failure (converter unreachable, non-JSON answer) is a
//! [`ConverterError`] with no `blocked` list. An application-level
//! failure from the converter (blocked construct, unparseable file)
//! carries the message and, for `convert_php`, the blocking
//! violations, so the job record keeps the same failed shape as
//! before the split.

use async_trait::async_trait;
use serde_json::Value;

/// What went wrong calling the converter service.
#[derive(Debug, Clone)]
pub struct ConverterError {
    /// Human-readable message (surfaced in job errors / API errors).
    pub message: String,
    /// Blocking violations from the conversion sandbox, when the
    /// converter rejected the input (only `convert_php` sets this).
    pub blocked: Option<Value>,
}

impl ConverterError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            blocked: None,
        }
    }

    pub fn blocked(message: impl Into<String>, blocked: Value) -> Self {
        Self {
            message: message.into(),
            blocked: Some(blocked),
        }
    }

    /// The caller-facing error (a 500 with the converter's message).
    pub fn to_wordless(&self) -> crate::error::WordlessError {
        crate::error::internal_error(&format!("Conversion service failed: {}.", self.message))
    }
}

impl std::fmt::Display for ConverterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ConverterError {}

/// Converter-service surface. `?Send`, like the `Db` trait and
/// `ObjectStore`: Workers futures are `!Send`, and the runtime is
/// single-threaded.
#[async_trait(?Send)]
pub trait ConverterClient {
    /// One PHP file through the full pipeline (safety scan → parse →
    /// emit). Returns the `ConversionReport` JSON. A blocked construct
    /// or an unparseable file is a [`ConverterError`] (with `blocked`
    /// set for sandbox rejections), not an `Ok`.
    async fn convert_php(
        &self,
        filename: &str,
        source: &str,
        slug: &str,
    ) -> Result<Value, ConverterError>;

    /// Static analysis for a batch of files, in one round trip.
    /// Returns the analyses array: `[{ "path", "analysis" | null,
    /// "error" | null }]` — `analysis` is the `FileAnalysis` JSON when
    /// the file parsed, otherwise `error` carries the parse failure.
    async fn analyze_files(&self, files: &[(String, String)]) -> Result<Value, ConverterError>;

    /// A whole upload through auto-convert. Returns `{ "generated":
    /// [[path, code], ...], "report": AutoConvertReport, "defaults":
    /// [OptionDefault, ...] }` — the generated TypeScript (keyed by
    /// `converted/<path>.ts`), the per-file record, and the literal
    /// `get_option()` defaults found (for the caller to migrate).
    async fn convert_files(
        &self,
        files: &[(String, String)],
        slug: &str,
        kind: &str,
    ) -> Result<Value, ConverterError>;

    /// The theme.json settings summary as a `SettingsMigrationReport`
    /// JSON (`carriedOver` = settings the theme engine applies as CSS,
    /// `skipped` = unsupported paths).
    async fn theme_settings(
        &self,
        theme_json: &Value,
        unsupported: &[String],
    ) -> Result<Value, ConverterError>;

    /// Elementor `_elementor_data` JSON to HTML (content only, no
    /// editor — same as the old in-process renderer).
    async fn render_elementor(&self, data: &Value) -> Result<String, ConverterError>;
}
