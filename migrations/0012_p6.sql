-- WORDLESS v2 P6 — extension runtime registry.
-- wl_extension_instances is the kill switch: a `disabled` instance is
-- never dispatched (status is re-read on every invocation).
-- host_token_hash stores only the SHA-256 hex of the derived host
-- token (see wordless-extensions host::derive_host_token); token_epoch
-- increments on every (re)load so a reload rotates the token.
CREATE TABLE wl_extension_instances (
  slug TEXT PRIMARY KEY,
  version TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'loaded' CHECK (status IN ('loaded', 'disabled')),
  permissions_json TEXT NOT NULL DEFAULT '[]',
  script_key TEXT NOT NULL DEFAULT '',
  host_token_hash TEXT NOT NULL DEFAULT '',
  token_epoch INTEGER NOT NULL DEFAULT 0,
  dev_url TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_extension_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL,
  hook TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'ok' CHECK (status IN ('ok', 'error', 'timeout', 'denied', 'skipped_disabled')),
  duration_ms INTEGER NOT NULL DEFAULT 0,
  output_summary TEXT,
  error TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_wl_extension_runs_slug ON wl_extension_runs (slug);
CREATE TABLE wl_extension_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL,
  level TEXT NOT NULL DEFAULT 'info' CHECK (level IN ('info', 'warn', 'error', 'denied')),
  message TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE INDEX idx_wl_extension_logs_slug ON wl_extension_logs (slug);
