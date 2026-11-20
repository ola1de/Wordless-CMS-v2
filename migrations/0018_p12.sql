-- P12 (Multisite, 2026-11-20): the site registry + per-site scoping.
-- Q14 (decided 2026-11-20, parent decision at P12 start): site-id
-- column on the wp_* tables + per-site options; single-site installs
-- carry site id 1 by default so nothing migrates. Path-based routing
-- by default (example.com/<site-slug>/...); subdomain routing via
-- Workers routes as a supported option; custom-hostname domain
-- mapping is OUT (needs Cloudflare for SaaS, paid — ruled out by
-- the user's no-money constraint).
--
-- `wl_sites.status`: public | archived | deleted. Archived sites
-- resolve but serve a 410; deleted sites are gone (their content
-- rows are removed by the delete handler, not by this migration).

CREATE TABLE IF NOT EXISTS wl_sites (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL UNIQUE,
  domain TEXT NOT NULL DEFAULT '',
  title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'public',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- The main site always exists: id 1, empty slug (the root). Seeded
-- here so resolution never has to special-case "no sites rows".
-- The display title is only the switcher label; the site's real
-- title still comes from its options (blogname).
INSERT OR IGNORE INTO wl_sites (id, slug, domain, title, status, created_at, updated_at)
VALUES (1, '', '', 'Main Site', 'public', '2026-11-20T00:00:00Z', '2026-11-20T00:00:00Z');

-- Per-site options. Site-scoped keys (blogname, blogdescription,
-- template, stylesheet, posts_per_page) live here for sites > 1 and
-- fall back to wp_options when unset; site 1 keeps reading
-- wp_options exactly as before, so single-site installs behave
-- identically with or without this table populated.
CREATE TABLE IF NOT EXISTS wl_site_options (
  site_id INTEGER NOT NULL,
  option_name TEXT NOT NULL,
  option_value TEXT NOT NULL,
  PRIMARY KEY (site_id, option_name)
);
CREATE INDEX IF NOT EXISTS idx_wl_site_options_site ON wl_site_options (site_id);

-- Site scoping on the content tables. DEFAULT 1 keeps every existing
-- row (and every existing INSERT that omits the column) on the main
-- site, so the migration is a no-op for single-site installs.
ALTER TABLE wp_posts ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wp_postmeta ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wp_terms ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wp_term_taxonomy ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wp_term_relationships ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wp_comments ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wp_commentmeta ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wp_termmeta ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wl_redirects ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
-- wl_404_log was PRIMARY KEY (path) in 0007: the same path on two
-- sites would collide on INSERT (the second site's 404 would fail
-- silently — log_not_found swallows errors). Rebuild it with a
-- per-site key so each site keeps its own 404 log.
CREATE TABLE wl_404_log_new (
  site_id INTEGER NOT NULL DEFAULT 1,
  path TEXT NOT NULL,
  count INTEGER NOT NULL DEFAULT 1,
  referrer TEXT NOT NULL DEFAULT '',
  first_seen TEXT NOT NULL,
  last_seen TEXT NOT NULL,
  PRIMARY KEY (site_id, path)
);
INSERT INTO wl_404_log_new (site_id, path, count, referrer, first_seen, last_seen)
  SELECT 1, path, count, referrer, first_seen, last_seen FROM wl_404_log;
DROP TABLE wl_404_log;
ALTER TABLE wl_404_log_new RENAME TO wl_404_log;
CREATE INDEX IF NOT EXISTS idx_wl_404_log_site ON wl_404_log (site_id, count DESC, last_seen DESC);

CREATE INDEX IF NOT EXISTS idx_wp_posts_site ON wp_posts (site_id, post_status, post_date);
CREATE INDEX IF NOT EXISTS idx_wp_postmeta_site ON wp_postmeta (site_id, post_id);
CREATE INDEX IF NOT EXISTS idx_wp_terms_site ON wp_terms (site_id, slug);
CREATE INDEX IF NOT EXISTS idx_wp_comments_site ON wp_comments (site_id, comment_post_ID);

-- Redirects were globally unique on old_path (0002); two sites can
-- legitimately own the same old path, so uniqueness becomes
-- per-site. Drop the global index, keep the post_id index.
DROP INDEX IF EXISTS idx_wl_redirects_old_path;
CREATE UNIQUE INDEX IF NOT EXISTS idx_wl_redirects_site_old ON wl_redirects (site_id, old_path);
ALTER TABLE wl_seo_audit_runs ADD COLUMN site_id INTEGER NOT NULL DEFAULT 1;
CREATE INDEX IF NOT EXISTS idx_wl_seo_audit_runs_site ON wl_seo_audit_runs (site_id, created_at DESC);
