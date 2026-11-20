//! WORDLESS v2 backup + restore (P3).
//!
//! Port of v1 `src/modules/backup/service.ts` (Milestone 9).
//!
//! A backup is a real ZIP stored in R2 under `backups/<id>.zip`,
//! with one JSON file per D1 table (rows as JSON), a
//! `media-manifest.json` (R2 keys + sizes), and a `manifest.json`
//! carrying the version, row counts, and a SHA-256 checksum per file.
//!
//! Excluded, by design (same as v1): `wl_backups` (the registry
//! itself — restoring it would erase the record of the backup being
//! restored) and `wl_jobs` (ephemeral background-job bookkeeping).
//!
//! A backup is only marked "verified" after `verify_backup` re-reads
//! the ZIP from R2 and recomputes every checksum. Restore refuses a
//! backup whose verification failed, refuses a different Wordless
//! minor version, and requires the typed confirmation string
//! `RESTORE`. Media files are NOT copied on restore — the media
//! manifest is checked key-by-key against R2 and every missing key
//! is reported, never silently skipped.
//!
//! Differences from v1, documented (not silent):
//!
//! * Checksums use the `sha2` crate directly (synchronous) instead
//!   of WebCrypto — same SHA-256, same hex output, no async crypto
//!   dependency on the backup path.
//! * The media manifest records `{key, size}` per object. v1 also
//!   records the R2 `etag`; the `ObjectStore` trait exposes no etag,
//!   so the manifest says so by omitting it rather than inventing
//!   one. The backup ZIP itself is (correctly) not listed in its own
//!   media manifest — it is stored after the manifest is built.
//! * The restore audit entry is written best-effort: audit logging
//!   must never fail a restore that already succeeded.
//!
//! ## Public API
//!
//! * [`service::create_backup`] —
//!   `pub async fn create_backup(db: &dyn Db, store: &dyn ObjectStore, created_by: i64, wordless_version: &str) -> Result<BackupSummary, WordlessError>`.
//!   Retention (keep the 10 most recent, configurable via the
//!   `wl_backup_retention` option) runs after every create.
//! * [`service::verify_backup`] —
//!   `pub async fn verify_backup(db: &dyn Db, store: &dyn ObjectStore, id: &str) -> Result<bool, WordlessError>`
//!   (checksum check); [`service::verify_backup_report`] returns the
//!   per-file [`service::VerifyReport`].
//! * [`service::restore_backup`] —
//!   `pub async fn restore_backup(db: &dyn Db, store: &dyn ObjectStore, id: &str, confirmation: &str, wordless_version: &str, actor_id: i64) -> Result<RestoreReport, WordlessError>`.
//!   Requires `confirmation == "RESTORE"` exactly, and verifies the
//!   checksum BEFORE restoring.
//! * [`service::list_backups`], [`service::get_backup_download`],
//!   [`service::prune_backups`], [`service::get_backup_retention`],
//!   [`service::run_backup_job`] (the `wl_jobs` runner, v1 shape).

pub mod service;

#[cfg(test)]
pub mod test_support;

pub use service::{
    create_backup, get_backup_download, get_backup_retention, list_backups, prune_backups,
    restore_backup, run_backup_job, verify_backup, verify_backup_report, BackupRow, BackupSummary,
    RestoreReport, VerifyReport, BACKUP_FORMAT_VERSION, BACKUP_RETENTION_DEFAULT,
};
