-- WORDLESS Milestone 2 — Core CMS.
-- Applies cleanly on top of 0001_init.sql (wrangler applies migrations in
-- filename order). Nothing here alters 0001; changes are additive, except
-- the wp_terms slug index, which is relaxed (see note).

-- ---------------------------------------------------------------- wl_redirects
-- Slug-change history: when a published post/page changes slug, the old
-- permalink path redirects (301) to the new one. old_path is the full
-- old URL path (e.g. "/2026/11/19/hello-world" or "/hello-world").
CREATE TABLE IF NOT EXISTS wl_redirects (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  old_path TEXT NOT NULL,
  new_path TEXT NOT NULL,
  post_id INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wl_redirects_old_path ON wl_redirects (old_path);
CREATE INDEX IF NOT EXISTS idx_wl_redirects_post_id ON wl_redirects (post_id);

-- ------------------------------------------- wp_terms slug: per-taxonomy --
-- 0001 made wp_terms.slug globally UNIQUE, but WordPress allows the same
-- slug in different taxonomies (a category and a tag may both be "news").
-- Relax the index; per-taxonomy uniqueness is enforced in the terms
-- service (terms/service.ts) with a numeric suffix, like post slugs.
DROP INDEX IF EXISTS idx_wp_terms_slug;
CREATE INDEX IF NOT EXISTS idx_wp_terms_slug ON wp_terms (slug);

-- ------------------------------------------------- indexes for M2 queries --
-- Revision lookup by parent post + type.
CREATE INDEX IF NOT EXISTS idx_wp_posts_parent_type ON wp_posts (post_parent, post_type);
-- Comment listing filtered by post and approval status.
CREATE INDEX IF NOT EXISTS idx_wp_comments_post_status ON wp_comments (comment_post_ID, comment_approved);
-- Term lookup by taxonomy + parent (hierarchical categories).
CREATE INDEX IF NOT EXISTS idx_wp_term_taxonomy_tax_parent ON wp_term_taxonomy (taxonomy, parent);

INSERT OR IGNORE INTO wl_migrations (id, applied_at) VALUES ('0002_m2', datetime('now'));
