-- WORDLESS Milestone 1 — D1 schema v1.
-- WordPress tables ported 1:1 in shape from wp-admin/includes/schema.php
-- (WordPress 7.1.5, $wp_db_version 61833), adapted to SQLite/D1 types.
-- Differences from MySQL, by design (see docs/02-architecture.md §3):
--   * option_value / meta_value store JSON, NOT PHP-serialized strings.
--   * user_pass stores PBKDF2 hashes (pbkdf2$iterations$salt$hash);
--     WordPress phpass hashes are NOT portable — imported users get a
--     forced password reset.
--   * Dates are ISO-8601 text, always UTC in the database.
--   * No AUTO_INCREMENT — SQLite INTEGER PRIMARY KEY AUTOINCREMENT.
-- Multisite tables are deferred (single-site first).
-- Wordless tables are prefixed wl_.

-- ---------------------------------------------------------------- wp_posts
CREATE TABLE IF NOT EXISTS wp_posts (
  ID INTEGER PRIMARY KEY AUTOINCREMENT,
  post_author INTEGER NOT NULL DEFAULT 0,
  post_date TEXT,
  post_date_gmt TEXT,
  post_content TEXT NOT NULL DEFAULT '',
  post_title TEXT NOT NULL DEFAULT '',
  post_excerpt TEXT NOT NULL DEFAULT '',
  post_status TEXT NOT NULL DEFAULT 'publish',
  comment_status TEXT NOT NULL DEFAULT 'open',
  ping_status TEXT NOT NULL DEFAULT 'open',
  post_password TEXT NOT NULL DEFAULT '',
  post_name TEXT NOT NULL DEFAULT '',
  post_modified TEXT,
  post_modified_gmt TEXT,
  post_content_filtered TEXT NOT NULL DEFAULT '',
  post_parent INTEGER NOT NULL DEFAULT 0,
  guid TEXT NOT NULL DEFAULT '',
  menu_order INTEGER NOT NULL DEFAULT 0,
  post_type TEXT NOT NULL DEFAULT 'post',
  post_mime_type TEXT NOT NULL DEFAULT '',
  comment_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_wp_posts_post_name ON wp_posts (post_name);
CREATE INDEX IF NOT EXISTS idx_wp_posts_type_status_date ON wp_posts (post_type, post_status, post_date, ID);
CREATE INDEX IF NOT EXISTS idx_wp_posts_post_parent ON wp_posts (post_parent);
CREATE INDEX IF NOT EXISTS idx_wp_posts_post_author ON wp_posts (post_author);

-- ------------------------------------------------------------- wp_postmeta
CREATE TABLE IF NOT EXISTS wp_postmeta (
  meta_id INTEGER PRIMARY KEY AUTOINCREMENT,
  post_id INTEGER NOT NULL DEFAULT 0,
  meta_key TEXT,
  meta_value TEXT
);
CREATE INDEX IF NOT EXISTS idx_wp_postmeta_post_id ON wp_postmeta (post_id);
CREATE INDEX IF NOT EXISTS idx_wp_postmeta_meta_key ON wp_postmeta (meta_key);

-- --------------------------------------------------------------- wp_users
CREATE TABLE IF NOT EXISTS wp_users (
  ID INTEGER PRIMARY KEY AUTOINCREMENT,
  user_login TEXT NOT NULL DEFAULT '',
  user_pass TEXT NOT NULL DEFAULT '',
  user_nicename TEXT NOT NULL DEFAULT '',
  user_email TEXT NOT NULL DEFAULT '',
  user_url TEXT NOT NULL DEFAULT '',
  user_registered TEXT NOT NULL DEFAULT '',
  user_activation_key TEXT NOT NULL DEFAULT '',
  user_status INTEGER NOT NULL DEFAULT 0,
  display_name TEXT NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wp_users_user_login ON wp_users (user_login);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wp_users_user_email ON wp_users (user_email);

-- ------------------------------------------------------------ wp_usermeta
CREATE TABLE IF NOT EXISTS wp_usermeta (
  umeta_id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id INTEGER NOT NULL DEFAULT 0,
  meta_key TEXT,
  meta_value TEXT
);
CREATE INDEX IF NOT EXISTS idx_wp_usermeta_user_id ON wp_usermeta (user_id);
CREATE INDEX IF NOT EXISTS idx_wp_usermeta_meta_key ON wp_usermeta (meta_key);

-- --------------------------------------------------------------- wp_terms
CREATE TABLE IF NOT EXISTS wp_terms (
  term_id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL DEFAULT '',
  slug TEXT NOT NULL DEFAULT '',
  term_group INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wp_terms_slug ON wp_terms (slug);
CREATE INDEX IF NOT EXISTS idx_wp_terms_name ON wp_terms (name);

-- ------------------------------------------------------- wp_term_taxonomy
CREATE TABLE IF NOT EXISTS wp_term_taxonomy (
  term_taxonomy_id INTEGER PRIMARY KEY AUTOINCREMENT,
  term_id INTEGER NOT NULL DEFAULT 0,
  taxonomy TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  parent INTEGER NOT NULL DEFAULT 0,
  count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_wp_term_taxonomy_term_id ON wp_term_taxonomy (term_id);
CREATE INDEX IF NOT EXISTS idx_wp_term_taxonomy_taxonomy ON wp_term_taxonomy (taxonomy);

-- --------------------------------------------------- wp_term_relationships
CREATE TABLE IF NOT EXISTS wp_term_relationships (
  object_id INTEGER NOT NULL DEFAULT 0,
  term_taxonomy_id INTEGER NOT NULL DEFAULT 0,
  term_order INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (object_id, term_taxonomy_id)
);
CREATE INDEX IF NOT EXISTS idx_wp_term_relationships_tt ON wp_term_relationships (term_taxonomy_id);

-- ------------------------------------------------------------- wp_comments
CREATE TABLE IF NOT EXISTS wp_comments (
  comment_ID INTEGER PRIMARY KEY AUTOINCREMENT,
  comment_post_ID INTEGER NOT NULL DEFAULT 0,
  comment_author TEXT NOT NULL DEFAULT '',
  comment_author_email TEXT NOT NULL DEFAULT '',
  comment_author_url TEXT NOT NULL DEFAULT '',
  comment_author_IP TEXT NOT NULL DEFAULT '',
  comment_date TEXT NOT NULL DEFAULT '',
  comment_date_gmt TEXT NOT NULL DEFAULT '',
  comment_content TEXT NOT NULL DEFAULT '',
  comment_karma INTEGER NOT NULL DEFAULT 0,
  comment_approved TEXT NOT NULL DEFAULT '1',
  comment_agent TEXT NOT NULL DEFAULT '',
  comment_type TEXT NOT NULL DEFAULT 'comment',
  comment_parent INTEGER NOT NULL DEFAULT 0,
  user_id INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_wp_comments_post_id ON wp_comments (comment_post_ID);
CREATE INDEX IF NOT EXISTS idx_wp_comments_approved ON wp_comments (comment_approved, comment_date_gmt);
CREATE INDEX IF NOT EXISTS idx_wp_comments_parent ON wp_comments (comment_parent);

-- ---------------------------------------------------------- wp_commentmeta
CREATE TABLE IF NOT EXISTS wp_commentmeta (
  meta_id INTEGER PRIMARY KEY AUTOINCREMENT,
  comment_id INTEGER NOT NULL DEFAULT 0,
  meta_key TEXT,
  meta_value TEXT
);
CREATE INDEX IF NOT EXISTS idx_wp_commentmeta_comment_id ON wp_commentmeta (comment_id);
CREATE INDEX IF NOT EXISTS idx_wp_commentmeta_meta_key ON wp_commentmeta (meta_key);

-- ---------------------------------------------------------------- wp_links
CREATE TABLE IF NOT EXISTS wp_links (
  link_id INTEGER PRIMARY KEY AUTOINCREMENT,
  link_url TEXT NOT NULL DEFAULT '',
  link_name TEXT NOT NULL DEFAULT '',
  link_image TEXT NOT NULL DEFAULT '',
  link_target TEXT NOT NULL DEFAULT '',
  link_description TEXT NOT NULL DEFAULT '',
  link_visible TEXT NOT NULL DEFAULT 'Y',
  link_owner INTEGER NOT NULL DEFAULT 1,
  link_rating INTEGER NOT NULL DEFAULT 0,
  link_updated TEXT NOT NULL DEFAULT '',
  link_rel TEXT NOT NULL DEFAULT '',
  link_notes TEXT NOT NULL DEFAULT '',
  link_rss TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_wp_links_visible ON wp_links (link_visible);

-- -------------------------------------------------------------- wp_options
CREATE TABLE IF NOT EXISTS wp_options (
  option_id INTEGER PRIMARY KEY AUTOINCREMENT,
  option_name TEXT NOT NULL DEFAULT '',
  option_value TEXT NOT NULL DEFAULT '',
  autoload TEXT NOT NULL DEFAULT 'yes'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wp_options_option_name ON wp_options (option_name);

-- ------------------------------------------------------------- wp_termmeta
CREATE TABLE IF NOT EXISTS wp_termmeta (
  meta_id INTEGER PRIMARY KEY AUTOINCREMENT,
  term_id INTEGER NOT NULL DEFAULT 0,
  meta_key TEXT,
  meta_value TEXT
);
CREATE INDEX IF NOT EXISTS idx_wp_termmeta_term_id ON wp_termmeta (term_id);
CREATE INDEX IF NOT EXISTS idx_wp_termmeta_meta_key ON wp_termmeta (meta_key);

-- ================================================= Wordless tables (wl_) ===

-- Opaque sessions (PBKDF2 user passwords; sessions revocable, auditable).
-- token_hash is SHA-256 hex of the bearer token; the raw token is never stored.
CREATE TABLE IF NOT EXISTS wl_sessions (
  id TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL,
  token_hash TEXT NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  ip_hash TEXT,
  user_agent TEXT,
  revoked INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wl_sessions_token_hash ON wl_sessions (token_hash);
CREATE INDEX IF NOT EXISTS idx_wl_sessions_user_id ON wl_sessions (user_id);
CREATE INDEX IF NOT EXISTS idx_wl_sessions_expires_at ON wl_sessions (expires_at);

-- Installed extensions (native Wordless + converted WordPress plugins/themes).
CREATE TABLE IF NOT EXISTS wl_extensions (
  id TEXT PRIMARY KEY,
  slug TEXT NOT NULL,
  name TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'plugin',
  version TEXT NOT NULL DEFAULT '0.1.0',
  status TEXT NOT NULL DEFAULT 'inactive',
  source TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wl_extensions_slug ON wl_extensions (slug);
CREATE INDEX IF NOT EXISTS idx_wl_extensions_status ON wl_extensions (status);

-- Durable background jobs (Queues/Workflows payloads; conversion, AI, media).
CREATE TABLE IF NOT EXISTS wl_jobs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'queued',
  payload TEXT NOT NULL DEFAULT '{}',
  result TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  run_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_wl_jobs_status ON wl_jobs (status, run_at);
CREATE INDEX IF NOT EXISTS idx_wl_jobs_kind ON wl_jobs (kind);

-- Audit trail for administrative and AI actions.
CREATE TABLE IF NOT EXISTS wl_audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id INTEGER,
  action TEXT NOT NULL,
  object_type TEXT,
  object_id TEXT,
  details TEXT,
  ip_hash TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_audit_log_created_at ON wl_audit_log (created_at);
CREATE INDEX IF NOT EXISTS idx_wl_audit_log_user_id ON wl_audit_log (user_id);
CREATE INDEX IF NOT EXISTS idx_wl_audit_log_action ON wl_audit_log (action);

-- API keys (hashed; raw key shown once at creation, never stored).
CREATE TABLE IF NOT EXISTS wl_api_keys (
  id TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL,
  name TEXT NOT NULL,
  key_hash TEXT NOT NULL,
  scopes TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  last_used_at TEXT,
  revoked INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wl_api_keys_key_hash ON wl_api_keys (key_hash);
CREATE INDEX IF NOT EXISTS idx_wl_api_keys_user_id ON wl_api_keys (user_id);

-- Applied migrations ledger.
CREATE TABLE IF NOT EXISTS wl_migrations (
  id TEXT PRIMARY KEY,
  applied_at TEXT NOT NULL
);
