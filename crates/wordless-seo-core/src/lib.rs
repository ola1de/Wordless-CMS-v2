//! WORDLESS v2 SEO core (2026-11-20 SEO split).
//!
//! The SEO primitives that must stay linked by the main Worker —
//! page-head rendering (title/meta/canonical/robots/OG/Twitter/
//! JSON-LD), per-post SEO fields, the site base URL, and public 404
//! logging — shared by the main Worker (public page rendering +
//! 404 logging), the school Worker (course pages: breadcrumbs +
//! Course JSON-LD), and the SEO Worker (structured-data preview API).
//!
//! Everything else SEO (sitemaps, robots.txt serving, llms.txt,
//! redirects CRUD, SEO analysis, content studio, CWV budgets, audit
//! runs, metadata import) lives in `wordless-seo` and runs in the
//! standalone `wordless-seo-worker` behind the main Worker's `SEO`
//! service binding. Split rule: a module moves to the SEO Worker
//! unless a public page render or the main request path needs it
//! in-process.
//!
//! Modules: `jsonld` (structured-data builders + validator),
//! `head` (page SEO head + breadcrumbs), `meta` (per-post
//! `_wl_seo_*` fields + FAQ), `site` (site base URL), `not_found`
//! (public 404 log).

pub mod head;
pub mod jsonld;
pub mod meta;
pub mod not_found;
pub mod site;

#[cfg(test)]
mod test_support;
