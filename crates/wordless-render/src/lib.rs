//! WORDLESS v2 public rendering (P2).
//!
//! Gutenberg block-comment HTML rendering, template hierarchy,
//! public pages, feeds (RSS). Port of the v1 modules under
//! `src/modules/{render,feeds,permalinks}/` in the v1.1.0 TypeScript
//! repo (read-only reference — same block parsing, same template
//! hierarchy, same Cache API rules; the Cache API itself is wired
//! by the P2 coordinator in `wordless-worker`).
//!
//! Modules:
//! - `blocks` — block-comment parser + renderer (pure logic).
//! - `templates` — template hierarchy resolution + block-theme
//!   template rendering over the active theme's files, including
//!   the allowlisted pattern-PHP evaluator.
//! - `pages` — home / single / page / archive / search / 404
//!   assembly (M2 layout fallback + themed rendering), plus the
//!   public path resolver (`resolve_path`).
//! - `feeds` — RSS generation (pure builder + DB query).
//! - `cache` — public-page cache RULES (max-age 60, purge lists).
//!   The Cache API itself is wired by the coordinator.
//!
//! Service functions take `db: &dyn Db` plus plain parameters and
//! return `Result<_, WordlessError>` (HTML as
//! `Result<String, WordlessError>` for render); route handlers live
//! in `wordless-worker` and are wired by the P2 coordinator.
//!
//! Notes on scope:
//! - `wordless-cms`, `wordless-themes` and `wordless-media` are
//!   still placeholder crates at P2 start, so the DB queries this
//!   crate needs (posts, terms, comments, menus, options) are made
//!   directly against D1 here with the same SQL v1 uses. When those
//!   crates land, the queries should move behind their APIs.
//! - Elementor: posts carrying `_elementor_data` are rendered from
//!   their block content only (see `pages::ELEMENTOR_GAP`).
//! - Shop pages (`render_shop`, cart, checkout) live in `shop`
//!   (P3, `wordless-commerce`).

pub mod blocks;
pub mod cache;
pub mod feeds;
pub mod pages;
pub mod shop;
pub mod templates;

#[cfg(test)]
mod test_support;
