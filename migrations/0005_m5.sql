-- Milestone 5: themes (wl_themes).
-- A theme is NOT an extension: it has its own table. Theme files
-- (templates, parts, assets) live in R2 under themes/<slug>/; the
-- theme.json and the generated stylesheet are cached here for fast reads.
-- The built-in default theme ("wordless-default") is seeded lazily by the
-- themes service when the table is empty (see src/modules/themes/service.ts).
CREATE TABLE IF NOT EXISTS wl_themes (
  slug TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  version TEXT NOT NULL DEFAULT '0.1.0',
  author TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL DEFAULT 'block' CHECK (kind IN ('block', 'classic')),
  status TEXT NOT NULL DEFAULT 'inactive' CHECK (status IN ('active', 'inactive')),
  theme_json TEXT,
  stylesheet TEXT NOT NULL DEFAULT '',
  stylesheet_hash TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL DEFAULT 'import',
  report_json TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_wl_themes_status ON wl_themes (status);
