//! Magic-byte sniffing for uploads.
//!
//! Port of v1 `src/modules/security/magic.ts` (Milestone 9) — the
//! sniffing table is copied exactly: signatures → MIME, extension →
//! expected MIME, SVG checked by content. A file whose bytes
//! contradict its declared MIME is rejected with v1's error messages.

use wordless_kernel::error::{bad_request, WordlessError};

/// One sniffable signature: either a fixed prefix or a predicate.
struct Signature {
    mime: &'static str,
    prefix: Option<&'static [u8]>,
    test: Option<fn(&[u8]) -> bool>,
}

fn starts_with(data: &[u8], prefix: &[u8]) -> bool {
    data.len() >= prefix.len() && data[..prefix.len()] == *prefix
}

fn is_webp(d: &[u8]) -> bool {
    d.len() >= 12 && starts_with(d, b"RIFF") && starts_with(&d[8..12], b"WEBP")
}

fn is_avif(d: &[u8]) -> bool {
    d.len() >= 12
        && starts_with(&d[4..8], b"ftyp")
        && (starts_with(&d[8..12], b"avif") || starts_with(&d[8..12], b"avis"))
}

/// The v1 signature table (`SIGNATURES` in magic.ts), in the same order.
const SIGNATURES: &[Signature] = &[
    Signature {
        mime: "image/jpeg",
        prefix: Some(&[0xff, 0xd8, 0xff]),
        test: None,
    },
    Signature {
        mime: "image/png",
        prefix: Some(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
        test: None,
    },
    Signature {
        mime: "image/gif",
        prefix: Some(&[0x47, 0x49, 0x46, 0x38]),
        test: None,
    },
    Signature {
        mime: "image/webp",
        prefix: None,
        test: Some(is_webp),
    },
    Signature {
        mime: "image/avif",
        prefix: None,
        test: Some(is_avif),
    },
    Signature {
        mime: "application/pdf",
        prefix: Some(&[0x25, 0x50, 0x44, 0x46]),
        test: None,
    },
];

/// Extension → expected MIME (v1 `EXTENSION_MIME`).
pub fn expected_mime_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "svg" => Some("image/svg+xml"),
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

/// Sniff the MIME of `data` from its magic bytes, or `None` when no
/// signature matches. SVG is text and has no magic bytes, so it is
/// never returned here — use [`sniff_svg`] for it.
pub fn sniff_mime(data: &[u8]) -> Option<&'static str> {
    for sig in SIGNATURES {
        let ok = match (sig.prefix, sig.test) {
            (Some(prefix), _) => starts_with(data, prefix),
            (None, Some(test)) => test(data),
            (None, None) => false,
        };
        if ok {
            return Some(sig.mime);
        }
    }
    None
}

/// Does `data` start with the magic bytes for `mime`?
pub fn matches_mime(data: &[u8], mime: &str) -> bool {
    sniff_mime(data) == Some(mime)
}

fn has_script_tag(lower: &str) -> bool {
    // v1: /<script[\s>]|/ — "<script" followed by whitespace or ">".
    let mut start = 0;
    while let Some(idx) = lower[start..].find("<script") {
        let after = start + idx + "<script".len();
        match lower.as_bytes().get(after) {
            Some(b) if *b == b'>' || b.is_ascii_whitespace() => return true,
            _ => start = after,
        }
    }
    false
}

fn has_javascript_url(lower: &str) -> bool {
    // v1: /javascript\s*:/ — "javascript", optional whitespace, ":".
    let mut start = 0;
    while let Some(idx) = lower[start..].find("javascript") {
        let mut i = start + idx + "javascript".len();
        while lower
            .as_bytes()
            .get(i)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            i += 1;
        }
        if lower.as_bytes().get(i) == Some(&b':') {
            return true;
        }
        start = i;
    }
    false
}

fn has_event_handler(lower: &str) -> bool {
    // v1: /on\w+\s*=/ — "on" + word chars + optional whitespace + "=".
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'o'
            && bytes[i + 1] == b'n'
            && (bytes[i + 2].is_ascii_alphanumeric() || bytes[i + 2] == b'_')
        {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                return true;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    false
}

/// Sniff an SVG's text (v1 `sniffSvg`): it must actually contain an
/// `<svg>` element and must not contain scripts, `javascript:` URLs or
/// inline event handlers. Only the first 4096 bytes are inspected,
/// exactly as in v1.
pub fn sniff_svg(data: &[u8]) -> bool {
    let head_len = data.len().min(4096);
    let head = String::from_utf8_lossy(&data[..head_len]);
    // Strip an optional XML declaration and one optional comment,
    // exactly as v1's /^\s*(<\?xml[^>]*\?>\s*)?(<!--[\s\S]*?-->\s*)?/ does.
    let mut rest = head.trim_start();
    if let Some(after) = rest.strip_prefix("<?xml") {
        if let Some(end) = after.find("?>") {
            rest = after[end + 2..].trim_start();
        }
    }
    if let Some(after) = rest.strip_prefix("<!--") {
        if let Some(end) = after.find("-->") {
            rest = after[end + 3..].trim_start();
        }
    }
    // Must start with "<svg" followed by whitespace or ">".
    let ok_start = rest.starts_with("<svg")
        && rest
            .as_bytes()
            .get(4)
            .is_some_and(|b| *b == b'>' || b.is_ascii_whitespace());
    if !ok_start {
        return false;
    }
    let lower = head.to_lowercase();
    !(has_script_tag(&lower) || has_javascript_url(&lower) || has_event_handler(&lower))
}

/// Verify that `data` really is what `mime_type` + `filename` claim
/// (v1 `verifyUploadBytes`). Throws a 400 on any mismatch. SVG is
/// checked by content; other types by magic bytes. Unknown extensions
/// are rejected.
///
/// Error messages are v1's, byte for byte.
pub fn verify_upload_bytes(
    filename: &str,
    mime_type: &str,
    data: &[u8],
) -> Result<(), WordlessError> {
    // v1: filename.split(".").pop() — for "file" (no dot) the extension
    // is "file" itself; for "a." it is "".
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    let expected = match expected_mime_for_extension(&ext) {
        Some(m) => m,
        None => {
            return Err(bad_request(&format!(
                "File extension \".{ext}\" is not allowed."
            )))
        }
    };
    if expected != mime_type {
        return Err(bad_request(&format!(
            "File extension \".{ext}\" does not match the declared type \"{mime_type}\"."
        )));
    }
    if mime_type == "image/svg+xml" {
        if !sniff_svg(data) {
            return Err(bad_request(
                "SVG file failed its content check (not a plain SVG document, or it contains scripts).",
            ));
        }
        return Ok(());
    }
    let sig = SIGNATURES.iter().find(|s| s.mime == mime_type);
    let Some(sig) = sig else {
        return Err(bad_request(&format!(
            "File type \"{mime_type}\" cannot be verified."
        )));
    };
    let ok = match (sig.prefix, sig.test) {
        (Some(prefix), _) => starts_with(data, prefix),
        (None, Some(test)) => test(data),
        (None, None) => false,
    };
    if !ok {
        return Err(bad_request(&format!(
            "File contents do not match the declared type \"{mime_type}\" — the file may have been renamed."
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46];
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
    ];
    const GIF: &[u8] = b"GIF89a................";
    const WEBP: &[u8] = b"RIFF\x24\x00\x00\x00WEBPVP8 ";
    const AVIF: &[u8] = b"\x00\x00\x00\x18ftypavif....";
    const AVIS: &[u8] = b"\x00\x00\x00\x18ftypavis....";
    const PDF: &[u8] = b"%PDF-1.4............";
    const PHP: &[u8] = b"<?php evil(); ?>";
    const SVG_CLEAN: &[u8] =
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"10\" height=\"10\"/></svg>";
    const SVG_SCRIPT: &[u8] =
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";
    const SVG_ONCLICK: &[u8] = b"<svg onclick=\"x()\"><rect/></svg>";
    const SVG_JS_HREF: &[u8] = b"<svg><a href=\"javascript:alert(1)\"><text>x</text></a></svg>";
    const SVG_WITH_DECL: &[u8] =
        b"<?xml version=\"1.0\"?><!-- a comment --><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";

    #[test]
    fn sniff_table_matches_v1_signatures() {
        assert_eq!(sniff_mime(JPEG), Some("image/jpeg"));
        assert_eq!(sniff_mime(PNG), Some("image/png"));
        assert_eq!(sniff_mime(GIF), Some("image/gif"));
        assert_eq!(sniff_mime(WEBP), Some("image/webp"));
        assert_eq!(sniff_mime(AVIF), Some("image/avif"));
        assert_eq!(sniff_mime(AVIS), Some("image/avif"));
        assert_eq!(sniff_mime(PDF), Some("application/pdf"));
        assert_eq!(sniff_mime(PHP), None);
        assert_eq!(sniff_mime(b""), None);
        // Truncated magic is not a match.
        assert_eq!(sniff_mime(&PNG[..4]), None);
        // RIFF without WEBP is not WebP.
        assert_eq!(sniff_mime(b"RIFF\x24\x00\x00\x00XXXX"), None);
    }

    #[test]
    fn verify_accepts_matching_files() {
        assert!(verify_upload_bytes("photo.jpg", "image/jpeg", JPEG).is_ok());
        assert!(verify_upload_bytes("photo.png", "image/png", PNG).is_ok());
        assert!(verify_upload_bytes("a.gif", "image/gif", GIF).is_ok());
        assert!(verify_upload_bytes("a.webp", "image/webp", WEBP).is_ok());
        assert!(verify_upload_bytes("a.avif", "image/avif", AVIF).is_ok());
        assert!(verify_upload_bytes("doc.pdf", "application/pdf", PDF).is_ok());
        assert!(verify_upload_bytes("icon.svg", "image/svg+xml", SVG_CLEAN).is_ok());
        assert!(verify_upload_bytes("icon.svg", "image/svg+xml", SVG_WITH_DECL).is_ok());
        // Extension case is ignored.
        assert!(verify_upload_bytes("photo.PNG", "image/png", PNG).is_ok());
        assert!(verify_upload_bytes("photo.JPEG", "image/jpeg", JPEG).is_ok());
    }

    #[test]
    fn verify_rejects_renamed_and_mismatched_files() {
        // PHP renamed to .jpg — the M9 case.
        let err = verify_upload_bytes("evil.jpg", "image/jpeg", PHP).unwrap_err();
        assert!(err.message.contains("do not match"), "{err:?}");
        assert_eq!(err.status, 400);
        let err = verify_upload_bytes("evil.png", "image/png", PHP).unwrap_err();
        assert!(err.message.contains("do not match"), "{err:?}");
        // Declared MIME contradicts the extension.
        let err = verify_upload_bytes("photo.png", "image/jpeg", JPEG).unwrap_err();
        assert!(err.message.contains("does not match"), "{err:?}");
        // Unknown extension.
        let err = verify_upload_bytes("photo.exe", "application/pdf", PDF).unwrap_err();
        assert!(err.message.contains("not allowed"), "{err:?}");
        // No extension at all: v1 reports ".file" as the extension.
        let err = verify_upload_bytes("file", "image/png", PNG).unwrap_err();
        assert_eq!(err.message, "File extension \".file\" is not allowed.");
    }

    #[test]
    fn svg_content_check_rejects_scripts_and_handlers() {
        assert!(sniff_svg(SVG_CLEAN));
        assert!(!sniff_svg(SVG_SCRIPT));
        assert!(!sniff_svg(SVG_ONCLICK));
        assert!(!sniff_svg(SVG_JS_HREF));
        assert!(sniff_svg(SVG_WITH_DECL));
        assert!(!sniff_svg(b"not svg at all"));
        assert!(!sniff_svg(b"<svgx></svgx>"));
        let err = verify_upload_bytes("icon.svg", "image/svg+xml", SVG_SCRIPT).unwrap_err();
        assert!(err.message.contains("content check"), "{err:?}");
    }
}
