-- Milestone 9: security + performance + backup + WordPress importer.
-- Login-attempt tracking (lockout), backups registry, image-derivative
-- records. Additive only; the wl_sessions change is one new nullable
-- column (last_seen_at, for the idle timeout). No existing table changes.
CREATE TABLE IF NOT EXISTS wl_login_attempts (
  key TEXT PRIMARY KEY,
  attempts INTEGER NOT NULL DEFAULT 0,
  first_at TEXT NOT NULL,
  last_at TEXT NOT NULL,
  locked_until TEXT,
  lockout_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS wl_backups (
  id TEXT PRIMARY KEY,
  filename TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  checksum TEXT NOT NULL DEFAULT '',
  tables TEXT NOT NULL DEFAULT '{}',
  media_count INTEGER NOT NULL DEFAULT 0,
  media_missing INTEGER NOT NULL DEFAULT 0,
  verified INTEGER NOT NULL DEFAULT 0,
  verified_at TEXT,
  created_at TEXT NOT NULL,
  created_by INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_wl_backups_created ON wl_backups (created_at DESC);

CREATE TABLE IF NOT EXISTS wl_media_derivatives (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  media_id INTEGER NOT NULL,
  width INTEGER NOT NULL,
  format TEXT NOT NULL,
  object_key TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  UNIQUE (media_id, width, format)
);
CREATE INDEX IF NOT EXISTS idx_wl_media_derivatives_media ON wl_media_derivatives (media_id);

-- Idle timeout for sessions (M9): last_seen_at is updated when a
-- session is verified and is more than 5 minutes stale.
ALTER TABLE wl_sessions ADD COLUMN last_seen_at TEXT;
