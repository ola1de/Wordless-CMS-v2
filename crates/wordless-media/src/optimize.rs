//! Video / document optimisation — the P2 mechanism decision.
//!
//! Doc §3 P2 requires "derivatives/poster frames for video, previews
//! for documents — mechanism decided at P2 start; no in-Worker video
//! transcoding is assumed". This module records the decision made at
//! P2 start, with the reasoning, and implements the honest variant.
//!
//! # Decision (2026-11-19, P2 start)
//!
//! | Kind | Decision | Reason |
//! | ---- | -------- | ------ |
//! | Images | Derivatives via the Cloudflare Images binding **when
//! bound**, original otherwise (never faked). | The Images binding
//! (`IMAGES`) is the v1 mechanism (derivatives.ts); it exists, is
//! documented, and is already the honest v1 behaviour. See
//! [`crate::service::resolve_media_request`]. |
//! | Video | Stored as-is with metadata; **no poster frame, no
//! transcoding in the Worker**. | A poster frame needs a video
//! decoder (ffmpeg or equivalent). No decoder runs inside a
//! Cloudflare Worker: there is no ffmpeg binding, no WASM video
//! decoder in this workspace, and the Worker has a 128 MB memory
//! limit against 50 MB uploads. Cloudflare Stream *could* transcode,
//! but it is a separate paid product with its own API/keys — not
//! bound here, not in `wrangler.toml`, and not verified. Claiming a
//! poster frame without one of those would be faked. |
//! | Documents | Stored as-is; the "preview" is a URL to the original
//! (PDFs render natively in browsers). **No in-Worker text
//! extraction, no thumbnail.** | The only document type the v1
//! allowlist admits is PDF (`application/pdf`). Extracting text or
//! rendering a thumbnail needs a PDF parser/renderer in WASM —
//! none is in this workspace, and adding one is a P3+ research
//! task, not a P2 port. |
//! | Audio / other | Stored as-is. | The v1 allowlist admits no audio
//! or other MIME types at all, so these cannot even enter the
//! bucket through the media or file-manager routes. |
//!
//! # A second, load-bearing fact
//!
//! The v1 MIME allowlist (`ALLOWED_MIME_TYPES`) admits ONLY images
//! (jpeg/png/gif/webp/avif/svg) and PDF. **No video MIME type is
//! uploadable through v1 at all** — a `video/mp4` upload is rejected
//! with `File type "video/mp4" is not allowed.` So the "video
//! optimisation" requirement, read literally, has no input in v1.
//! The honest P2 outcome is therefore: the delivery path is ready
//! for video (range-capable serving — see
//! [`crate::service::parse_range_header`] — plus stored-as-is
//! delivery below), and the day a video MIME is admitted, it is
//! delivered as-is with its metadata and a documented reason for
//! the missing poster frame. Admitting video MIME types is a
//! product decision for the parent, not something this crate
//! silently does.
//!
//! # P2 done-when
//!
//! "An optimised delivery variant OR a documented reason it
//! cannot" — both halves are delivered here: images get real
//! derivatives when the binding exists; video and documents get
//! the documented reason, implemented as stored-as-is delivery
//! with honest metadata, never a faked variant.

use serde::{Deserialize, Serialize};

use crate::service::MediaRecord;

/// What kind of media this is, by MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Document,
    Other,
}

pub fn classify_media(mime_type: &str) -> MediaKind {
    if mime_type.starts_with("image/") {
        MediaKind::Image
    } else if mime_type.starts_with("video/") {
        MediaKind::Video
    } else if mime_type.starts_with("audio/") {
        MediaKind::Audio
    } else if mime_type == "application/pdf"
        || mime_type.starts_with("application/msword")
        || mime_type == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        || mime_type == "text/plain"
    {
        MediaKind::Document
    } else {
        MediaKind::Other
    }
}

/// The optimisation decision for one media item, as data — what the
/// coordinator serves and what it tells the admin. Nothing here is a
/// faked variant: `variant` is always `"original"` except for images
/// when a real derivative exists (resolved by the service layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizedDelivery {
    pub media_id: i64,
    pub kind: MediaKind,
    /// `"original"` or `"derivative"` (derivatives only via the
    /// Images binding; see [`crate::service::resolve_media_request`]).
    pub variant: &'static str,
    /// URL to serve.
    pub url: String,
    /// Poster-frame URL for video. Always `None` in P2: no in-Worker
    /// video decoder exists (see the module docs).
    pub poster_url: Option<String>,
    /// Preview URL for documents. In P2 this is the original's URL:
    /// PDFs render natively in browsers, so the original IS the
    /// preview.
    pub preview_url: Option<String>,
    /// Honest note for the admin UI when the delivery is not
    /// optimised (empty when a real derivative is served).
    pub note: String,
}

/// The P2 optimisation decision, in one place, for the coordinator
/// and the admin UI to quote.
pub fn optimization_decision_text() -> &'static str {
    "Images: derivatives via the Cloudflare Images binding when bound, original otherwise (never faked). \
     Video: stored as-is with metadata — no in-Worker transcoding or poster frame exists (no video decoder runs in a Worker; Cloudflare Stream is a separate paid product, not bound). \
     Documents: stored as-is; the preview is the original (PDFs render natively in browsers) — no in-Worker text extraction or thumbnail. \
     Note: the v1 MIME allowlist admits no video MIME type at all, so video cannot currently be uploaded through the media or file-manager routes."
}

/// Decide the optimised delivery for a media record.
///
/// - Images: `variant` is `"derivative"` only when the caller passes
///   a real derived URL (i.e. the coordinator already resolved one
///   through the Images binding); otherwise `"original"` with a note
///   saying derivatives are unavailable when `images_bound` is false.
/// - Video: original, no poster frame, with the documented reason.
/// - Documents: original, preview URL = original URL.
/// - Anything else: original.
pub fn optimize_delivery(
    record: &MediaRecord,
    images_bound: bool,
    derived_url: Option<&str>,
) -> OptimizedDelivery {
    let kind = classify_media(&record.mime_type);
    match kind {
        MediaKind::Image => {
            if let Some(url) = derived_url {
                OptimizedDelivery {
                    media_id: record.id,
                    kind,
                    variant: "derivative",
                    url: url.to_string(),
                    poster_url: None,
                    preview_url: None,
                    note: String::new(),
                }
            } else {
                OptimizedDelivery {
                    media_id: record.id,
                    kind,
                    variant: "original",
                    url: record.url.clone(),
                    poster_url: None,
                    preview_url: None,
                    note: if images_bound {
                        "No derivative stored yet for this size/format.".to_string()
                    } else {
                        "Image derivatives are unavailable: the Cloudflare Images binding is not configured. Serving the original.".to_string()
                    },
                }
            }
        }
        MediaKind::Video => OptimizedDelivery {
            media_id: record.id,
            kind,
            variant: "original",
            url: record.url.clone(),
            poster_url: None,
            preview_url: None,
            note: "Video is stored as-is: no in-Worker video transcoding or poster-frame extraction exists (no video decoder runs in a Worker). Range requests can seek within the original.".to_string(),
        },
        MediaKind::Document => OptimizedDelivery {
            media_id: record.id,
            kind,
            variant: "original",
            url: record.url.clone(),
            poster_url: None,
            preview_url: Some(record.url.clone()),
            note: "Document preview is the original file (PDFs render natively in browsers). No in-Worker text extraction or thumbnail is available.".to_string(),
        },
        MediaKind::Audio | MediaKind::Other => OptimizedDelivery {
            media_id: record.id,
            kind,
            variant: "original",
            url: record.url.clone(),
            poster_url: None,
            preview_url: None,
            note: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::MediaRecord;

    fn record(id: i64, mime: &str) -> MediaRecord {
        MediaRecord {
            id,
            title: "t".to_string(),
            slug: "t".to_string(),
            mime_type: mime.to_string(),
            object_key: format!("abc12345-file-{id}"),
            author_id: 1,
            date: "2026-11-19T00:00:00.000Z".to_string(),
            url: format!("/media/abc12345-file-{id}"),
            filename: format!("file-{id}"),
            size: 10,
            alt: String::new(),
        }
    }

    #[test]
    fn classification_by_mime() {
        assert_eq!(classify_media("image/png"), MediaKind::Image);
        assert_eq!(classify_media("image/svg+xml"), MediaKind::Image);
        assert_eq!(classify_media("video/mp4"), MediaKind::Video);
        assert_eq!(classify_media("audio/mpeg"), MediaKind::Audio);
        assert_eq!(classify_media("application/pdf"), MediaKind::Document);
        assert_eq!(classify_media("application/octet-stream"), MediaKind::Other);
    }

    #[test]
    fn video_never_gets_a_fake_poster_frame() {
        let out = optimize_delivery(&record(7, "video/mp4"), false, None);
        assert_eq!(out.variant, "original");
        assert_eq!(out.poster_url, None);
        assert!(out.note.contains("no in-Worker video transcoding"));
    }

    #[test]
    fn document_preview_is_the_original() {
        let out = optimize_delivery(&record(8, "application/pdf"), false, None);
        assert_eq!(out.variant, "original");
        assert_eq!(out.preview_url.as_deref(), Some("/media/abc12345-file-8"));
        assert!(out.note.contains("preview is the original"));
    }

    #[test]
    fn image_unbound_is_original_with_honest_note() {
        let out = optimize_delivery(&record(9, "image/png"), false, None);
        assert_eq!(out.variant, "original");
        assert!(out.note.contains("Images binding is not configured"));
        let with_derivative = optimize_delivery(
            &record(9, "image/png"),
            true,
            Some("/media/abc12345-file-9?width=640&format=webp"),
        );
        assert_eq!(with_derivative.variant, "derivative");
        assert!(with_derivative.note.is_empty());
    }

    #[test]
    fn decision_text_states_all_three_mechanisms() {
        let text = optimization_decision_text();
        assert!(text.contains("Images"));
        assert!(text.contains("Video"));
        assert!(text.contains("Documents"));
        assert!(text.contains("no video MIME type"));
    }
}
