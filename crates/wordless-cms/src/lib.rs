//! WORDLESS v2 core CMS (P2).
//!
//! Posts, revisions, terms, comments, menus, search, permalinks,
//! options, settings, activity, custom post types and custom
//! taxonomies. Port of the v1 modules under
//! `src/modules/{posts,revisions,terms,comments,menus,search,
//! permalinks,options,settings,activity}/` in the v1.1.0 TypeScript
//! repo (read-only reference — same SQL, same bound parameters, same
//! response shapes).
//!
//! Service functions take `db: &dyn Db` and plain parameters and
//! return typed `Result<T, WordlessError>` (serialisable to the v1
//! JSON shapes with `serde_json::to_value`); route handlers live in
//! `wordless-worker` and are wired by the P2 coordinator.
//!
//! Custom post types and custom taxonomies are NEW in v2 (v1's
//! `register_post_type` / `register_taxonomy` were in-memory stubs):
//! they persist as JSON in `wp_options` under `wl_post_types` and
//! `wl_taxonomies` — no new migration. See `post_types` /
//! `taxonomies` for the exact option keys and JSON shapes.

pub mod activity;
pub mod comments;
pub mod common;
pub mod login_branding;
pub mod menus;
pub mod options;
pub mod permalinks;
pub mod post_types;
pub mod postmeta;
pub mod posts;
pub mod revisions;
pub mod scheduling;
pub mod search;
pub mod settings;
pub mod taxonomies;
pub mod terms;
pub(crate) mod timezones;

#[cfg(test)]
pub(crate) mod service_tests;
#[cfg(test)]
pub(crate) mod test_db;
