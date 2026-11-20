//! Theme registry (P2) — port of the registry half of v1
//! `src/modules/themes/service.ts` (+ `types.ts`).
//!
//! Storage model (same as v1):
//! - Metadata + theme.json + generated stylesheet: `wl_themes` (D1).
//! - Theme files (templates/, parts/, style.css, assets): R2 under
//!   `themes/<slug>/` via the `ObjectStore` interface.
//! - The built-in default theme ("wordless-default") is different: its
//!   files live in the in-code bundle (embedded below from
//!   `themes/wordless-default/`, copied from the v1 repo) because the
//!   Worker cannot read repo files at runtime. Its D1 row is seeded
//!   lazily the first time any theme read happens on a fresh install,
//!   and refreshed when the bundled theme version is newer than the
//!   stored row (upgrade).
//! - The active theme is the `active_theme` option (default
//!   "wordless-default"); `wl_themes.status` mirrors it.
//!
//! P3: v1's M11 auto-convert runs here — an imported theme's PHP is
//! converted with `wordless-conversion`, the generated TypeScript is
//! stored under `themes/<slug>/converted/`, and literal option
//! defaults are carried over as Wordless options (never overwriting
//! existing values). Imported PHP is stored, never executed, and the
//! import report carries the conversion record.
//!
//! Service functions use only the primitive `Db` methods (`query_all` /
//! `query_one` / `exec`) — never `prepare` — so they stay testable
//! against the in-memory fake in `test_support.rs` (the `Statement`
//! constructor is `pub(crate)` to `wordless-db`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wordless_conversion_jobs::migrate_option_defaults;
use wordless_db::row::{row_opt_text, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::converter::ConverterClient;
use wordless_kernel::error::{bad_request, internal_error, not_found, WordlessError};
use wordless_kernel::storage::ObjectStore;
use wordless_kernel::time::now_iso;

use crate::classic::{analyze_classic_theme, parse_style_css_header, ClassicReport};
use crate::themejson::{
    generate_theme_css, generate_theme_css_with_variation, parse_theme_json, stylesheet_hash,
    ParsedThemeJson,
};
use crate::zip::{extract_theme_zip, ExtractedThemeZip};

/// The built-in default theme's slug.
pub const DEFAULT_THEME_SLUG: &str = "wordless-default";
/// The built-in default theme's display name.
pub const DEFAULT_THEME_NAME: &str = "Wordless";
/// The built-in default theme's author.
pub const DEFAULT_THEME_AUTHOR: &str = "Wordless";
/// The built-in default theme's release version.
///
/// NOTE: this is the *theme release* version (v1 `DEFAULT_THEME_VERSION`,
/// from the style.css header). The theme.json `"version": 3` is the
/// theme.json *schema* version — a different number.
pub const DEFAULT_THEME_VERSION: &str = "0.7.0";
/// Option key holding the active theme's slug.
pub const ACTIVE_THEME_OPTION: &str = "active_theme";

/// The bundled default theme.json (embedded from
/// `themes/wordless-default/theme.json`).
pub const DEFAULT_THEME_JSON: &str = include_str!("../../../themes/wordless-default/theme.json");
/// The bundled default theme's style.css (embedded from
/// `themes/wordless-default/style.css`).
pub const DEFAULT_THEME_STYLE_CSS: &str =
    include_str!("../../../themes/wordless-default/style.css");
/// The bundled default theme's template/part files, as
/// (relative path, content) pairs.
pub const DEFAULT_THEME_FILES: &[(&str, &str)] = &[
    (
        "templates/404.html",
        include_str!("../../../themes/wordless-default/templates/404.html"),
    ),
    (
        "templates/archive.html",
        include_str!("../../../themes/wordless-default/templates/archive.html"),
    ),
    (
        "templates/home.html",
        include_str!("../../../themes/wordless-default/templates/home.html"),
    ),
    (
        "templates/index.html",
        include_str!("../../../themes/wordless-default/templates/index.html"),
    ),
    (
        "templates/page.html",
        include_str!("../../../themes/wordless-default/templates/page.html"),
    ),
    (
        "templates/search.html",
        include_str!("../../../themes/wordless-default/templates/search.html"),
    ),
    (
        "templates/single.html",
        include_str!("../../../themes/wordless-default/templates/single.html"),
    ),
    (
        "parts/footer.html",
        include_str!("../../../themes/wordless-default/parts/footer.html"),
    ),
    (
        "parts/header.html",
        include_str!("../../../themes/wordless-default/parts/header.html"),
    ),
];

/// One `wl_themes` row, as the API returns it (v1 `ThemeRecord`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeRecord {
    pub slug: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub kind: String,
    pub status: String,
    /// True when a theme.json is stored (block themes only).
    #[serde(rename = "hasThemeJson")]
    pub has_theme_json: bool,
    #[serde(rename = "stylesheetHash")]
    pub stylesheet_hash: String,
    pub source: String,
    /// Import/classic report when one exists (parsed report_json).
    pub report: Option<Value>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// A theme with its files and stylesheet (v1 `ThemeDetail`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeDetail {
    #[serde(flatten)]
    pub record: ThemeRecord,
    /// Parsed theme.json (block themes) or null (classic themes).
    #[serde(rename = "themeJson")]
    pub theme_json: Option<Value>,
    /// The generated (block) or imported (classic) stylesheet.
    pub stylesheet: String,
    /// Theme files: relative path → content (templates, parts, style.css).
    pub files: std::collections::BTreeMap<String, String>,
}

/// A loaded theme: record + parsed theme.json + files + stylesheet.
#[derive(Debug, Clone)]
pub struct LoadedTheme {
    pub record: ThemeRecord,
    /// Parsed theme.json (block themes) or None (classic themes).
    pub parsed: Option<ParsedThemeJson>,
    /// Theme files: relative path → content.
    pub files: Vec<(String, String)>,
    /// Active stylesheet (generated or imported), variation applied when given.
    pub stylesheet: String,
    pub stylesheet_hash: String,
}

/// Stylesheet for the active theme (variation applied when given).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveStylesheet {
    pub css: String,
    pub hash: String,
    pub slug: String,
}

/// The result of importing a block theme (v1 `ImportResult`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub theme: ThemeRecord,
    /// Warnings from the import (skipped files, unsupported theme.json features).
    pub warnings: Vec<String>,
    /// Files stored, by relative path.
    pub files: Vec<String>,
}

/// The result of importing a classic theme.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassicImportResult {
    pub theme: ThemeRecord,
    pub report: ClassicReport,
    pub warnings: Vec<String>,
}

/// One `wl_themes` row in D1 column form.
#[derive(Debug, Clone)]
struct ThemeRow {
    slug: String,
    name: String,
    version: String,
    author: String,
    kind: String,
    status: String,
    theme_json: Option<String>,
    stylesheet: String,
    stylesheet_hash: String,
    source: String,
    report_json: Option<String>,
    created_at: String,
    updated_at: String,
}

fn row_to_theme_row(row: &Row) -> ThemeRow {
    ThemeRow {
        slug: row_text(row, "slug"),
        name: row_text(row, "name"),
        version: row_text(row, "version"),
        author: row_text(row, "author"),
        kind: row_text(row, "kind"),
        status: row_text(row, "status"),
        theme_json: row_opt_text(row, "theme_json"),
        stylesheet: row_text(row, "stylesheet"),
        stylesheet_hash: row_text(row, "stylesheet_hash"),
        source: row_text(row, "source"),
        report_json: row_opt_text(row, "report_json"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

fn row_to_record(row: &ThemeRow) -> ThemeRecord {
    let mut report: Option<Value> = None;
    if let Some(raw) = &row.report_json {
        if !raw.is_empty() {
            report = serde_json::from_str(raw).ok();
        }
    }
    ThemeRecord {
        slug: row.slug.clone(),
        name: row.name.clone(),
        version: row.version.clone(),
        author: row.author.clone(),
        kind: row.kind.clone(),
        status: row.status.clone(),
        has_theme_json: !row.theme_json.as_deref().unwrap_or("").is_empty(),
        stylesheet_hash: row.stylesheet_hash.clone(),
        source: row.source.clone(),
        report,
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
    }
}

// ------------------------------------------------------- option helpers
//
// Local copies of the `wordless-db` options helpers, using only the
// primitive `Db` methods (see the module docs — `wordless-db`'s
// `get_option` / `set_option` go through `prepare`, which this crate
// cannot construct). Same SQL, same JSON-value semantics.

/// Read an option's JSON value, or `None` when the row is missing or
/// the value is not valid JSON.
/// P12: theme options (`active_theme`, variations, previews) are
/// per-site — site 1 reads `wp_options` exactly as before, other
/// sites read `wl_site_options` with fallback.
pub(crate) async fn get_option_json(
    db: &dyn Db,
    site_id: i64,
    key: &str,
) -> Result<Option<Value>, WordlessError> {
    Ok(wordless_multisite::get_site_option::<Option<Value>>(db, site_id, key, None).await?)
}

/// Write an option (insert or update), JSON-encoding the value.
pub(crate) async fn set_option_json(
    db: &dyn Db,
    site_id: i64,
    key: &str,
    value: &Value,
) -> Result<(), WordlessError> {
    wordless_multisite::set_site_option(db, site_id, key, value).await
}

/// Delete an option. Returns true when a row was removed.
pub(crate) async fn delete_option(db: &dyn Db, site_id: i64, key: &str) -> Result<bool, WordlessError> {
    Ok(wordless_multisite::delete_site_option(db, site_id, key).await?)
}

// ------------------------------------------------------- content types

fn content_type_for(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".gif") {
        "image/gif"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else if path.ends_with(".php") {
        "text/x-php; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

/// Compare two "x.y.z" versions: -1 when a < b, 0 when equal, 1 when a > b.
pub fn compare_semver(a: &str, b: &str) -> i32 {
    let pa: Vec<i64> = a.split('.').map(parse_semver_part).collect();
    let pb: Vec<i64> = b.split('.').map(parse_semver_part).collect();
    for i in 0..pa.len().max(pb.len()) {
        let d = pa.get(i).copied().unwrap_or(0) - pb.get(i).copied().unwrap_or(0);
        if d != 0 {
            return if d < 0 { -1 } else { 1 };
        }
    }
    0
}

/// JS `Number.parseInt(n, 10) || 0` for one dotted version segment.
fn parse_semver_part(s: &str) -> i64 {
    let t = s.trim_start();
    let t = t.strip_prefix('-').unwrap_or(t);
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<i64>().unwrap_or(0)
}

fn slugify_theme_name(name: &str, fallback: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    // Collapse runs of hyphens (v1's /[^a-z0-9]+/g → single "-").
    let mut collapsed = String::with_capacity(slug.len());
    let mut last_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !last_dash {
                collapsed.push(c);
            }
            last_dash = true;
        } else {
            collapsed.push(c);
            last_dash = false;
        }
    }
    let collapsed = collapsed.trim_matches('-').to_string();
    if collapsed.is_empty() {
        fallback.to_string()
    } else {
        collapsed
    }
}

/// The built-in default theme, as a D1 row (inserted lazily).
fn default_theme_row(now: &str) -> ThemeRow {
    let theme_json: Value =
        serde_json::from_str(DEFAULT_THEME_JSON).expect("bundled default theme.json must parse");
    let parsed = parse_theme_json(&theme_json);
    let generated = generate_theme_css(&parsed);
    let stylesheet = format!("{generated}\n{DEFAULT_THEME_STYLE_CSS}");
    ThemeRow {
        slug: DEFAULT_THEME_SLUG.to_string(),
        name: DEFAULT_THEME_NAME.to_string(),
        version: DEFAULT_THEME_VERSION.to_string(),
        author: DEFAULT_THEME_AUTHOR.to_string(),
        kind: "block".to_string(),
        status: "active".to_string(),
        theme_json: Some(DEFAULT_THEME_JSON.to_string()),
        stylesheet: stylesheet.clone(),
        stylesheet_hash: stylesheet_hash(&stylesheet),
        source: "builtin".to_string(),
        report_json: None,
        created_at: now.to_string(),
        updated_at: now.to_string(),
    }
}

/// Test-only row seeder (the refresh tests insert stale rows directly).
#[cfg(test)]
async fn insert_theme_row(db: &dyn Db, row: &ThemeRow) -> Result<(), WordlessError> {
    db.exec(
        "INSERT INTO wl_themes
         (slug, name, version, author, kind, status, theme_json, stylesheet, stylesheet_hash, source, report_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            Param::Text(row.slug.clone()),
            Param::Text(row.name.clone()),
            Param::Text(row.version.clone()),
            Param::Text(row.author.clone()),
            Param::Text(row.kind.clone()),
            Param::Text(row.status.clone()),
            Param::from(row.theme_json.clone()),
            Param::Text(row.stylesheet.clone()),
            Param::Text(row.stylesheet_hash.clone()),
            Param::Text(row.source.clone()),
            Param::from(row.report_json.clone()),
            Param::Text(row.created_at.clone()),
            Param::Text(row.updated_at.clone()),
        ],
    )
    .await?;
    Ok(())
}

/// Insert or update a theme row. On conflict the status and created_at
/// stay as they are (same `ON CONFLICT … DO UPDATE SET` list as v1).
async fn upsert_theme_row(db: &dyn Db, row: &ThemeRow) -> Result<(), WordlessError> {
    let now = now_iso();
    db.exec(
        "INSERT INTO wl_themes
         (slug, name, version, author, kind, status, theme_json, stylesheet, stylesheet_hash, source, report_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(slug) DO UPDATE SET
           name = excluded.name, version = excluded.version, author = excluded.author,
           kind = excluded.kind, theme_json = excluded.theme_json,
           stylesheet = excluded.stylesheet, stylesheet_hash = excluded.stylesheet_hash,
           source = excluded.source, report_json = excluded.report_json, updated_at = excluded.updated_at",
        &[
            Param::Text(row.slug.clone()),
            Param::Text(row.name.clone()),
            Param::Text(row.version.clone()),
            Param::Text(row.author.clone()),
            Param::Text(row.kind.clone()),
            Param::Text(row.status.clone()),
            Param::from(row.theme_json.clone()),
            Param::Text(row.stylesheet.clone()),
            Param::Text(row.stylesheet_hash.clone()),
            Param::Text(row.source.clone()),
            Param::from(row.report_json.clone()),
            Param::Text(now.clone()),
            Param::Text(now),
        ],
    )
    .await?;
    Ok(())
}

/// Insert the default theme row when the table is empty (fresh install),
/// or refresh the built-in row when the bundled theme is newer
/// (upgrade), so an existing install picks up a redesigned default
/// theme. Only the built-in row is ever refreshed: imported/custom
/// themes are left alone, a row newer than the bundle is never
/// downgraded, and customizer variations (stored as options, not in
/// this row) are untouched.
pub async fn ensure_default_theme(db: &dyn Db) -> Result<(), WordlessError> {
    let count_row: Option<Row> = db
        .query_one("SELECT COUNT(*) AS n FROM wl_themes", &[])
        .await?;
    let count = count_row
        .as_ref()
        .map(|r| {
            r.get("n").and_then(|n| n.as_i64()).unwrap_or_else(|| {
                r.get("n")
                    .and_then(|n| n.as_f64())
                    .map(|f| f as i64)
                    .unwrap_or(0)
            })
        })
        .unwrap_or(0);
    let row = default_theme_row(&now_iso());
    if count == 0 {
        // INSERT OR IGNORE: a concurrent seed must not fail the request.
        db.exec(
            "INSERT OR IGNORE INTO wl_themes
             (slug, name, version, author, kind, status, theme_json, stylesheet, stylesheet_hash, source, report_json, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[
                Param::Text(row.slug.clone()),
                Param::Text(row.name.clone()),
                Param::Text(row.version.clone()),
                Param::Text(row.author.clone()),
                Param::Text(row.kind.clone()),
                Param::Text(row.status.clone()),
                Param::from(row.theme_json.clone()),
                Param::Text(row.stylesheet.clone()),
                Param::Text(row.stylesheet_hash.clone()),
                Param::Text(row.source.clone()),
                Param::from(row.report_json.clone()),
                Param::Text(row.created_at.clone()),
                Param::Text(row.updated_at.clone()),
            ],
        )
        .await?;
        return Ok(());
    }
    let existing: Option<Row> = db
        .query_one(
            "SELECT version FROM wl_themes WHERE slug = ? AND source = 'builtin' LIMIT 1",
            &[Param::Text(DEFAULT_THEME_SLUG.to_string())],
        )
        .await?;
    let Some(existing) = existing else {
        return Ok(());
    };
    if compare_semver(DEFAULT_THEME_VERSION, &row_text(&existing, "version")) <= 0 {
        return Ok(());
    }
    db.exec(
        "UPDATE wl_themes
         SET theme_json = ?, stylesheet = ?, stylesheet_hash = ?, version = ?, updated_at = ?
         WHERE slug = ? AND source = 'builtin'",
        &[
            Param::from(row.theme_json.clone()),
            Param::Text(row.stylesheet.clone()),
            Param::Text(row.stylesheet_hash.clone()),
            Param::Text(row.version.clone()),
            Param::Text(row.updated_at.clone()),
            Param::Text(DEFAULT_THEME_SLUG.to_string()),
        ],
    )
    .await?;
    Ok(())
}

async fn get_theme_row(db: &dyn Db, slug: &str) -> Result<Option<ThemeRow>, WordlessError> {
    let row: Option<Row> = db
        .query_one(
            "SELECT * FROM wl_themes WHERE slug = ? LIMIT 1",
            &[Param::Text(slug.to_string())],
        )
        .await?;
    Ok(row.as_ref().map(row_to_theme_row))
}

pub async fn list_themes(db: &dyn Db) -> Result<Vec<ThemeRecord>, WordlessError> {
    ensure_default_theme(db).await?;
    let rows: Vec<Row> = db
        .query_all("SELECT * FROM wl_themes ORDER BY slug ASC", &[])
        .await?;
    Ok(rows
        .iter()
        .map(|r| row_to_record(&row_to_theme_row(r)))
        .collect())
}

pub async fn get_theme(db: &dyn Db, slug: &str) -> Result<ThemeRecord, WordlessError> {
    ensure_default_theme(db).await?;
    match get_theme_row(db, slug).await? {
        Some(row) => Ok(row_to_record(&row)),
        None => Err(not_found(&format!("Theme \"{slug}\" is not installed."))),
    }
}

pub async fn get_active_theme_slug(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    ensure_default_theme(db).await?;
    let slug = match get_option_json(db, site_id, ACTIVE_THEME_OPTION).await? {
        Some(Value::String(s)) => s,
        _ => DEFAULT_THEME_SLUG.to_string(),
    };
    let row: Option<Row> = db
        .query_one(
            "SELECT slug FROM wl_themes WHERE slug = ? LIMIT 1",
            &[Param::Text(slug.clone())],
        )
        .await?;
    Ok(if row.is_none() {
        DEFAULT_THEME_SLUG.to_string()
    } else {
        slug
    })
}

async fn load_theme_files(
    store: &dyn ObjectStore,
    row: &ThemeRow,
) -> Result<Vec<(String, String)>, WordlessError> {
    if row.slug == DEFAULT_THEME_SLUG {
        // Built-in default: files come from the in-code bundle, not R2.
        let mut files: Vec<(String, String)> = DEFAULT_THEME_FILES
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect();
        files.push(("style.css".to_string(), DEFAULT_THEME_STYLE_CSS.to_string()));
        return Ok(files);
    }
    let prefix = format!("themes/{}/", row.slug);
    let mut files: Vec<(String, String)> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = store.list_objects(&prefix, 1000, cursor.as_deref()).await?;
        for meta in &page.objects {
            if let Some((bytes, _)) = store.get_object(&meta.key).await? {
                files.push((
                    meta.key[prefix.len()..].to_string(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                ));
            }
        }
        if !page.truncated {
            break;
        }
        cursor = page.cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(files)
}

/// Load the active theme with its files and stylesheet.
pub async fn load_active_theme(
    db: &dyn Db,
    site_id: i64,
    store: &dyn ObjectStore,
    variation: Option<&Value>,
) -> Result<LoadedTheme, WordlessError> {
    let slug = get_active_theme_slug(db, site_id).await?;
    let row = match get_theme_row(db, &slug).await? {
        Some(row) => row,
        None => {
            return Err(internal_error(&format!(
                "Active theme \"{slug}\" has no wl_themes row."
            )))
        }
    };
    let record = row_to_record(&row);
    let mut parsed: Option<ParsedThemeJson> = None;
    if let Some(theme_json) = &row.theme_json {
        if !theme_json.is_empty() {
            if let Ok(value) = serde_json::from_str::<Value>(theme_json) {
                parsed = Some(parse_theme_json(&value));
            }
        }
    }
    let files = load_theme_files(store, &row).await?;
    let mut stylesheet = row.stylesheet.clone();
    if let (Some(parsed), Some(variation)) = (&parsed, variation) {
        let style_css = files
            .iter()
            .find(|(p, _)| p == "style.css")
            .map(|(_, c)| c.as_str())
            .unwrap_or("");
        stylesheet = format!(
            "{}\n{}",
            generate_theme_css_with_variation(parsed, variation),
            style_css
        );
    }
    let stylesheet_hash = stylesheet_hash(&stylesheet);
    Ok(LoadedTheme {
        record,
        parsed,
        files,
        stylesheet,
        stylesheet_hash,
    })
}

/// Stylesheet for the active theme (variation applied when given).
pub async fn get_active_stylesheet(
    db: &dyn Db,
    site_id: i64,
    store: &dyn ObjectStore,
    variation: Option<&Value>,
) -> Result<ActiveStylesheet, WordlessError> {
    let loaded = load_active_theme(db, site_id, store, variation).await?;
    Ok(ActiveStylesheet {
        css: loaded.stylesheet,
        hash: loaded.stylesheet_hash,
        slug: loaded.record.slug,
    })
}

async fn store_theme_files(
    store: &dyn ObjectStore,
    slug: &str,
    files: &ExtractedThemeZip,
) -> Result<(), WordlessError> {
    for (path, bytes) in &files.files {
        store
            .put_object(
                &format!("themes/{slug}/{path}"),
                bytes.clone(),
                content_type_for(path),
            )
            .await?;
    }
    Ok(())
}

/// Import a block theme from a ZIP upload.
///
/// P3: the M11 auto-convert runs on the theme's PHP (see
/// [`auto_convert_theme`]); the import report's `conversion` field
/// carries the record, and generated TypeScript is stored under
/// `themes/<slug>/converted/`. PHP files are stored as-is and never
/// executed.
pub async fn import_theme(
    db: &dyn Db,
    store: &dyn ObjectStore,
    filename: &str,
    data: &[u8],
    converter: Option<&dyn ConverterClient>,
) -> Result<ImportResult, WordlessError> {
    if !filename.to_lowercase().ends_with(".zip") {
        return Err(bad_request("Theme import requires a .zip file."));
    }
    ensure_default_theme(db).await?;
    let extracted = extract_theme_zip(data)?;
    let style_css = match extracted.get("style.css") {
        Some(bytes) => bytes.to_vec(),
        None => {
            return Err(bad_request(
                "The theme ZIP is missing \"style.css\" at its root.",
            ))
        }
    };
    let theme_json_raw = match extracted.get("theme.json") {
        Some(bytes) => bytes.to_vec(),
        None => {
            return Err(bad_request(
                "The theme ZIP is missing \"theme.json\" at its root. Block themes need one; classic themes use the classic import.",
            ))
        }
    };
    let style_css_text = String::from_utf8_lossy(&style_css).into_owned();
    let header = match parse_style_css_header(&style_css_text) {
        Some(header) => header,
        None => return Err(bad_request(
            "style.css has no \"Theme Name:\" header — this does not look like a WordPress theme.",
        )),
    };
    let theme_json: Value = match serde_json::from_slice(&theme_json_raw) {
        Ok(value) => value,
        Err(_) => return Err(bad_request("theme.json is not valid JSON.")),
    };
    let parsed = parse_theme_json(&theme_json);
    if !parsed.errors.is_empty() {
        let detail = parsed
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.path, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(bad_request(&format!("theme.json is invalid — {detail}")));
    }
    let slug = slugify_theme_name(
        if header.text_domain.is_empty() {
            &header.name
        } else {
            &header.text_domain
        },
        "imported-theme",
    );
    if slug == DEFAULT_THEME_SLUG {
        return Err(bad_request(&format!(
            "A theme with slug \"{DEFAULT_THEME_SLUG}\" already exists (the built-in default)."
        )));
    }
    let generated = generate_theme_css(&parsed);
    let stylesheet = format!("{generated}\n{style_css_text}");
    let mut warnings = extracted.warnings.clone();
    warnings.extend(
        parsed
            .warnings
            .iter()
            .map(|w| format!("{}: {}", w.path, w.message)),
    );
    let mut sorted_files: Vec<String> = extracted.paths().iter().map(|s| s.to_string()).collect();
    sorted_files.sort();
    let mut report = json!({
        "kind": "block-theme-import",
        "version": parsed.version,
        "warnings": warnings,
        "unsupported": parsed.unsupported,
        "files": sorted_files,
        "conversion": Value::Null,
    });
    // P5 F9: v1 returns the auto-convert report on EVERY import —
    // even a PHP-free theme gets the (empty) report with the
    // theme.json settings summary. The gate on `has_php` left
    // `conversion: null` for PHP-free themes.
    let php_files: Vec<(String, String)> = extracted
        .files
        .iter()
        .filter(|(p, _)| p.to_lowercase().ends_with(".php"))
        .map(|(p, b)| (p.clone(), String::from_utf8_lossy(b).into_owned()))
        .collect();
    report["conversion"] = auto_convert_theme(
        db,
        store,
        &slug,
        &php_files,
        Some(&theme_json),
        &parsed.unsupported,
        converter,
    )
    .await?;
    store_theme_files(store, &slug, &extracted).await?;
    upsert_theme_row(
        db,
        &ThemeRow {
            slug: slug.clone(),
            name: header.name.clone(),
            version: header.version.clone(),
            author: header.author.clone(),
            kind: "block".to_string(),
            status: "inactive".to_string(),
            theme_json: Some(serde_json::to_string(&theme_json).unwrap_or_default()),
            stylesheet: stylesheet.clone(),
            stylesheet_hash: stylesheet_hash(&stylesheet),
            source: "import".to_string(),
            report_json: Some(serde_json::to_string(&report).unwrap_or_default()),
            created_at: now_iso(),
            updated_at: now_iso(),
        },
    )
    .await?;
    let record = get_theme(db, &slug).await?;
    Ok(ImportResult {
        theme: record,
        warnings: report["warnings"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        files: sorted_files,
    })
}

/// P3: run the M11 auto-convert over an imported theme's PHP files.
/// Converts each file, stores the generated TypeScript under
/// `themes/<slug>/converted/`, carries literal `get_option()` defaults
/// over as Wordless options (never overwriting existing values), and
/// merges the theme.json settings summary into the report. Returns
/// the `AutoConvertReport` JSON for the import report's `conversion`
/// field. Nothing is executed — generated TypeScript is stored, never
/// run.
async fn auto_convert_theme(
    db: &dyn Db,
    store: &dyn ObjectStore,
    slug: &str,
    files: &[(String, String)],
    theme_json: Option<&Value>,
    unsupported: &[String],
    converter: Option<&dyn ConverterClient>,
) -> Result<Value, WordlessError> {
    let Some(converter) = converter else {
        return Err(internal_error(
            "Conversion service is not configured. Start the wordless-converter worker (local dev: WORDLESS_CONVERTER_URL) or bind it as a service (production: CONVERTER binding).",
        ));
    };
    // One round trip: generated TS + report + literal option defaults.
    let converted = converter
        .convert_files(files, slug, "theme")
        .await
        .map_err(|e| e.to_wordless())?;
    let generated: Vec<(String, String)> =
        serde_json::from_value(converted.get("generated").cloned().unwrap_or(Value::Null))
            .unwrap_or_default();
    for (path, code) in &generated {
        store
            .put_object(
                &format!("themes/{slug}/{path}"),
                code.clone().into_bytes(),
                "text/plain; charset=utf-8",
            )
            .await?;
    }
    let defaults: Vec<Value> = converted
        .get("defaults")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut settings = migrate_option_defaults(db, &defaults)
        .await
        .map_err(WordlessError::from)?;
    if let Some(tj) = theme_json {
        let summary = converter
            .theme_settings(tj, unsupported)
            .await
            .map_err(|e| e.to_wordless())?;
        settings = merge_settings_reports(settings, summary);
    }
    let mut report = converted.get("report").cloned().unwrap_or(Value::Null);
    report["settings"] = settings;
    Ok(report)
}

/// Merge a theme.json settings summary into a conversion report's
/// settings (the `SettingsMigrationReport` wire shape:
/// `carriedOver` + `skipped` arrays concatenated).
fn merge_settings_reports(into: Value, extra: Value) -> Value {
    let mut carried_over = into
        .get("carriedOver")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    carried_over.extend(
        extra
            .get("carriedOver")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
    );
    let mut skipped = into
        .get("skipped")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    skipped.extend(
        extra
            .get("skipped")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
    );
    json!({ "carriedOver": carried_over, "skipped": skipped })
}

/// Import a classic theme from a ZIP upload: stylesheet + analysis report.
///
/// P3: the report's `conversion` field carries the M11 auto-convert
/// record (see [`auto_convert_theme`]), and per-file verdicts come
/// from the PHP analyser (see `classic::analyze_classic_theme`).
pub async fn import_classic_theme(
    db: &dyn Db,
    store: &dyn ObjectStore,
    filename: &str,
    data: &[u8],
    converter: Option<&dyn ConverterClient>,
) -> Result<ClassicImportResult, WordlessError> {
    if !filename.to_lowercase().ends_with(".zip") {
        return Err(bad_request("Theme import requires a .zip file."));
    }
    ensure_default_theme(db).await?;
    let extracted = extract_theme_zip(data)?;
    let style_css = match extracted.get("style.css") {
        Some(bytes) => bytes.to_vec(),
        None => {
            return Err(bad_request(
                "The theme ZIP is missing \"style.css\" at its root.",
            ))
        }
    };
    let text_files: Vec<(String, String)> = extracted
        .files
        .iter()
        .filter(|(path, _)| {
            path.ends_with(".css")
                || path.ends_with(".php")
                || path.ends_with(".json")
                || path.ends_with(".txt")
        })
        .map(|(path, bytes)| (path.clone(), String::from_utf8_lossy(bytes).into_owned()))
        .collect();
    let Some(converter) = converter else {
        return Err(internal_error(
            "Conversion service is not configured. Start the wordless-converter worker (local dev: WORDLESS_CONVERTER_URL) or bind it as a service (production: CONVERTER binding).",
        ));
    };
    let mut report = match analyze_classic_theme(&text_files, converter).await? {
        Some(report) => report,
        None => return Err(bad_request(
            "style.css has no \"Theme Name:\" header — this does not look like a WordPress theme.",
        )),
    };
    let slug = slugify_theme_name(
        if report.header.text_domain.is_empty() {
            &report.header.name
        } else {
            &report.header.text_domain
        },
        "imported-classic-theme",
    );
    if slug == DEFAULT_THEME_SLUG {
        return Err(bad_request(&format!(
            "A theme with slug \"{DEFAULT_THEME_SLUG}\" already exists (the built-in default)."
        )));
    }
    let stylesheet = String::from_utf8_lossy(&style_css).into_owned();
    // P5 F9: v1 returns the auto-convert report on EVERY import, even
    // for a PHP-free classic theme (empty report, no settings).
    report.conversion =
        Some(auto_convert_theme(db, store, &slug, &text_files, None, &[], Some(converter)).await?);
    store_theme_files(store, &slug, &extracted).await?;
    upsert_theme_row(
        db,
        &ThemeRow {
            slug: slug.clone(),
            name: report.header.name.clone(),
            version: report.header.version.clone(),
            author: report.header.author.clone(),
            kind: "classic".to_string(),
            status: "inactive".to_string(),
            theme_json: None,
            stylesheet: stylesheet.clone(),
            stylesheet_hash: stylesheet_hash(&stylesheet),
            source: "import-classic".to_string(),
            report_json: Some(serde_json::to_string(&report).unwrap_or_default()),
            created_at: now_iso(),
            updated_at: now_iso(),
        },
    )
    .await?;
    let record = get_theme(db, &slug).await?;
    Ok(ClassicImportResult {
        theme: record,
        report,
        warnings: extracted.warnings.clone(),
    })
}

pub async fn activate_theme(db: &dyn Db, site_id: i64, slug: &str) -> Result<ThemeRecord, WordlessError> {
    ensure_default_theme(db).await?;
    if get_theme_row(db, slug).await?.is_none() {
        return Err(not_found(&format!("Theme \"{slug}\" is not installed.")));
    }
    let now = now_iso();
    db.exec(
        "UPDATE wl_themes SET status = 'inactive', updated_at = ? WHERE status = 'active'",
        &[Param::Text(now.clone())],
    )
    .await?;
    db.exec(
        "UPDATE wl_themes SET status = 'active', updated_at = ? WHERE slug = ?",
        &[Param::Text(now), Param::Text(slug.to_string())],
    )
    .await?;
    set_option_json(db, site_id, ACTIVE_THEME_OPTION, &Value::String(slug.to_string())).await?;
    get_theme(db, slug).await
}

/// True when `slug` is the active theme on the main site or on
/// any other site (the stored value is the JSON-encoded slug).
async fn theme_active_anywhere(db: &dyn Db, slug: &str) -> Result<bool, WordlessError> {
    if let Some(active) = get_option_json(db, 1, ACTIVE_THEME_OPTION).await? {
        if active.as_str() == Some(slug) {
            return Ok(true);
        }
    }
    let json = serde_json::to_string(&slug).unwrap_or_default();
    let row: Option<Row> = db
        .query_one(
            "SELECT 1 AS x FROM wl_site_options WHERE option_name = 'active_theme' AND option_value = ? LIMIT 1",
            &[Param::Text(json)],
        )
        .await?;
    Ok(row.is_some())
}

pub async fn delete_theme(
    db: &dyn Db,
    store: &dyn ObjectStore,
    slug: &str,
) -> Result<bool, WordlessError> {
    ensure_default_theme(db).await?;
    if slug == DEFAULT_THEME_SLUG {
        return Err(bad_request("The built-in default theme cannot be deleted."));
    }
    // P12: the theme row is network-wide, so deletion refuses when
    // the theme is active on ANY site (network option or any site's
    // override), not just the caller's site.
    if theme_active_anywhere(db, slug).await? {
        return Err(bad_request(
            "Cannot delete the active theme. Activate another theme first.",
        ));
    }
    let prefix = format!("themes/{slug}/");
    let mut cursor: Option<String> = None;
    loop {
        let page = store.list_objects(&prefix, 1000, cursor.as_deref()).await?;
        for meta in &page.objects {
            store.delete_object(&meta.key).await?;
        }
        if !page.truncated {
            break;
        }
        cursor = page.cursor;
        if cursor.is_none() {
            break;
        }
    }
    let meta = db
        .exec(
            "DELETE FROM wl_themes WHERE slug = ?",
            &[Param::Text(slug.to_string())],
        )
        .await?;
    // Variations are per-site: remove the theme's variation on
    // every site, not just the caller's.
    delete_option(db, 1, &crate::customizer::variation_option_key(slug)).await?;
    db.exec(
        "DELETE FROM wl_site_options WHERE option_name = ?",
        &[Param::Text(crate::customizer::variation_option_key(slug))],
    )
    .await?;
    Ok(meta.changes > 0)
}

pub async fn get_theme_detail(
    db: &dyn Db,
    store: &dyn ObjectStore,
    slug: &str,
) -> Result<ThemeDetail, WordlessError> {
    ensure_default_theme(db).await?;
    let row = match get_theme_row(db, slug).await? {
        Some(row) => row,
        None => return Err(not_found(&format!("Theme \"{slug}\" is not installed."))),
    };
    let record = row_to_record(&row);
    let mut theme_json: Option<Value> = None;
    if let Some(raw) = &row.theme_json {
        if !raw.is_empty() {
            theme_json = serde_json::from_str(raw).ok();
        }
    }
    let files_map = load_theme_files(store, &row).await?;
    let mut files = std::collections::BTreeMap::new();
    for (path, content) in files_map {
        files.insert(path, content);
    }
    Ok(ThemeDetail {
        record,
        theme_json,
        stylesheet: row.stylesheet.clone(),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_converter::TestConverter;
    use crate::test_support::{FakeDb, FakeStore};
    use futures_executor::block_on;
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (path, content) in files {
            writer.start_file(*path, opts).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    const MINIMAL_V3: &str = r##"{
      "version": 3,
      "settings": {
        "color": {
          "palette": [
            { "slug": "base", "color": "#FFFFFF", "name": "Base" },
            { "slug": "primary", "color": "#2563EB", "name": "Primary" }
          ]
        },
        "typography": { "fontSizes": [{ "slug": "medium", "size": "1rem", "name": "Medium" }] },
        "layout": { "contentSize": "680px", "wideSize": "1080px" }
      },
      "styles": {
        "color": { "background": "var:preset|color|base", "text": "var:preset|color|contrast" },
        "typography": { "fontFamily": "system-ui, sans-serif", "lineHeight": "1.7" },
        "elements": { "link": { "color": { "text": "var:preset|color|primary" } } }
      }
    }"##;

    const BLOCK_ZIP_STYLE: &str = "/*\nTheme Name: Test Block\nVersion: 1.2.0\nAuthor: Wordless Test\nDescription: A minimal block theme for tests.\nText Domain: test-block\n*/\nbody { color: #111; }\n";

    fn block_theme_zip() -> Vec<u8> {
        zip_of(&[
            ("test-block/style.css", BLOCK_ZIP_STYLE.as_bytes()),
            ("test-block/theme.json", MINIMAL_V3.as_bytes()),
            (
                "test-block/templates/index.html",
                b"<!-- wp:template-part {\"slug\":\"header\"} /-->\n<!-- wp:query --><!-- wp:post-template --><!-- wp:post-title {\"level\":2,\"isLink\":true} /--><!-- /wp:post-template --><!-- /wp:query -->\n<!-- wp:template-part {\"slug\":\"footer\"} /-->",
            ),
            ("test-block/parts/header.html", b"<!-- wp:site-title /-->"),
            (
                "test-block/parts/footer.html",
                b"<!-- wp:paragraph --><p>Footer</p><!-- /wp:paragraph -->",
            ),
        ])
    }

    #[test]
    fn compare_semver_orders_versions() {
        assert_eq!(compare_semver("0.5.0", "0.6.0"), -1);
        assert_eq!(compare_semver("0.6.0", "0.6.0"), 0);
        assert_eq!(compare_semver("9.9.9", "0.6.0"), 1);
        assert_eq!(compare_semver("1.0", "1.0.0"), 0);
        assert_eq!(compare_semver("1.10.0", "1.9.0"), 1);
    }

    #[test]
    fn slugifies_theme_names_like_v1() {
        assert_eq!(slugify_theme_name("Test Block", "fallback"), "test-block");
        assert_eq!(slugify_theme_name("My  Theme!!", "fallback"), "my-theme");
        assert_eq!(slugify_theme_name("!!!", "fallback"), "fallback");
    }

    #[test]
    fn bundled_default_theme_parses_cleanly_as_v3() {
        let value: Value = serde_json::from_str(DEFAULT_THEME_JSON).unwrap();
        let parsed = parse_theme_json(&value);
        assert_eq!(parsed.version, 3);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(DEFAULT_THEME_VERSION, "0.7.0");
        // The bundle matches the v1 repo copy (files embedded, not edited).
        assert!(DEFAULT_THEME_STYLE_CSS.contains("Version: 0.7.0"));
        assert_eq!(DEFAULT_THEME_FILES.len(), 9);
    }

    #[test]
    fn generated_default_theme_css_matches_v1_byte_for_byte() {
        // Golden values from v1's own generateThemeCss on the same
        // theme.json (run in Node against the v1 repo, 2026-11-19):
        // 2346 bytes, cyrb53 hash b2f0cc32848c. The hash covers the
        // full byte content, so this is a byte-for-byte parity check.
        // (Theme 0.7.0: fluid display type, wider measure, generous
        // whitespace — the previous goldens were 2184 / 50ae0b9fe999
        // for the 0.6.0 theme.)
        let value: Value = serde_json::from_str(DEFAULT_THEME_JSON).unwrap();
        let parsed = parse_theme_json(&value);
        let css = generate_theme_css(&parsed);
        assert_eq!(css.len(), 2346);
        assert_eq!(stylesheet_hash(&css), "b2f0cc32848c");
        assert!(css.starts_with(":root {\n  --wp--preset--color--base: #FFFFFF;"));
    }

    #[test]
    fn seeds_builtin_default_theme_on_first_read() {
        let db = FakeDb::new();
        let store = FakeStore::new();
        let themes = block_on(list_themes(&db)).unwrap();
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].slug, "wordless-default");
        assert_eq!(themes[0].status, "active");
        let loaded = block_on(load_active_theme(&db, 1, &store, None)).unwrap();
        assert!(
            loaded
                .stylesheet
                .contains("--wp--preset--color--primary: #0D9488;"),
            "{}",
            loaded.stylesheet
        );
        assert!(loaded
            .files
            .iter()
            .any(|(p, _)| p == "templates/single.html"));
    }

    #[test]
    fn imports_activates_and_deletes_a_block_theme() {
        let db = FakeDb::new();
        let store = FakeStore::new();
        let imported = block_on(import_theme(
            &db,
            &store,
            "test-block.zip",
            &block_theme_zip(),
            Some(&TestConverter),
        ))
        .unwrap();
        assert_eq!(imported.theme.slug, "test-block");
        assert_eq!(imported.theme.kind, "block");
        assert_eq!(imported.theme.status, "inactive");
        assert!(imported.files.contains(&"theme.json".to_string()));
        assert_eq!(store.keys_under("themes/test-block/").len(), 5);

        let active = block_on(activate_theme(&db, 1, "test-block")).unwrap();
        assert_eq!(active.status, "active");
        let loaded = block_on(load_active_theme(&db, 1, &store, None)).unwrap();
        assert_eq!(loaded.record.slug, "test-block");
        assert!(
            loaded
                .stylesheet
                .contains("--wp--preset--color--primary: #2563EB;"),
            "{}",
            loaded.stylesheet
        );
        assert!(
            loaded.stylesheet.contains("body { color: #111; }"),
            "{}",
            loaded.stylesheet
        );

        // Cannot delete the active theme.
        let err = block_on(delete_theme(&db, &store, "test-block")).unwrap_err();
        assert!(err.message.contains("active theme"), "{}", err.message);
        block_on(activate_theme(&db, 1, "wordless-default")).unwrap();
        assert!(block_on(delete_theme(&db, &store, "test-block")).unwrap());
        assert!(block_on(get_theme(&db, "test-block")).is_err());
    }

    #[test]
    fn rejects_block_import_without_theme_json() {
        let db = FakeDb::new();
        let store = FakeStore::new();
        let zip = zip_of(&[("style.css", BLOCK_ZIP_STYLE.as_bytes())]);
        let err = block_on(import_theme(
            &db,
            &store,
            "bad.zip",
            &zip,
            Some(&TestConverter),
        ))
        .unwrap_err();
        assert!(err.message.contains("theme.json"), "{}", err.message);
    }

    #[test]
    fn rejects_non_zip_filename() {
        let db = FakeDb::new();
        let store = FakeStore::new();
        let err = block_on(import_theme(
            &db,
            &store,
            "theme.tar.gz",
            &block_theme_zip(),
            Some(&TestConverter),
        ))
        .unwrap_err();
        assert!(err.message.contains(".zip"), "{}", err.message);
    }

    #[test]
    fn import_report_carries_conversion_record() {
        let db = FakeDb::new();
        let store = FakeStore::new();
        let zip = zip_of(&[
            ("test-block/style.css", BLOCK_ZIP_STYLE.as_bytes()),
            ("test-block/theme.json", MINIMAL_V3.as_bytes()),
            (
                "test-block/functions.php",
                b"<?php\nfunction test_block_setup() {\n  return get_option('test_block_mode', 'standard');\n}\n",
            ),
        ]);
        let imported = block_on(import_theme(
            &db,
            &store,
            "test-block.zip",
            &zip,
            Some(&TestConverter),
        ))
        .unwrap();
        let report = imported.theme.report.as_ref().unwrap();
        assert_eq!(report["kind"], "block-theme-import");
        // P3: the conversion record is filled, the gap warning is gone,
        // the generated TS is stored, and the literal option default is
        // carried over as a Wordless option.
        let conversion = &report["conversion"];
        assert!(conversion.is_object(), "{conversion}");
        assert_eq!(conversion["phpFiles"], 1);
        assert_eq!(conversion["converted"], 1);
        assert_eq!(conversion["partial"], 0);
        assert_eq!(conversion["blocked"], 0);
        assert!(
            !imported
                .warnings
                .iter()
                .any(|w| w.contains("auto-conversion is not available")),
            "{:?}",
            imported.warnings
        );
        assert!(
            store
                .keys()
                .contains(&"themes/test-block/converted/functions.ts".to_string()),
            "generated TS not stored: {:?}",
            store.keys()
        );
        // The fake has no `prepare` (service code uses the primitive
        // methods), so read the carried-over option with `query_one`.
        let row: Option<Row> = block_on(db.query_one(
            "SELECT option_value FROM wp_options WHERE option_name = ? LIMIT 1",
            &[Param::Text("test_block_mode".to_string())],
        ))
        .unwrap();
        let raw = row.map(|r| wordless_db::row::row_text(&r, "option_value"));
        assert_eq!(raw.as_deref(), Some("\"standard\""));
    }

    #[test]
    fn refreshes_stale_builtin_row_when_bundled_theme_is_newer() {
        let db = FakeDb::new();
        let now = "2026-01-01T00:00:00.000Z";
        block_on(insert_theme_row(
            &db,
            &ThemeRow {
                slug: "wordless-default".to_string(),
                name: "wordless-default".to_string(),
                version: "0.5.0".to_string(),
                author: "Wordless".to_string(),
                kind: "block".to_string(),
                status: "active".to_string(),
                theme_json: Some("{}".to_string()),
                stylesheet: "/* stale */".to_string(),
                stylesheet_hash: "stale-hash".to_string(),
                source: "builtin".to_string(),
                report_json: None,
                created_at: now.to_string(),
                updated_at: now.to_string(),
            },
        ))
        .unwrap();
        let themes = block_on(list_themes(&db)).unwrap();
        let def = themes
            .iter()
            .find(|t| t.slug == "wordless-default")
            .unwrap();
        assert_eq!(def.version, DEFAULT_THEME_VERSION);
        let stored: Option<Row> = block_on(db.query_one(
            "SELECT * FROM wl_themes WHERE slug = ? LIMIT 1",
            &[Param::Text("wordless-default".to_string())],
        ))
        .unwrap();
        let stored = stored.unwrap();
        assert!(row_text(&stored, "stylesheet").contains(".post-card"));
        assert_ne!(row_text(&stored, "stylesheet_hash"), "stale-hash");
    }

    #[test]
    fn never_downgrades_builtin_row_newer_than_bundle() {
        let db = FakeDb::new();
        let now = "2026-01-01T00:00:00.000Z";
        block_on(insert_theme_row(
            &db,
            &ThemeRow {
                slug: "wordless-default".to_string(),
                name: "wordless-default".to_string(),
                version: "9.9.9".to_string(),
                author: "Wordless".to_string(),
                kind: "block".to_string(),
                status: "active".to_string(),
                theme_json: Some("{}".to_string()),
                stylesheet: "/* newer */".to_string(),
                stylesheet_hash: "newer-hash".to_string(),
                source: "builtin".to_string(),
                report_json: None,
                created_at: now.to_string(),
                updated_at: now.to_string(),
            },
        ))
        .unwrap();
        block_on(list_themes(&db)).unwrap();
        let stored: Option<Row> = block_on(db.query_one(
            "SELECT * FROM wl_themes WHERE slug = ? LIMIT 1",
            &[Param::Text("wordless-default".to_string())],
        ))
        .unwrap();
        let stored = stored.unwrap();
        assert_eq!(row_text(&stored, "version"), "9.9.9");
        assert_eq!(row_text(&stored, "stylesheet"), "/* newer */");
    }

    #[test]
    fn leaves_imported_themes_alone_on_refresh() {
        let db = FakeDb::new();
        let now = "2026-01-01T00:00:00.000Z";
        for (slug, version, stylesheet, source) in [
            ("wordless-default", "0.5.0", "/* stale */", "builtin"),
            ("custom", "1.0.0", "/* custom */", "imported"),
        ] {
            block_on(insert_theme_row(
                &db,
                &ThemeRow {
                    slug: slug.to_string(),
                    name: slug.to_string(),
                    version: version.to_string(),
                    author: "Wordless".to_string(),
                    kind: "block".to_string(),
                    status: "active".to_string(),
                    theme_json: Some("{}".to_string()),
                    stylesheet: stylesheet.to_string(),
                    stylesheet_hash: "h".to_string(),
                    source: source.to_string(),
                    report_json: None,
                    created_at: now.to_string(),
                    updated_at: now.to_string(),
                },
            ))
            .unwrap();
        }
        block_on(list_themes(&db)).unwrap();
        let custom: Option<Row> = block_on(db.query_one(
            "SELECT * FROM wl_themes WHERE slug = ? LIMIT 1",
            &[Param::Text("custom".to_string())],
        ))
        .unwrap();
        let custom = custom.unwrap();
        assert_eq!(row_text(&custom, "version"), "1.0.0");
        assert_eq!(row_text(&custom, "stylesheet"), "/* custom */");
        let def: Option<Row> = block_on(db.query_one(
            "SELECT * FROM wl_themes WHERE slug = ? LIMIT 1",
            &[Param::Text("wordless-default".to_string())],
        ))
        .unwrap();
        assert_eq!(row_text(&def.unwrap(), "version"), DEFAULT_THEME_VERSION);
    }

    #[test]
    fn imports_classic_theme_stylesheet_plus_report() {
        let db = FakeDb::new();
        let store = FakeStore::new();
        let zip = zip_of(&[
            (
                "style.css",
                b"/*\nTheme Name: Classic Sample\nVersion: 1.0.0\nAuthor: Wordless Test\nText Domain: classic-sample\n*/\nbody { background: #fff; }\n",
            ),
            ("index.php", b"<?php\nget_header();\nif ( have_posts() ) {\n\twhile ( have_posts() ) {\n\t\tthe_post();\n\t\tthe_content();\n\t}\n}\nget_footer();\n"),
            ("single.php", b"<?php\nget_header();\nthe_post();\nthe_content();\ncomments_template();\nget_footer();\n"),
            (
                "functions.php",
                b"<?php\nfunction classic_sample_setup() {\n\tadd_theme_support( 'title-tag' );\n}\nadd_action( 'after_setup_theme', 'classic_sample_setup' );\n",
            ),
        ]);
        let result = block_on(import_classic_theme(
            &db,
            &store,
            "classic-sample.zip",
            &zip,
            Some(&TestConverter),
        ))
        .unwrap();
        assert_eq!(result.theme.slug, "classic-sample");
        assert_eq!(result.theme.kind, "classic");
        assert_eq!(result.report.files.len(), 3);
        assert!(!result.theme.stylesheet_hash.is_empty());
        let index = result
            .report
            .files
            .iter()
            .find(|f| f.file == "index.php")
            .unwrap();
        assert!(index.template_tags.contains(&"get_header".to_string()));
        assert!(index.template_tags.contains(&"have_posts".to_string()));
        assert!(index.template_tags.contains(&"the_content".to_string()));
        // Active theme is still the default until activation.
        let loaded = block_on(load_active_theme(&db, 1, &store, None)).unwrap();
        assert_eq!(loaded.record.slug, "wordless-default");
        block_on(activate_theme(&db, 1, "classic-sample")).unwrap();
        let classic_loaded = block_on(load_active_theme(&db, 1, &store, None)).unwrap();
        assert_eq!(classic_loaded.record.kind, "classic");
        assert!(
            classic_loaded.stylesheet.contains("background: #fff;"),
            "{}",
            classic_loaded.stylesheet
        );
    }
}
