//! WORDLESS v2 SEO (P2).
//!
//! Per-post SEO fields, head rendering, deterministic SEO analysis,
//! sitemap + robots.txt, redirect manager + 404 log, content studio
//! (templates + calendar), and Yoast / Rank Math metadata import.
//! Port of the v1 module under `src/modules/seo/` in the v1.1.0
//! TypeScript repo (read-only reference — the exact scoring formula:
//! an empty post scores exactly 50; same `_wl_seo_*` postmeta keys,
//! same sitemap index + pagination, same redirect + 404-log tables).
//!
//! Modules: `analyze` (deterministic scorer, pure logic),
//! `sitemap` (index + pagination), `robots` (configurable
//! robots.txt), `redirects` (redirect manager + 404-log read side),
//! `studio` (content templates + editorial calendar), `import`
//! (Yoast / Rank Math import).
//!
//! The render side — `head` (title / meta / canonical / robots /
//! OG / Twitter / JSON-LD rendering), `jsonld` (structured-data
//! builders + validator), `meta` (per-post `_wl_seo_*` fields +
//! FAQ), `site` (site base URL), `not_found` (public 404 logging)
//! — lives in `wordless-seo-core` (2026-11-20 SEO split): the main
//! Worker renders page heads in-process, so those modules stay
//! linked by main; this crate is the API surface that runs in the
//! standalone `wordless-seo-worker`.
//!
//! Service functions take `db: &dyn Db` plus plain parameters and
//! return `Result<_, WordlessError>` (typed results); route handlers
//! live in `wordless-worker` and are wired by the P2 coordinator.
//!
//! Out of scope (P3 / other milestones): Elementor content rendering
//! (see `wordless-render`'s gap note) and the M6 AI seo-auditor agent
//! (v1 `seo.test.ts` "seo-auditor agent uses the engine" — that agent
//! lives in the AI orchestrator, not in this crate).

pub mod analyze;
pub mod audit;
pub mod cwv;
pub mod import;
pub mod llms;
pub mod redirects;
pub mod robots;
pub mod sitemap;
pub mod studio;

#[cfg(test)]
mod test_support;
