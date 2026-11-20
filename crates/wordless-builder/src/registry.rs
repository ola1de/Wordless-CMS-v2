//! P11 — the extension-widget registry (`wl_builder_widgets`,
//! migration `0017_p11.sql`).
//!
//! An extension declares builder widgets here; the palette only shows
//! a widget when its extension is installed AND its runtime instance
//! is `loaded` (the P6 kill switch, re-checked on every palette read).
//! A widget whose extension is installed but disabled, or never
//! loaded, is invisible to the builder — never silently half-working.
//!
//! Rendering an extension widget on the public site is NOT in P11:
//! a layout holding one renders its other nodes and lists the widget
//! type as unsupported (the P3 renderer's own rule). The palette, the
//! preview API and the public render all agree on that list, so the
//! author always sees exactly what will not render.

use serde::{Deserialize, Serialize};
use wordless_db::{
    row::{row_opt_text, row_text},
    Db, Param, Row,
};
use wordless_kernel::{error::WordlessError, time::now_iso};

use crate::palette::icon_known;

/// One registered extension widget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuilderWidget {
    pub extension_slug: String,
    pub widget_type: String,
    pub label: String,
    pub icon: String,
    pub description: String,
    /// Settings schema JSON (same shape as the palette `fields`).
    pub settings_schema_json: String,
    /// Runtime status of the owning extension: `loaded`, `disabled`,
    /// or `not-loaded` (installed, never loaded). `""` when the
    /// extension row itself is gone.
    #[serde(default)]
    pub extension_status: String,
}

fn widget_from_row(row: &Row) -> BuilderWidget {
    BuilderWidget {
        extension_slug: row_text(row, "extension_slug"),
        widget_type: row_text(row, "widget_type"),
        label: row_text(row, "label"),
        icon: row_text(row, "icon"),
        description: row_text(row, "description"),
        settings_schema_json: row_text(row, "settings_schema_json"),
        extension_status: row_opt_text(row, "extension_status").unwrap_or_default(),
    }
}

/// Widget-type rules: lowercase slug characters, and never a type the
/// built-in palette already owns (an extension cannot shadow a core
/// widget or element).
pub fn valid_widget_type(widget_type: &str) -> bool {
    if widget_type.is_empty() || widget_type.len() > 64 {
        return false;
    }
    let mut chars = widget_type.chars();
    let first_ok = chars
        .next()
        .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .unwrap_or(false);
    first_ok
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !crate::palette::is_core_widget(widget_type)
        && !crate::palette::is_element_widget(widget_type)
}

/// Register (or re-register) one widget for an installed extension.
/// Errors: extension not installed (404), bad widget type (400),
/// unknown icon (400), empty label (400). The settings schema must
/// be valid JSON (an object or array); it is stored verbatim.
pub async fn register_widget(
    db: &dyn Db,
    extension_slug: &str,
    widget_type: &str,
    label: &str,
    icon: &str,
    description: &str,
    settings_schema_json: &str,
) -> Result<BuilderWidget, WordlessError> {
    if !valid_widget_type(widget_type) {
        return Err(WordlessError::new(
            "invalid_widget_type",
            400,
            "widget_type must be a lowercase slug that does not collide with a built-in widget",
        ));
    }
    if label.trim().is_empty() {
        return Err(WordlessError::new(
            "invalid_widget",
            400,
            "label must not be empty",
        ));
    }
    if !icon.is_empty() && !icon_known(icon) {
        return Err(WordlessError::new(
            "unknown_icon",
            400,
            "icon must be a name from the icon library",
        ));
    }
    let schema: serde_json::Value = serde_json::from_str(settings_schema_json).map_err(|_| {
        WordlessError::new(
            "invalid_settings_schema",
            400,
            "settings_schema must be valid JSON",
        )
    })?;
    if !schema.is_object() && !schema.is_array() {
        return Err(WordlessError::new(
            "invalid_settings_schema",
            400,
            "settings_schema must be a JSON object or array",
        ));
    }
    // The extension must be installed (a row in wl_extensions).
    let installed: Option<Row> = db
        .prepare("SELECT slug FROM wl_extensions WHERE slug = ? LIMIT 1")
        .bind(vec![Param::Text(extension_slug.to_string())])
        .first()
        .await?;
    if installed.is_none() {
        return Err(WordlessError::new(
            "extension_not_installed",
            404,
            "cannot register a widget for an extension that is not installed",
        ));
    }
    let now = now_iso();
    db.prepare(
        "INSERT INTO wl_builder_widgets (extension_slug, widget_type, label, icon, description, settings_schema_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(extension_slug, widget_type) DO UPDATE SET
           label = excluded.label, icon = excluded.icon,
           description = excluded.description,
           settings_schema_json = excluded.settings_schema_json,
           updated_at = excluded.updated_at",
    )
    .bind(vec![
        Param::Text(extension_slug.to_string()),
        Param::Text(widget_type.to_string()),
        Param::Text(label.to_string()),
        Param::Text(icon.to_string()),
        Param::Text(description.to_string()),
        Param::Text(settings_schema_json.to_string()),
        Param::Text(now.clone()),
        Param::Text(now),
    ])
    .run()
    .await?;
    Ok(BuilderWidget {
        extension_slug: extension_slug.to_string(),
        widget_type: widget_type.to_string(),
        label: label.to_string(),
        icon: icon.to_string(),
        description: description.to_string(),
        settings_schema_json: settings_schema_json.to_string(),
        extension_status: String::new(),
    })
}

/// Every registered widget, with its extension's runtime status
/// (joined from `wl_extension_instances`; `not-loaded` when there is
/// no instance row).
pub async fn list_widgets(db: &dyn Db) -> Result<Vec<BuilderWidget>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare(
            "SELECT w.extension_slug, w.widget_type, w.label, w.icon, w.description,
                    w.settings_schema_json,
                    COALESCE(i.status, 'not-loaded') AS extension_status
             FROM wl_builder_widgets w
             LEFT JOIN wl_extension_instances i ON i.slug = w.extension_slug
             ORDER BY w.extension_slug, w.widget_type",
        )
        .all()
        .await?;
    Ok(rows.iter().map(widget_from_row).collect())
}

/// Only the widgets whose extension instance is `loaded` — the
/// palette gate. Disabled / never-loaded extensions contribute
/// nothing, and the caller never has to remember the rule.
pub async fn loaded_widgets(db: &dyn Db) -> Result<Vec<BuilderWidget>, WordlessError> {
    Ok(list_widgets(db)
        .await?
        .into_iter()
        .filter(|w| w.extension_status == "loaded")
        .collect())
}

/// Delete one registered widget. Returns true when a row was removed.
pub async fn delete_widget(
    db: &dyn Db,
    extension_slug: &str,
    widget_type: &str,
) -> Result<bool, WordlessError> {
    let done = db
        .prepare("DELETE FROM wl_builder_widgets WHERE extension_slug = ? AND widget_type = ?")
        .bind(vec![
            Param::Text(extension_slug.to_string()),
            Param::Text(widget_type.to_string()),
        ])
        .run()
        .await?;
    Ok(done.changes > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_type_rules() {
        assert!(valid_widget_type("fancy-slider"));
        assert!(valid_widget_type("w2"));
        assert!(!valid_widget_type(""));
        assert!(!valid_widget_type("FancySlider"));
        assert!(!valid_widget_type("fancy_slider"));
        assert!(!valid_widget_type("wordless-badge"));
        assert!(!valid_widget_type("heading"));
        assert!(!valid_widget_type("button"));
        assert!(!valid_widget_type("a".repeat(65).as_str()));
    }
}
