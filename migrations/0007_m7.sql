-- Milestone 7: content studio + SEO engine.
-- 404 log (public 404s only; query strings are never stored — see
-- src/modules/seo/notfound.ts). One row per path, with a hit counter.
-- Capped at 500 paths by the service (prune on insert). Additive only;
-- SEO fields live in wp_postmeta and robots.txt in wp_options, so no
-- other tables are needed.
CREATE TABLE IF NOT EXISTS wl_404_log (
  path TEXT PRIMARY KEY,
  count INTEGER NOT NULL DEFAULT 1,
  referrer TEXT NOT NULL DEFAULT '',
  first_seen TEXT NOT NULL,
  last_seen TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_404_log_count ON wl_404_log (count DESC, last_seen DESC);
