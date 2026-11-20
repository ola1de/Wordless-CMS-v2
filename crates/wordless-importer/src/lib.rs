//! WORDLESS v2 WordPress importer (P3).
//!
//! Port of v1 `src/modules/importer/` (Milestone 9): WXR
//! (WordPress eXtended RSS) and mysqldump imports, plus the safe PHP
//! unserializer. Data only — the importer parses XML and SQL data, it
//! never evals PHP or executes imported code.
//!
//! Rules (same as v1, from `docs/02-architecture.md` and
//! `docs/import/wordpress-import.md`):
//!
//! * WordPress phpass password hashes are NOT portable — every
//!   imported user gets an empty (unusable) password + the
//!   forced-reset flag `wl_password_reset_required`; they cannot log
//!   in until they reset.
//! * PHP-serialized values in options/meta become JSON via the safe
//!   unserializer; serialized OBJECTS are refused and recorded as
//!   failed (never eval'd).
//! * Block content is preserved verbatim (`post_content` stored as-is).
//! * WordPress taxonomy `post_tag` maps to Wordless taxonomy `tag`.
//! * Original IDs are kept when free, so relationships survive; when
//!   an ID collides the row gets a new ID and the mapping is recorded.
//! * Every item lands in the report as imported / skipped / failed
//!   with a reason — nothing is silently dropped.
//!
//! ## WXR parsing without quick-xml
//!
//! The architecture doc (§2.2) names `quick-xml` for WXR, but that
//! crate is **not available offline** in this VM (crates.io is
//! blocked, and it is not in `Cargo.lock`). `wxr` therefore contains
//! a small hand-rolled XML reader (~200 lines) for the WXR subset
//! only (`rss`/`channel`/`item`, titles, `content:encoded`,
//! `wp:post_type`/`post_status`/`post_name`/`post_date`, category/tag
//! terms, `wp:comment`). It is a real tokenizer — never regex —
//! handling elements, attributes, CDATA, comments, processing
//! instructions, and numeric/named entities. It is not a general
//! XML parser and makes no claim to be one.
//!
//! ## Public API
//!
//! * [`service::import_wxr`] —
//!   `pub async fn import_wxr(db: &dyn Db, xml: &str, admin_id: i64) -> Result<ImportReport, WordlessError>`.
//!   Attachments are recorded `skipped` (no fetcher configured).
//!   [`service::import_wxr_with_media`] is the same with an
//!   [`service::AttachmentFetcher`] + an `ObjectStore`, so attachment
//!   files land in R2 under `media/imported-<id>-<slug>.<ext>`.
//! * [`service::import_sql`] —
//!   `pub async fn import_sql(db: &dyn Db, sql: &str, admin_id: i64) -> Result<ImportReport, WordlessError>`.
//! * [`service::ImportReport`] carries [`service::ImportCounts`]
//!   (`posts`, `pages`, `users`, `terms`, `comments`, `skipped`) plus
//!   the v1-shaped totals and per-type counts.
//! * [`php::safe_unserialize`], [`sql::parse_sql_dump`],
//!   [`wxr::parse_wxr`] — the pure parsers, unit-tested directly.
//!
//! `admin_id` (not in the task's shorthand signature) is the fallback
//! author for items whose creator was not in the export; the v1
//! signature takes the same value. Import jobs (`wl_jobs`, kind
//! `import_wxr` / `import_sql`) are worker wiring, not part of this
//! crate — the worker reads the uploaded file from R2 and calls these
//! functions, as v1 `runImportJob` does.

pub mod php;
pub mod service;
pub mod sql;
pub mod wxr;

#[cfg(test)]
pub mod test_support;

pub use php::{safe_unserialize, PhpUnserializeError};
pub use service::{import_sql, import_wxr, import_wxr_with_media, ImportCounts, ImportReport};
pub use sql::parse_sql_dump;
pub use wxr::parse_wxr;
