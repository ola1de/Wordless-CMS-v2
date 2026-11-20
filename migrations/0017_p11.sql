-- P11 (Visual Builder, 2026-11-20): the extension-widget registry.
-- An extension declares builder widgets here; the palette only shows
-- a widget when its extension is installed AND its runtime instance
-- is `loaded` (the P6 kill switch). Rendering an extension widget on
-- the public site is NOT in P11 — a layout holding one renders its
-- other nodes and lists the widget type as unsupported.
CREATE TABLE IF NOT EXISTS wl_builder_widgets (
  extension_slug TEXT NOT NULL,
  widget_type TEXT NOT NULL,
  label TEXT NOT NULL,
  icon TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  settings_schema_json TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (extension_slug, widget_type)
);
CREATE INDEX IF NOT EXISTS idx_wl_builder_widgets_slug ON wl_builder_widgets (extension_slug);
