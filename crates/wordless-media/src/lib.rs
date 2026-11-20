//! WORDLESS v2 media (P2).
//!
//! R2 uploads, listing, file manager operations, image derivatives,
//! video/document optimisation. Port of the v1 modules under
//! `src/modules/{media,files}/` in the v1.1.0 TypeScript repo
//! (read-only reference — same keys, same caps, same honest
//! "unbound → serve original" behaviour for the Images binding) plus
//! the derivatives half of `src/modules/performance/derivatives.ts`.
//!
//! Service functions take `db: &dyn Db` and `store: &dyn ObjectStore`
//! plus plain parameters and return `Result<_, WordlessError>` (or
//! typed results); route handlers live in `wordless-worker` and are
//! wired by the P2 coordinator.
//!
//! Modules: `magic` (magic-byte sniffing), `multipart` (hand-rolled
//! multipart/form-data parser — pure Rust; the worker entry reads raw
//! request bytes, so platform form_data is NOT available to
//! handlers), `service` (upload/list/get/update/delete, derivatives,
//! serving), `files` (file manager), `optimize` (video/document
//! optimisation decision + implementation).

pub mod files;
pub mod magic;
pub mod multipart;
pub mod optimize;
pub mod service;

#[cfg(test)]
mod test_support;

pub use files::{
    delete_file, download_file, list_files, rename_file, upload_file, validate_file_key,
    validate_file_name, validate_prefix, DeleteFileResult, DownloadedFile, FileEntry,
    FileListResult, UploadFileInput, MANAGED_PREFIXES, MAX_FILE_LIST_ENTRIES,
};
pub use magic::{sniff_mime, sniff_svg, verify_upload_bytes};
pub use multipart::{extract_boundary, parse_multipart, MultipartForm, UploadedFile};
pub use optimize::{
    classify_media, optimization_decision_text, optimize_delivery, MediaKind, OptimizedDelivery,
};
pub use service::{
    build_srcset, delete_media, derivative_key, derivative_status, find_media_by_key,
    get_media_record, is_allowed_mime_type, is_valid_derivative_format, is_valid_derivative_width,
    list_derivatives, list_media, media_url_for_key, object_key_for, parse_media_id,
    parse_range_header, rename_media_object, resolve_media_request, safe_filename,
    serve_stored_object, slugify, store_derivative, update_media_alt, upload_media,
    validate_upload, ByteRange, DerivativeRecord, ListMediaOpts, MediaListResult, MediaRecord,
    MediaResolution, ServedMedia, UploadMediaInput, ALLOWED_MIME_TYPES,
    DERIVATIVES_UNAVAILABLE_HEADER, DERIVATIVE_FORMATS, DERIVATIVE_WIDTHS, MAX_UPLOAD_BYTES,
    MEDIA_CACHE_CONTROL,
};
