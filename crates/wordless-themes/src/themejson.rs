//! theme.json engine (P2) — port of v1 `src/modules/themes/themejson.ts`.
//!
//! Parses + validates theme.json versions 1–3, resolves presets to CSS
//! custom properties, and generates the theme stylesheet. Pure functions —
//! no D1, no R2 — so they are unit-testable in isolation.
//!
//! Support matrix (same as v1, documented in v1
//! `docs/themes/theme-support.md`):
//! - v3: everything below is supported.
//! - v2: everything below EXCEPT `settings.typography.writingMode` and
//!   per-block `elements` inside `styles.blocks.*` (v3 additions) — those
//!   are recorded as unsupported warnings, not errors.
//! - v1: partial — `customTemplates`, `templateParts`, `patterns`,
//!   per-block settings (`settings.blocks`), per-block styles
//!   (`styles.blocks`) and `settings.custom` are NOT supported; they are
//!   recorded as unsupported warnings and ignored.
//!
//! `patterns` in theme.json are recorded, never executed: the list names
//! reusable block fragments. Rendering a pattern happens from the theme's
//! `patterns/<name>.php` file (see `patterns.rs`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A theme.json schema version.
pub type ThemeJsonVersion = u8;

/// One validation finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeJsonIssue {
    pub path: String,
    pub message: String,
}

fn issue(path: &str, message: &str) -> ThemeJsonIssue {
    ThemeJsonIssue {
        path: path.to_string(),
        message: message.to_string(),
    }
}

/// The result of parsing + validating a theme.json value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedThemeJson {
    pub version: ThemeJsonVersion,
    /// The raw theme.json object (settings/styles as parsed).
    pub theme: Value,
    pub settings: Value,
    pub styles: Value,
    #[serde(rename = "customTemplates")]
    pub custom_templates: Vec<Value>,
    #[serde(rename = "templateParts")]
    pub template_parts: Vec<Value>,
    pub patterns: Vec<String>,
    pub errors: Vec<ThemeJsonIssue>,
    pub warnings: Vec<ThemeJsonIssue>,
    /// Feature names the engine does not support for this version.
    pub unsupported: Vec<String>,
}

fn is_object(value: &Value) -> bool {
    value.is_object()
}

fn obj(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn is_slug(value: &Value) -> bool {
    match value.as_str() {
        Some(s) => {
            let b = s.as_bytes();
            !b.is_empty()
                && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
                && b.iter()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        }
        None => false,
    }
}

fn is_hex_color(hex: &str) -> bool {
    (hex.len() == 3 || hex.len() == 6 || hex.len() == 8)
        && hex.bytes().all(|c| c.is_ascii_hexdigit())
}

fn is_color(value: &Value) -> bool {
    let Some(s) = value.as_str() else {
        return false;
    };
    let v = s.trim();
    if v.is_empty() {
        return false;
    }
    if let Some(hex) = v.strip_prefix('#') {
        return is_hex_color(hex);
    }
    // Functional colours (rgb()/hsl()/color-mix()/var()) and named colours.
    v.contains('(') || v.bytes().all(|c| c.is_ascii_alphabetic())
}

fn css_var_name(parts: &[&str]) -> String {
    format!("--wp--{}", parts.join("--"))
}

/// `var:preset|color|accent-1` → `var(--wp--preset--color--accent-1)`.
pub fn resolve_preset_value(value: &str) -> String {
    let trimmed = value.trim();
    let Some(rest) = trimmed.strip_prefix("var:preset|") else {
        return trimmed.to_string();
    };
    let parts: Vec<&str> = rest.split('|').collect();
    let Some(kind) = parts.first() else {
        return trimmed.to_string();
    };
    let rest_parts = &parts[1..];
    if rest_parts.is_empty() {
        return trimmed.to_string();
    }
    let mut all: Vec<&str> = if *kind == "custom" {
        vec!["custom"]
    } else {
        vec!["preset", kind]
    };
    all.extend_from_slice(rest_parts);
    format!("var({})", css_var_name(&all))
}

/// cyrb53 — small deterministic sync hash (URL versioning, not security).
/// Ported bit-for-bit from the v1 implementation: JS `Math.imul` is a
/// 32-bit wrapping multiply and `>>>` is an unsigned shift, so the
/// arithmetic is done on `u32`; `String.length` / `charCodeAt` count
/// UTF-16 code units, hence the `encode_utf16` iteration.
pub fn stylesheet_hash(css: &str) -> String {
    let units: Vec<u16> = css.encode_utf16().collect();
    let len = units.len() as u32;
    let mut h1: u32 = 0xdeadbeef ^ len;
    let mut h2: u32 = 0x41c6ce57 ^ len;
    for ch in units {
        h1 = (h1 ^ u32::from(ch)).wrapping_mul(2_654_435_761);
        h2 = (h2 ^ u32::from(ch)).wrapping_mul(1_597_334_677);
    }
    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h2 ^ (h2 >> 13)).wrapping_mul(3_266_489_909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h1 ^ (h1 >> 13)).wrapping_mul(3_266_489_909);
    let combined = format!("{h2:08x}{h1:08x}");
    combined[..12].to_string()
}

#[derive(Default)]
struct Validator {
    errors: Vec<ThemeJsonIssue>,
    warnings: Vec<ThemeJsonIssue>,
    unsupported: Vec<String>,
}

fn check_slug_entry(v: &mut Validator, entry: &Value, path: &str, fields: &[&str]) {
    let Some(entry_obj) = obj(entry) else {
        v.errors.push(issue(path, "Must be an object."));
        return;
    };
    if !is_slug(entry.get("slug").unwrap_or(&Value::Null)) {
        v.errors.push(issue(
            &format!("{path}.slug"),
            "Must be a lowercase slug (letters, digits, hyphens).",
        ));
    }
    for field in fields {
        match entry_obj.get(*field) {
            None => v.errors.push(issue(
                &format!("{path}.{field}"),
                &format!("Missing required field \"{field}\"."),
            )),
            Some(value) => {
                if !value.is_string() || value.as_str().unwrap_or("").trim().is_empty() {
                    v.errors.push(issue(
                        &format!("{path}.{field}"),
                        "Must be a non-empty string.",
                    ));
                }
            }
        }
    }
}

fn validate_palette(v: &mut Validator, palette: &Value, path: &str) {
    let Some(entries) = palette.as_array() else {
        v.errors.push(issue(
            path,
            "Must be an array of {slug, color, name} entries.",
        ));
        return;
    };
    for (i, entry) in entries.iter().enumerate() {
        let p = format!("{path}[{i}]");
        check_slug_entry(v, entry, &p, &["name"]);
        if let Some(entry_obj) = obj(entry) {
            match entry_obj.get("color") {
                Some(color) => {
                    if !is_color(color) {
                        v.errors.push(issue(
                            &format!("{p}.color"),
                            "Must be a CSS colour (#hex, rgb(), hsl(), color-mix(), var() or a named colour).",
                        ));
                    }
                }
                None => v.errors.push(issue(
                    &format!("{p}.color"),
                    "Missing required field \"color\".",
                )),
            }
        }
    }
}

fn validate_settings(v: &mut Validator, settings: &Value, version: ThemeJsonVersion, path: &str) {
    let Some(settings_obj) = obj(settings) else {
        v.errors.push(issue(path, "Must be an object."));
        return;
    };
    if let Some(color) = settings_obj.get("color") {
        if let Some(color_obj) = obj(color) {
            if let Some(palette) = color_obj.get("palette") {
                validate_palette(v, palette, &format!("{path}.color.palette"));
            }
            if let Some(gradients) = color_obj.get("gradients") {
                if let Some(entries) = gradients.as_array() {
                    for (i, entry) in entries.iter().enumerate() {
                        check_slug_entry(
                            v,
                            entry,
                            &format!("{path}.color.gradients[{i}]"),
                            &["gradient", "name"],
                        );
                    }
                } else {
                    v.errors.push(issue(
                        &format!("{path}.color.gradients"),
                        "Must be an array of {slug, gradient, name} entries.",
                    ));
                }
            }
            if let Some(duotone) = color_obj.get("duotone") {
                if let Some(entries) = duotone.as_array() {
                    for (i, entry) in entries.iter().enumerate() {
                        let p = format!("{path}.color.duotone[{i}]");
                        check_slug_entry(v, entry, &p, &["name"]);
                        if let Some(entry_obj) = obj(entry) {
                            if !entry_obj
                                .get("colors")
                                .map(|c| c.is_array())
                                .unwrap_or(false)
                            {
                                v.errors.push(issue(
                                    &format!("{p}.colors"),
                                    "Must be an array of colours.",
                                ));
                            }
                        }
                    }
                } else {
                    v.errors.push(issue(
                        &format!("{path}.color.duotone"),
                        "Must be an array of {slug, colors, name} entries.",
                    ));
                }
            }
        } else {
            v.errors
                .push(issue(&format!("{path}.color"), "Must be an object."));
        }
    }
    if let Some(typography) = settings_obj.get("typography") {
        if let Some(typo_obj) = obj(typography) {
            if typo_obj.contains_key("writingMode") && version < 3 {
                v.unsupported
                    .push("settings.typography.writingMode (theme.json v3 only)".to_string());
                v.warnings.push(issue(
                    &format!("{path}.typography.writingMode"),
                    "Not supported before theme.json v3; ignored.",
                ));
            }
            if let Some(font_sizes) = typo_obj.get("fontSizes") {
                if let Some(entries) = font_sizes.as_array() {
                    for (i, entry) in entries.iter().enumerate() {
                        let p = format!("{path}.typography.fontSizes[{i}]");
                        check_slug_entry(v, entry, &p, &["name"]);
                        if let Some(entry_obj) = obj(entry) {
                            match entry_obj.get("size") {
                                None => v.errors.push(issue(
                                    &format!("{p}.size"),
                                    "Missing required field \"size\".",
                                )),
                                Some(size) => {
                                    if size.is_string()
                                        && size.as_str().unwrap_or("").trim().is_empty()
                                    {
                                        v.errors.push(issue(
                                            &format!("{p}.size"),
                                            "Must be a non-empty size.",
                                        ));
                                    } else if !size.is_string() && !size.is_number() {
                                        v.errors.push(issue(
                                            &format!("{p}.size"),
                                            "Must be a size string (e.g. \"1rem\") or a number.",
                                        ));
                                    }
                                }
                            }
                            if let Some(fluid) = entry_obj.get("fluid") {
                                if fluid != &Value::Bool(false) && !is_object(fluid) {
                                    v.errors.push(issue(
                                        &format!("{p}.fluid"),
                                        "Must be false or an object {min, max}.",
                                    ));
                                } else if let Some(fluid_obj) = obj(fluid) {
                                    let min_ok = fluid_obj
                                        .get("min")
                                        .map(|m| m.is_string())
                                        .unwrap_or(false);
                                    let max_ok = fluid_obj
                                        .get("max")
                                        .map(|m| m.is_string())
                                        .unwrap_or(false);
                                    if !min_ok || !max_ok {
                                        v.errors.push(issue(
                                            &format!("{p}.fluid"),
                                            "Fluid sizes need string \"min\" and \"max\" (e.g. \"1rem\" / \"1.5rem\").",
                                        ));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    v.errors.push(issue(
                        &format!("{path}.typography.fontSizes"),
                        "Must be an array of {slug, size, name} entries.",
                    ));
                }
            }
            if let Some(font_families) = typo_obj.get("fontFamilies") {
                if let Some(entries) = font_families.as_array() {
                    for (i, entry) in entries.iter().enumerate() {
                        check_slug_entry(
                            v,
                            entry,
                            &format!("{path}.typography.fontFamilies[{i}]"),
                            &["fontFamily", "name"],
                        );
                    }
                } else {
                    v.errors.push(issue(
                        &format!("{path}.typography.fontFamilies"),
                        "Must be an array of {slug, fontFamily, name} entries.",
                    ));
                }
            }
        } else {
            v.errors
                .push(issue(&format!("{path}.typography"), "Must be an object."));
        }
    }
    if let Some(spacing) = settings_obj.get("spacing") {
        if let Some(spacing_obj) = obj(spacing) {
            if let Some(spacing_sizes) = spacing_obj.get("spacingSizes") {
                if let Some(entries) = spacing_sizes.as_array() {
                    for (i, entry) in entries.iter().enumerate() {
                        check_slug_entry(
                            v,
                            entry,
                            &format!("{path}.spacing.spacingSizes[{i}]"),
                            &["size", "name"],
                        );
                    }
                } else {
                    v.errors.push(issue(
                        &format!("{path}.spacing.spacingSizes"),
                        "Must be an array of {slug, size, name} entries.",
                    ));
                }
            }
            if let Some(units) = spacing_obj.get("units") {
                if !units.is_array() {
                    v.errors.push(issue(
                        &format!("{path}.spacing.units"),
                        "Must be an array of unit strings.",
                    ));
                }
            }
        } else {
            v.errors
                .push(issue(&format!("{path}.spacing"), "Must be an object."));
        }
    }
    if let Some(layout) = settings_obj.get("layout") {
        if let Some(layout_obj) = obj(layout) {
            for key in ["contentSize", "wideSize"] {
                if let Some(value) = layout_obj.get(key) {
                    if !value.is_string() {
                        v.errors.push(issue(
                            &format!("{path}.layout.{key}"),
                            "Must be a size string (e.g. \"680px\").",
                        ));
                    }
                }
            }
        } else {
            v.errors
                .push(issue(&format!("{path}.layout"), "Must be an object."));
        }
    }
    if let Some(custom) = settings_obj.get("custom") {
        if version < 2 {
            v.unsupported
                .push("settings.custom (theme.json v2+)".to_string());
            v.warnings.push(issue(
                &format!("{path}.custom"),
                "Not supported in theme.json v1; ignored.",
            ));
        } else if !is_object(custom) {
            v.errors.push(issue(
                &format!("{path}.custom"),
                "Must be an object of custom values.",
            ));
        }
    }
    if settings_obj.contains_key("blocks") && version < 2 {
        v.unsupported
            .push("settings.blocks (theme.json v2+)".to_string());
        v.warnings.push(issue(
            &format!("{path}.blocks"),
            "Per-block settings are not supported in theme.json v1; ignored.",
        ));
    }
}

fn validate_style_object(
    v: &mut Validator,
    styles: &Value,
    path: &str,
    _version: ThemeJsonVersion,
    allow_elements: bool,
) {
    let Some(styles_obj) = obj(styles) else {
        v.errors.push(issue(path, "Must be an object."));
        return;
    };
    for key in ["color", "typography", "spacing"] {
        if let Some(value) = styles_obj.get(key) {
            if !is_object(value) {
                v.errors
                    .push(issue(&format!("{path}.{key}"), "Must be an object."));
            }
        }
    }
    if let Some(elements) = styles_obj.get("elements") {
        if !allow_elements {
            v.unsupported
                .push("styles.elements (theme.json v2+)".to_string());
            v.warnings.push(issue(
                &format!("{path}.elements"),
                "Element styles are not supported in theme.json v1; ignored.",
            ));
        } else if let Some(elements_obj) = obj(elements) {
            for (name, el) in elements_obj {
                if !is_object(el) {
                    v.errors.push(issue(
                        &format!("{path}.elements.{name}"),
                        "Must be a style object.",
                    ));
                }
            }
        } else {
            v.errors.push(issue(
                &format!("{path}.elements"),
                "Must be an object keyed by element name (link, heading, button, h1–h6).",
            ));
        }
    }
}

fn validate_styles(v: &mut Validator, styles: &Value, version: ThemeJsonVersion, path: &str) {
    let Some(styles_obj) = obj(styles) else {
        v.errors.push(issue(path, "Must be an object."));
        return;
    };
    validate_style_object(v, styles, path, version, version >= 2);
    if let Some(blocks) = styles_obj.get("blocks") {
        if version < 2 {
            v.unsupported
                .push("styles.blocks (theme.json v2+)".to_string());
            v.warnings.push(issue(
                &format!("{path}.blocks"),
                "Per-block styles are not supported in theme.json v1; ignored.",
            ));
        } else if let Some(blocks_obj) = obj(blocks) {
            for (name, block_styles) in blocks_obj {
                let block_path = format!("{path}.blocks.{name}");
                let Some(block_obj) = obj(block_styles) else {
                    v.errors.push(issue(&block_path, "Must be a style object."));
                    continue;
                };
                // Per-block `elements` arrived with theme.json v3.
                if block_obj.contains_key("elements") && version < 3 {
                    v.unsupported.push(format!(
                        "styles.blocks.{name}.elements (theme.json v3 only)"
                    ));
                    v.warnings.push(issue(
                        &format!("{block_path}.elements"),
                        "Per-block element styles are not supported before theme.json v3; ignored.",
                    ));
                }
                let mut rest = Map::new();
                for (k, val) in block_obj {
                    if k != "elements" {
                        rest.insert(k.clone(), val.clone());
                    }
                }
                validate_style_object(v, &Value::Object(rest), &block_path, version, true);
            }
        } else {
            v.errors.push(issue(
                &format!("{path}.blocks"),
                "Must be an object keyed by block name.",
            ));
        }
    }
}

/// Validate a partial `settings` object (customizer variations reuse this).
pub fn validate_theme_settings(settings: &Value, version: ThemeJsonVersion) -> Vec<ThemeJsonIssue> {
    let mut v = Validator::default();
    if !is_object(settings) {
        return vec![issue("settings", "Must be an object.")];
    }
    validate_settings(&mut v, settings, version, "settings");
    v.errors
}

/// Parse + validate a theme.json value. Never throws on bad input.
pub fn parse_theme_json(raw: &Value) -> ParsedThemeJson {
    let mut v = Validator::default();
    let Some(raw_obj) = obj(raw) else {
        return ParsedThemeJson {
            version: 3,
            theme: Value::Object(Map::new()),
            settings: Value::Object(Map::new()),
            styles: Value::Object(Map::new()),
            custom_templates: Vec::new(),
            template_parts: Vec::new(),
            patterns: Vec::new(),
            errors: vec![issue("", "theme.json must be a JSON object.")],
            warnings: Vec::new(),
            unsupported: Vec::new(),
        };
    };
    let version_raw = raw_obj.get("version");
    let version_num = version_raw.and_then(|n| n.as_f64()).unwrap_or(f64::NAN);
    let version: ThemeJsonVersion =
        if (1.0..=3.0).contains(&version_num) && version_num.fract() == 0.0 {
            version_num as u8
        } else {
            3
        };
    if !((1.0..=3.0).contains(&version_num) && version_num.fract() == 0.0) {
        v.errors.push(issue("version", "Must be 1, 2 or 3."));
    }
    let settings = match raw_obj.get("settings") {
        Some(s) if is_object(s) => {
            validate_settings(&mut v, s, version, "settings");
            s.clone()
        }
        Some(_) => {
            v.errors.push(issue("settings", "Must be an object."));
            Value::Object(Map::new())
        }
        None => Value::Object(Map::new()),
    };
    let styles = match raw_obj.get("styles") {
        Some(s) if is_object(s) => {
            validate_styles(&mut v, s, version, "styles");
            s.clone()
        }
        Some(_) => {
            v.errors.push(issue("styles", "Must be an object."));
            Value::Object(Map::new())
        }
        None => Value::Object(Map::new()),
    };
    let custom_templates = match raw_obj.get("customTemplates") {
        Some(Value::Array(entries)) => {
            if version < 2 {
                v.unsupported
                    .push("customTemplates (theme.json v2+)".to_string());
                v.warnings.push(issue(
                    "customTemplates",
                    "Not supported in theme.json v1; ignored.",
                ));
            }
            entries.iter().filter(|t| is_object(t)).cloned().collect()
        }
        Some(_) => {
            v.errors.push(issue("customTemplates", "Must be an array."));
            Vec::new()
        }
        None => Vec::new(),
    };
    let template_parts = match raw_obj.get("templateParts") {
        Some(Value::Array(entries)) => {
            if version < 2 {
                v.unsupported
                    .push("templateParts (theme.json v2+)".to_string());
                v.warnings.push(issue(
                    "templateParts",
                    "Not supported in theme.json v1; ignored.",
                ));
            }
            entries.iter().filter(|t| is_object(t)).cloned().collect()
        }
        Some(_) => {
            v.errors.push(issue("templateParts", "Must be an array."));
            Vec::new()
        }
        None => Vec::new(),
    };
    let mut patterns: Vec<String> = Vec::new();
    if let Some(p) = raw_obj.get("patterns") {
        match p {
            Value::Array(entries) => {
                patterns = entries
                    .iter()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect();
            }
            _ => {
                v.errors
                    .push(issue("patterns", "Must be an array of pattern slugs."));
            }
        }
        if version < 2 {
            v.unsupported.push("patterns (theme.json v2+)".to_string());
            v.warnings.push(issue(
                "patterns",
                "Not supported in theme.json v1; ignored.",
            ));
        }
    }
    for key in raw_obj.keys() {
        if ![
            "$schema",
            "version",
            "settings",
            "styles",
            "customTemplates",
            "templateParts",
            "patterns",
        ]
        .contains(&key.as_str())
        {
            v.warnings.push(issue(
                key,
                &format!("Unknown top-level key \"{key}\"; ignored."),
            ));
        }
    }
    let mut unsupported: Vec<String> = Vec::new();
    for item in v.unsupported {
        if !unsupported.contains(&item) {
            unsupported.push(item);
        }
    }
    ParsedThemeJson {
        version,
        theme: raw.clone(),
        settings,
        styles,
        custom_templates,
        template_parts,
        patterns,
        errors: v.errors,
        warnings: v.warnings,
        unsupported,
    }
}

// ------------------------------------------------------------------ CSS output

/// Render a JSON scalar the way JS string interpolation does: a string
/// prints bare, a number prints as a number (never JSON-quoted).
fn css_scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn preset_size(size: &Value, font_sizes: &[Value]) -> Option<String> {
    let size_str = size.as_str()?;
    if size_str.starts_with("var:") {
        return Some(resolve_preset_value(size_str));
    }
    let found = font_sizes.iter().any(|f| {
        obj(f)
            .and_then(|o| o.get("slug"))
            .and_then(|s| s.as_str())
            .map(|s| s == size_str)
            .unwrap_or(false)
    });
    if found {
        return Some(format!("var(--wp--preset--font-size--{size_str})"));
    }
    Some(size_str.to_string())
}

fn fluid_clamp(min: &str, max: &str) -> String {
    // WordPress fluid formula, fixed viewport range 20rem–75rem.
    format!("clamp({min}, calc({min} + ({max} - {min}) * ((100vw - 20rem) / 55rem)), {max})")
}

fn flatten_custom(map: &Map<String, Value>, prefix: &[String], out: &mut Vec<(String, String)>) {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for key in keys {
        let value = &map[key];
        let mut path: Vec<String> = prefix.to_vec();
        path.push(key.clone());
        if let Some(inner) = obj(value) {
            flatten_custom(inner, &path, out);
        } else if value.is_string() || value.is_number() {
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => unreachable!(),
            };
            out.push((format!("--wp--custom--{}", path.join("--")), text));
        }
    }
}

fn style_props(style: &Value, font_sizes: &[Value]) -> Vec<String> {
    let mut props: Vec<String> = Vec::new();
    let style_obj = obj(style).cloned().unwrap_or_default();
    let color = style_obj
        .get("color")
        .and_then(|c| obj(c).cloned())
        .unwrap_or_default();
    if let Some(Value::String(background)) = color.get("background") {
        props.push(format!(
            "background-color: {}",
            resolve_preset_value(background)
        ));
    }
    if let Some(Value::String(gradient)) = color.get("gradient") {
        props.push(format!("background: {}", resolve_preset_value(gradient)));
    }
    if let Some(Value::String(text)) = color.get("text") {
        props.push(format!("color: {}", resolve_preset_value(text)));
    }
    let typo = style_obj
        .get("typography")
        .and_then(|t| obj(t).cloned())
        .unwrap_or_default();
    if let Some(Value::String(font_family)) = typo.get("fontFamily") {
        props.push(format!(
            "font-family: {}",
            resolve_preset_value(font_family)
        ));
    }
    if let Some(font_size) = typo.get("fontSize") {
        if let Some(resolved) = preset_size(font_size, font_sizes) {
            props.push(format!("font-size: {resolved}"));
        }
    }
    if let Some(line_height) = typo.get("lineHeight") {
        if line_height.is_string() || line_height.is_number() {
            props.push(format!("line-height: {}", css_scalar(line_height)));
        }
    }
    if let Some(font_weight) = typo.get("fontWeight") {
        if font_weight.is_string() || font_weight.is_number() {
            props.push(format!("font-weight: {}", css_scalar(font_weight)));
        }
    }
    if let Some(Value::String(font_style)) = typo.get("fontStyle") {
        props.push(format!("font-style: {font_style}"));
    }
    if let Some(Value::String(letter_spacing)) = typo.get("letterSpacing") {
        props.push(format!("letter-spacing: {letter_spacing}"));
    }
    if let Some(Value::String(text_transform)) = typo.get("textTransform") {
        props.push(format!("text-transform: {text_transform}"));
    }
    if let Some(Value::String(text_decoration)) = typo.get("textDecoration") {
        props.push(format!("text-decoration: {text_decoration}"));
    }
    let spacing = style_obj
        .get("spacing")
        .and_then(|s| obj(s).cloned())
        .unwrap_or_default();
    for key in ["padding", "margin"] {
        match spacing.get(key) {
            Some(Value::String(value)) => {
                props.push(format!("{key}: {}", resolve_preset_value(value)));
            }
            Some(Value::Object(sides)) => {
                for side in ["top", "right", "bottom", "left"] {
                    if let Some(Value::String(value)) = sides.get(side) {
                        props.push(format!("{key}-{side}: {}", resolve_preset_value(value)));
                    }
                }
            }
            _ => {}
        }
    }
    props
}

fn element_props(el: &Value, font_sizes: &[Value]) -> Vec<String> {
    style_props(el, font_sizes)
}

/// Generate the deterministic stylesheet for a parsed theme.json.
pub fn generate_theme_css(parsed: &ParsedThemeJson) -> String {
    let settings = &parsed.settings;
    let styles = &parsed.styles;
    let mut lines: Vec<String> = Vec::new();
    let mut root_vars: Vec<String> = Vec::new();

    let settings_obj = obj(settings).cloned().unwrap_or_default();
    let color = settings_obj
        .get("color")
        .and_then(|c| obj(c).cloned())
        .unwrap_or_default();
    if let Some(Value::Array(palette)) = color.get("palette") {
        for entry in palette {
            if let Some(entry_obj) = obj(entry) {
                if is_slug(entry_obj.get("slug").unwrap_or(&Value::Null)) {
                    if let Some(Value::String(c)) = entry_obj.get("color") {
                        let slug = entry_obj.get("slug").unwrap().as_str().unwrap();
                        root_vars.push(format!(
                            "  {}: {c};",
                            css_var_name(&["preset", "color", slug])
                        ));
                    }
                }
            }
        }
    }
    if let Some(Value::Array(gradients)) = color.get("gradients") {
        for entry in gradients {
            if let Some(entry_obj) = obj(entry) {
                if is_slug(entry_obj.get("slug").unwrap_or(&Value::Null)) {
                    if let Some(Value::String(g)) = entry_obj.get("gradient") {
                        let slug = entry_obj.get("slug").unwrap().as_str().unwrap();
                        root_vars.push(format!(
                            "  {}: {g};",
                            css_var_name(&["preset", "gradient", slug])
                        ));
                    }
                }
            }
        }
    }
    let typo = settings_obj
        .get("typography")
        .and_then(|t| obj(t).cloned())
        .unwrap_or_default();
    let font_sizes: Vec<Value> = match typo.get("fontSizes") {
        Some(Value::Array(entries)) => entries.clone(),
        _ => Vec::new(),
    };
    for entry in &font_sizes {
        let Some(entry_obj) = obj(entry) else {
            continue;
        };
        if !is_slug(entry_obj.get("slug").unwrap_or(&Value::Null)) {
            continue;
        }
        let slug = entry_obj.get("slug").unwrap().as_str().unwrap();
        if let Some(fluid_obj) = entry_obj.get("fluid").and_then(obj) {
            if let (Some(Value::String(min)), Some(Value::String(max))) =
                (fluid_obj.get("min"), fluid_obj.get("max"))
            {
                root_vars.push(format!(
                    "  {}: {};",
                    css_var_name(&["preset", "font-size", slug]),
                    fluid_clamp(min, max)
                ));
                continue;
            }
        }
        if let Some(size) = entry_obj.get("size") {
            if size.is_string() || size.is_number() {
                root_vars.push(format!(
                    "  {}: {};",
                    css_var_name(&["preset", "font-size", slug]),
                    css_scalar(size)
                ));
            }
        }
    }
    if let Some(Value::Array(font_families)) = typo.get("fontFamilies") {
        for entry in font_families {
            if let Some(entry_obj) = obj(entry) {
                if is_slug(entry_obj.get("slug").unwrap_or(&Value::Null)) {
                    if let Some(Value::String(family)) = entry_obj.get("fontFamily") {
                        let slug = entry_obj.get("slug").unwrap().as_str().unwrap();
                        root_vars.push(format!(
                            "  {}: {family};",
                            css_var_name(&["preset", "font-family", slug])
                        ));
                    }
                }
            }
        }
    }
    let spacing = settings_obj
        .get("spacing")
        .and_then(|s| obj(s).cloned())
        .unwrap_or_default();
    if let Some(Value::Array(spacing_sizes)) = spacing.get("spacingSizes") {
        for entry in spacing_sizes {
            if let Some(entry_obj) = obj(entry) {
                if is_slug(entry_obj.get("slug").unwrap_or(&Value::Null)) {
                    if let Some(size) = entry_obj.get("size") {
                        if size.is_string() || size.is_number() {
                            let slug = entry_obj.get("slug").unwrap().as_str().unwrap();
                            root_vars.push(format!(
                                "  {}: {};",
                                css_var_name(&["preset", "spacing", slug]),
                                css_scalar(size)
                            ));
                        }
                    }
                }
            }
        }
    }
    let layout = settings_obj
        .get("layout")
        .and_then(|l| obj(l).cloned())
        .unwrap_or_default();
    if let Some(Value::String(content_size)) = layout.get("contentSize") {
        root_vars.push(format!(
            "  --wp--style--global--content-size: {content_size};"
        ));
    }
    if let Some(Value::String(wide_size)) = layout.get("wideSize") {
        root_vars.push(format!("  --wp--style--global--wide-size: {wide_size};"));
    }
    if let Some(Value::String(gap)) = styles
        .as_object()
        .and_then(|s| s.get("spacing"))
        .and_then(|s| obj(s))
        .and_then(|s| s.get("blockGap"))
    {
        root_vars.push(format!("  --wp--style--block-gap: {gap};"));
    }
    if let Some(custom_obj) = settings_obj.get("custom").and_then(obj) {
        let mut flat: Vec<(String, String)> = Vec::new();
        flatten_custom(custom_obj, &[], &mut flat);
        for (name, value) in flat {
            root_vars.push(format!("  {name}: {value};"));
        }
    }
    if !root_vars.is_empty() {
        lines.push(":root {".to_string());
        lines.extend(root_vars);
        lines.push("}".to_string());
    }

    let body_props = style_props(styles, &font_sizes);
    if !body_props.is_empty() {
        lines.push("body {".to_string());
        for prop in body_props {
            lines.push(format!("  {prop};"));
        }
        lines.push("}".to_string());
    }

    let elements = styles
        .as_object()
        .and_then(|s| s.get("elements"))
        .and_then(|e| obj(e).cloned())
        .unwrap_or_default();
    if let Some(link) = elements.get("link") {
        if is_object(link) {
            let props = element_props(link, &font_sizes);
            if !props.is_empty() {
                lines.push("a {".to_string());
                for prop in props {
                    lines.push(format!("  {prop};"));
                }
                lines.push("}".to_string());
            }
        }
    }
    if let Some(heading) = elements.get("heading") {
        if is_object(heading) {
            let props = element_props(heading, &font_sizes);
            if !props.is_empty() {
                lines.push("h1, h2, h3, h4, h5, h6 {".to_string());
                for prop in props {
                    lines.push(format!("  {prop};"));
                }
                lines.push("}".to_string());
            }
        }
    }
    for level in ["h1", "h2", "h3", "h4", "h5", "h6"] {
        if let Some(el) = elements.get(level) {
            if is_object(el) {
                let props = element_props(el, &font_sizes);
                if !props.is_empty() {
                    lines.push(format!("{level} {{"));
                    for prop in props {
                        lines.push(format!("  {prop};"));
                    }
                    lines.push("}".to_string());
                }
            }
        }
    }
    if let Some(button) = elements.get("button") {
        if is_object(button) {
            let props = element_props(button, &font_sizes);
            if !props.is_empty() {
                lines.push("button, .wp-element-button, input[type=\"submit\"] {".to_string());
                for prop in props {
                    lines.push(format!("  {prop};"));
                }
                lines.push("}".to_string());
            }
        }
    }

    let blocks = styles
        .as_object()
        .and_then(|s| s.get("blocks"))
        .and_then(|b| obj(b).cloned())
        .unwrap_or_default();
    let mut block_names: Vec<&String> = blocks.keys().collect();
    block_names.sort();
    for name in block_names {
        let block_styles = &blocks[name];
        let Some(block_obj) = obj(block_styles) else {
            continue;
        };
        let mut rest = Map::new();
        for (k, val) in block_obj {
            if k != "elements" {
                rest.insert(k.clone(), val.clone());
            }
        }
        let props = style_props(&Value::Object(rest), &font_sizes);
        if props.is_empty() {
            continue;
        }
        let short = name.strip_prefix("core/").unwrap_or(name);
        let class_name = format!("wp-block-{}", short.replace('/', "-"));
        lines.push(format!(".{class_name} {{"));
        for prop in props {
            lines.push(format!("  {prop};"));
        }
        lines.push("}".to_string());
    }

    let has_content_size = matches!(layout.get("contentSize"), Some(Value::String(_)));
    let has_wide_size = matches!(layout.get("wideSize"), Some(Value::String(_)));
    if has_content_size || has_wide_size {
        lines.push("main, .wp-site-blocks {".to_string());
        if has_content_size {
            lines.push("  max-width: var(--wp--style--global--content-size);".to_string());
        }
        lines.push("  margin-left: auto;".to_string());
        lines.push("  margin-right: auto;".to_string());
        lines.push("}".to_string());
        if has_wide_size {
            lines.push(".alignwide {".to_string());
            lines.push("  max-width: var(--wp--style--global--wide-size);".to_string());
            lines.push("}".to_string());
        }
    }

    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    }
}

/// Deep-merge variation settings over base settings. Arrays of
/// `{slug, …}` entries merge by slug (override wins per slug, new slugs
/// appended); other arrays replace; objects merge recursively.
pub fn merge_theme_settings(base: &Value, override_value: &Value) -> Value {
    let Some(base_obj) = obj(base) else {
        return override_value.clone();
    };
    let Some(override_obj) = obj(override_value) else {
        return override_value.clone();
    };
    let mut out = base_obj.clone();
    for (key, value) in override_obj {
        match (out.get(key).cloned(), value) {
            (Some(Value::Object(_)), Value::Object(_)) => {
                let merged = merge_theme_settings(out.get(key).unwrap(), value);
                out.insert(key.clone(), merged);
            }
            (Some(Value::Array(existing_arr)), Value::Array(new_arr)) => {
                let mut merged: Vec<Value> = existing_arr.clone();
                for entry in new_arr {
                    let mut handled = false;
                    if let Some(entry_obj) = obj(entry) {
                        if let Some(Value::String(slug)) = entry_obj.get("slug") {
                            let idx = merged.iter().position(|e| {
                                obj(e)
                                    .and_then(|o| o.get("slug"))
                                    .and_then(|s| s.as_str())
                                    .map(|s| s == slug)
                                    .unwrap_or(false)
                            });
                            if let Some(idx) = idx {
                                let mut merged_entry =
                                    obj(&merged[idx]).cloned().unwrap_or_default();
                                for (k, v) in entry_obj {
                                    merged_entry.insert(k.clone(), v.clone());
                                }
                                merged[idx] = Value::Object(merged_entry);
                            } else {
                                merged.push(entry.clone());
                            }
                            handled = true;
                        }
                    }
                    if !handled {
                        merged.push(entry.clone());
                    }
                }
                out.insert(key.clone(), Value::Array(merged));
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(out)
}

/// Apply a variation's settings to a parsed theme and regenerate its CSS.
pub fn generate_theme_css_with_variation(parsed: &ParsedThemeJson, variation: &Value) -> String {
    let merged = ParsedThemeJson {
        settings: merge_theme_settings(&parsed.settings, variation),
        ..parsed.clone()
    };
    generate_theme_css(&merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    const MINIMAL_V3_CSS: &str = ":root {\n  --wp--preset--color--base: #FFFFFF;\n  --wp--preset--color--primary: #2563EB;\n  --wp--preset--font-size--medium: 1rem;\n  --wp--style--global--content-size: 680px;\n  --wp--style--global--wide-size: 1080px;\n}\nbody {\n  background-color: var(--wp--preset--color--base);\n  color: var(--wp--preset--color--contrast);\n  font-family: system-ui, sans-serif;\n  line-height: 1.7;\n}\na {\n  color: var(--wp--preset--color--primary);\n}\nmain, .wp-site-blocks {\n  max-width: var(--wp--style--global--content-size);\n  margin-left: auto;\n  margin-right: auto;\n}\n.alignwide {\n  max-width: var(--wp--style--global--wide-size);\n}\n";

    fn minimal_v3() -> Value {
        serde_json::from_str(MINIMAL_V3).unwrap()
    }

    #[test]
    fn parses_valid_v3_with_no_errors() {
        let parsed = parse_theme_json(&minimal_v3());
        assert_eq!(parsed.version, 3);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    }

    #[test]
    fn rejects_bad_version_and_palette_entry_without_colour() {
        let parsed = parse_theme_json(&json!({
            "version": 4,
            "settings": { "color": { "palette": [{ "slug": "x", "name": "X" }] } }
        }));
        let paths: Vec<&str> = parsed.errors.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"version"), "{paths:?}");
        assert!(
            paths.contains(&"settings.color.palette[0].color"),
            "{paths:?}"
        );
    }

    #[test]
    fn rejects_font_size_without_size() {
        let parsed = parse_theme_json(&json!({
            "version": 3,
            "settings": { "typography": { "fontSizes": [{ "slug": "big", "name": "Big" }] } }
        }));
        let paths: Vec<&str> = parsed.errors.iter().map(|e| e.path.as_str()).collect();
        assert!(
            paths.contains(&"settings.typography.fontSizes[0].size"),
            "{paths:?}"
        );
    }

    #[test]
    fn rejects_non_object_theme_json() {
        assert!(!parse_theme_json(&json!("nope")).errors.is_empty());
        assert!(!parse_theme_json(&Value::Null).errors.is_empty());
    }

    #[test]
    fn records_v1_gaps_as_unsupported_warnings_not_errors() {
        let parsed = parse_theme_json(&json!({
            "version": 1,
            "settings": { "custom": { "radius": "4px" } },
            "customTemplates": [{ "name": "x", "title": "X" }],
            "templateParts": [{ "name": "header", "title": "Header" }]
        }));
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let joined = parsed.unsupported.join(" ");
        assert!(joined.contains("settings.custom"), "{joined}");
        assert!(joined.contains("customTemplates"), "{joined}");
        assert!(joined.contains("templateParts"), "{joined}");
    }

    #[test]
    fn records_v2_gaps_as_unsupported() {
        let parsed = parse_theme_json(&json!({
            "version": 2,
            "settings": { "typography": { "writingMode": true } },
            "styles": { "blocks": { "core/quote": { "elements": { "link": { "color": { "text": "#000" } } } } } }
        }));
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let joined = parsed.unsupported.join(" ");
        assert!(joined.contains("writingMode"), "{joined}");
        assert!(
            joined.contains("styles.blocks.core/quote.elements"),
            "{joined}"
        );
    }

    #[test]
    fn validates_variation_partial_settings_with_same_rules() {
        assert!(validate_theme_settings(
            &json!({ "color": { "palette": [{ "slug": "x", "color": "#fff", "name": "X" }] } }),
            3
        )
        .is_empty());
        let bad = validate_theme_settings(
            &json!({ "color": { "palette": [{ "slug": "x", "name": "X" }] } }),
            3,
        );
        let paths: Vec<&str> = bad.iter().map(|e| e.path.as_str()).collect();
        assert!(
            paths.contains(&"settings.color.palette[0].color"),
            "{paths:?}"
        );
    }

    #[test]
    fn generates_exact_stylesheet_for_minimal_v3() {
        let parsed = parse_theme_json(&minimal_v3());
        assert_eq!(generate_theme_css(&parsed), MINIMAL_V3_CSS);
    }

    #[test]
    fn generates_preset_vars_fluid_sizes_and_per_block_styles() {
        let parsed = parse_theme_json(&json!({
            "version": 3,
            "settings": {
                "color": { "palette": [{ "slug": "accent", "color": "#0D9488", "name": "Accent" }] },
                "typography": {
                    "fontSizes": [{ "slug": "large", "size": "1.25rem", "name": "Large", "fluid": { "min": "1.125rem", "max": "1.5rem" } }]
                },
                "spacing": { "spacingSizes": [{ "slug": "50", "size": "30px", "name": "Regular" }] },
                "custom": { "radius": "6px" }
            },
            "styles": {
                "blocks": { "core/quote": { "color": { "text": "var:preset|color|accent" } } },
                "elements": { "button": { "color": { "background": "var:preset|color|accent", "text": "#fff" } } }
            }
        }));
        let css = generate_theme_css(&parsed);
        assert!(
            css.contains("--wp--preset--color--accent: #0D9488;"),
            "{css}"
        );
        assert!(
            css.contains("--wp--preset--font-size--large: clamp(1.125rem,"),
            "{css}"
        );
        assert!(css.contains("--wp--preset--spacing--50: 30px;"), "{css}");
        assert!(css.contains("--wp--custom--radius: 6px;"), "{css}");
        assert!(css.contains(".wp-block-quote {"), "{css}");
        assert!(
            css.contains("color: var(--wp--preset--color--accent);"),
            "{css}"
        );
        assert!(css.contains("button, .wp-element-button"), "{css}");
    }

    #[test]
    fn hashes_deterministically_and_changes_with_css() {
        assert_eq!(stylesheet_hash("a{}"), stylesheet_hash("a{}"));
        assert_ne!(stylesheet_hash("a{}"), stylesheet_hash("b{}"));
        let h = stylesheet_hash("a{}");
        assert_eq!(h.len(), 12);
        assert!(h.bytes().all(|b| b.is_ascii_hexdigit()), "{h}");
    }

    #[test]
    fn hash_matches_v1_cyrb53_golden_values() {
        // Golden values computed from v1's stylesheetHash (Node, 2026-11-19).
        assert_eq!(stylesheet_hash("a{}"), "d56e3a2baeb5");
        assert_eq!(stylesheet_hash(""), "488bdcb81aee");
        assert_eq!(
            stylesheet_hash("hello world, this is a longer stylesheet with unicode héllo"),
            "902c029af83f"
        );
    }

    #[test]
    fn merges_variation_settings_by_slug() {
        let merged = merge_theme_settings(
            &json!({ "color": { "palette": [{ "slug": "primary", "color": "#111", "name": "P" }] } }),
            &json!({ "color": { "palette": [{ "slug": "primary", "color": "#F00" }, { "slug": "new", "color": "#0F0", "name": "N" }] } }),
        );
        let palette = merged["color"]["palette"].as_array().unwrap();
        assert_eq!(palette.len(), 2);
        assert_eq!(palette[0]["color"], "#F00");
        assert_eq!(palette[1]["slug"], "new");
    }

    #[test]
    fn applies_variation_to_generated_css() {
        let parsed = parse_theme_json(&minimal_v3());
        let css = generate_theme_css_with_variation(
            &parsed,
            &json!({ "color": { "palette": [{ "slug": "primary", "color": "#FF0000", "name": "Primary" }] } }),
        );
        assert!(
            css.contains("--wp--preset--color--primary: #FF0000;"),
            "{css}"
        );
        assert!(css.contains("--wp--preset--color--base: #FFFFFF;"), "{css}");
    }

    #[test]
    fn resolve_preset_value_forms() {
        assert_eq!(
            resolve_preset_value("var:preset|color|accent-1"),
            "var(--wp--preset--color--accent-1)"
        );
        assert_eq!(
            resolve_preset_value("var:preset|custom|radius"),
            "var(--wp--custom--radius)"
        );
        assert_eq!(resolve_preset_value("  #fff  "), "#fff");
        assert_eq!(resolve_preset_value("var:preset|"), "var:preset|");
    }
}
