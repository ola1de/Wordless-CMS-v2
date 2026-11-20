//! WORDLESS v2 themes (P2).
//!
//! theme.json engine v1–v3, theme registry, ZIP import, customizer,
//! patterns, classic-theme analysis. Port of the v1 module under
//! `src/modules/themes/` in the v1.1.0 TypeScript repo (read-only
//! reference — same engine semantics, same registry tables, same caps:
//! 10,000 files / 50 MB per ZIP; the bundled default theme is
//! `themes/wordless-default/` in the v1 repo, copied to
//! `wordless-rust/themes/wordless-default/` and embedded with
//! `include_str!`).
//!
//! Modules:
//! - `themejson` — pure theme.json v1–v3 → CSS, cargo-testable.
//! - `zip` — ZIP reading via the `zip` crate, in-memory Cursor.
//! - `registry` — wl_themes rows, import/activate/delete, built-in
//!   theme refresh, active-theme loading.
//! - `customizer` — variations + preview tokens.
//! - `patterns` — wp:pattern file helpers + the allowlisted literal
//!   PHP evaluator (no PHP execution, same rule as v1). The template
//!   renderer that *uses* these lives in `wordless-render`.
//! - `classic` — style.css header parsing + template-tag scan for
//!   classic themes (stylesheet + analysis report only).
//!
//! Service functions take `db: &dyn Db` (and `store: &dyn
//! ObjectStore` where the v1 flow stores ZIP bytes in R2) plus plain
//! parameters and return `Result<_, WordlessError>` (typed results);
//! route handlers live in `wordless-worker` and are wired by the
//! P2 coordinator.
//!
//! Honest P2 gaps (see the crate report): v1's M11 auto-convert
//! (PHP → TypeScript + option-default carry-over) is a P3
//! `wordless-conversion` integration point — import reports carry
//! `"conversion": null`; classic per-file convertibility verdicts are
//! `partial` until the P3 analyser exists.

pub mod classic;
pub mod customizer;
pub mod patterns;
pub mod registry;
pub mod themejson;
pub mod zip;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
pub(crate) mod test_converter;

pub use classic::{
    analyze_classic_theme, find_template_tags, parse_style_css_header, render_classic_report,
    ClassicFileReport, ClassicReport, StyleCssHeader, TEMPLATE_TAGS,
};
pub use customizer::{
    create_preview, delete_variation, get_preview, get_variation, resolve_variation,
    save_variation, variation_option_key, ThemePreview, ThemeVariation, PREVIEW_OPTION,
    PREVIEW_TTL_MS,
};
pub use patterns::{
    escape_html, evaluate_pattern_php, extract_pattern_html, find_pattern_file, kses_post,
    pattern_file_name,
};
pub use registry::{
    activate_theme, delete_theme, ensure_default_theme, get_active_stylesheet,
    get_active_theme_slug, get_theme, get_theme_detail, import_classic_theme, import_theme,
    list_themes, load_active_theme, ActiveStylesheet, ClassicImportResult, ImportResult,
    LoadedTheme, ThemeDetail, ThemeRecord, ACTIVE_THEME_OPTION, DEFAULT_THEME_AUTHOR,
    DEFAULT_THEME_FILES, DEFAULT_THEME_JSON, DEFAULT_THEME_NAME, DEFAULT_THEME_SLUG,
    DEFAULT_THEME_STYLE_CSS, DEFAULT_THEME_VERSION,
};
pub use themejson::{
    generate_theme_css, generate_theme_css_with_variation, merge_theme_settings, parse_theme_json,
    resolve_preset_value, stylesheet_hash, validate_theme_settings, ParsedThemeJson,
    ThemeJsonIssue, ThemeJsonVersion,
};
pub use zip::{
    extract_theme_zip, zip_text_file, zip_text_files_under, ExtractedThemeZip,
    ALLOWED_THEME_EXTENSIONS, MAX_THEME_FILES, MAX_THEME_ZIP_BYTES,
};
