//! Platform-neutral object-storage interface (R2).
//!
//! The kernel defines the trait; the worker entry implements it over
//! the real R2 bucket. Services only ever see this interface, so the
//! storage backend stays replaceable and the logic stays testable.

use async_trait::async_trait;

use crate::error::WordlessError;

/// Metadata for one stored object (R2 key).
#[derive(Debug, Clone)]
pub struct ObjectMeta {
    pub key: String,
    pub size: u64,
    pub content_type: Option<String>,
    /// ISO-8601 UTC timestamp of upload, when the backend reports one.
    pub uploaded_at: Option<String>,
}

/// One page of a listing.
#[derive(Debug, Clone)]
pub struct ListPage {
    pub objects: Vec<ObjectMeta>,
    /// True when more objects exist beyond this page.
    pub truncated: bool,
    /// Opaque cursor for the next page, when `truncated` is true.
    pub cursor: Option<String>,
}

/// Object-store surface. P1 needed only `count_objects` (site-health);
/// P2 media/files need the full surface.
///
/// `?Send`, like the `Db` trait: R2 calls also return `JsFuture`,
/// which is `!Send`, and the Workers runtime is single-threaded.
#[async_trait(?Send)]
pub trait ObjectStore {
    /// Count objects up to `limit`. Returns `(counted, truncated)` —
    /// `truncated` is true when the bucket holds more than `limit`
    /// objects, so the count is reported as "at least N", never as an
    /// exact figure it is not (same rule as v1 `getPerformanceHealth`).
    async fn count_objects(&self, limit: usize) -> Result<(usize, bool), WordlessError>;

    /// Fetch an object's bytes plus its stored content type, or `None`
    /// when the key does not exist.
    async fn get_object(
        &self,
        key: &str,
    ) -> Result<Option<(Vec<u8>, Option<String>)>, WordlessError>;

    /// Store bytes under `key` with the given content type (overwrites).
    async fn put_object(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<(), WordlessError>;

    /// Delete a key. Deleting a missing key is a no-op success (same
    /// as R2 / v1 behaviour).
    async fn delete_object(&self, key: &str) -> Result<(), WordlessError>;

    /// Metadata for a key, or `None` when it does not exist.
    async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, WordlessError>;

    /// List objects under `prefix`, one page. `cursor` continues a
    /// previous page; `limit` is capped by the backend.
    async fn list_objects(
        &self,
        prefix: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<ListPage, WordlessError>;
}
