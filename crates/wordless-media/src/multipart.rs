//! Hand-rolled `multipart/form-data` parser — pure Rust, no platform
//! dependency.
//!
//! The worker entry reads RAW request bytes (`req.bytes()` into
//! `ctx.body()`), so the platform's `formData()` is NOT available to
//! handlers. Upload routes must parse the multipart body themselves
//! from those bytes plus the `Content-Type` boundary. This module is
//! that parser. It works on bytes throughout so binary payloads pass
//! through byte-identically.

use wordless_kernel::error::{bad_request, WordlessError};

/// One uploaded file part.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    /// The form field name (`name="file"`).
    pub name: String,
    /// The client-supplied filename, when present.
    pub filename: Option<String>,
    /// The part's declared `Content-Type`, when present.
    pub content_type: Option<String>,
    /// The part's raw bytes, byte-identical to the request body.
    pub bytes: Vec<u8>,
}

/// A parsed multipart body.
#[derive(Debug, Clone, Default)]
pub struct MultipartForm {
    /// Text fields, in body order: `(name, value)`.
    pub fields: Vec<(String, String)>,
    /// File parts, in body order.
    pub files: Vec<UploadedFile>,
}

impl MultipartForm {
    /// First text field with this name, if any.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// First file part with this field name, if any.
    pub fn file(&self, name: &str) -> Option<&UploadedFile> {
        self.files.iter().find(|f| f.name == name)
    }
}

/// Extract the boundary from a `Content-Type` header value such as
/// `multipart/form-data; boundary=----WebKitFormBoundary7MA4YWxkTrZu0gW`.
/// The boundary may be quoted. Returns a 400 when the header is not
/// multipart or carries no boundary.
pub fn extract_boundary(content_type: &str) -> Result<String, WordlessError> {
    let lower = content_type.to_lowercase();
    if !lower.contains("multipart/form-data") {
        return Err(bad_request("Request is not multipart/form-data."));
    }
    for part in content_type.split(';') {
        let part = part.trim();
        if part.len() >= 9 && part[..9].eq_ignore_ascii_case("boundary=") {
            let mut value = part[9..].trim();
            if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
                value = &value[1..value.len() - 1];
            }
            if value.is_empty() {
                break;
            }
            return Ok(value.to_string());
        }
    }
    Err(bad_request("Multipart boundary is missing."))
}

/// Find `needle` in `haystack` starting at `from`. Byte search —
/// no UTF-8 assumptions, so binary bodies are safe.
fn find_subslice(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() || from > haystack.len() - needle.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Parse one part's headers (`Content-Disposition`, `Content-Type`,
/// …). Header names are case-insensitive; continuation lines are not
/// supported (browsers do not emit them for uploads).
fn parse_part_headers(raw: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(raw);
    let mut out = Vec::new();
    for line in text.split("\r\n") {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            out.push((name.trim().to_lowercase(), value.trim().to_string()));
        }
    }
    out
}

/// Parse a `Content-Disposition` value's `name="…"` / `filename="…"`
/// parameters. Handles quoted strings with `\"` and `\\` escapes.
fn disposition_param(value: &str, key: &str) -> Option<String> {
    let lower = value.to_lowercase();
    let needle = format!("{key}=");
    let pos = lower.find(&needle)?;
    let start = pos + needle.len();
    let rest = &value[start..];
    if let Some(quoted) = rest.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = quoted.chars();
        let mut closed = false;
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(esc) = chars.next() {
                        out.push(esc);
                    }
                }
                '"' => {
                    closed = true;
                    break;
                }
                _ => out.push(c),
            }
        }
        if closed {
            return Some(out);
        }
        return None;
    }
    // Unquoted token (rare): up to the next ';' or whitespace.
    let end = rest.find([';', ' ', '\t']).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Parse a whole multipart body. `content_type` is the request's
/// `Content-Type` header value; `body` is the raw request bytes.
pub fn parse_multipart(body: &[u8], content_type: &str) -> Result<MultipartForm, WordlessError> {
    let boundary = extract_boundary(content_type)?;
    // The delimiter that opens a part: "--" + boundary. The first
    // delimiter must be at the start of the body (possibly after a
    // preamble, which RFC 7578 allows — we skip to the first one).
    let delim = format!("--{boundary}").into_bytes();
    let Some(mut pos) = find_subslice(body, &delim, 0) else {
        return Err(bad_request("Multipart body is empty or malformed."));
    };
    pos += delim.len();
    // The first delimiter may be immediately followed by "--" (empty
    // body) or by CRLF.
    if body[pos..].starts_with(b"--") {
        return Ok(MultipartForm::default());
    }

    let mut form = MultipartForm::default();
    loop {
        // After a delimiter comes CRLF, then the part headers.
        if body[pos..].starts_with(b"\r\n") {
            pos += 2;
        } else if body[pos..].starts_with(b"\n") {
            pos += 1;
        } else {
            return Err(bad_request("Multipart body is malformed."));
        }
        // Headers end at the first blank line (CRLFCRLF or LFLF).
        let (header_end, header_sep_len) = if let Some(i) = find_subslice(body, b"\r\n\r\n", pos) {
            (i, 4)
        } else if let Some(i) = find_subslice(body, b"\n\n", pos) {
            (i, 2)
        } else {
            return Err(bad_request("Multipart part is missing its headers."));
        };
        let headers = parse_part_headers(&body[pos..header_end]);
        let content_start = header_end + header_sep_len;

        // The part's content runs to the CRLF before the next delimiter.
        // Search for "\r\n--boundary" first, then "\n--boundary".
        let (next_delim, delim_prefix_len) = match find_subslice(
            body,
            &{
                let mut d = b"\r\n".to_vec();
                d.extend_from_slice(&delim);
                d
            },
            content_start,
        ) {
            Some(i) => (i, 2),
            None => match find_subslice(
                body,
                &{
                    let mut d = b"\n".to_vec();
                    d.extend_from_slice(&delim);
                    d
                },
                content_start,
            ) {
                Some(i) => (i, 1),
                None => {
                    return Err(bad_request(
                        "Multipart body is truncated: closing boundary not found.",
                    ))
                }
            },
        };
        let content = &body[content_start..next_delim];
        let disposition = headers
            .iter()
            .find(|(n, _)| n == "content-disposition")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let name = disposition_param(&disposition, "name").unwrap_or_default();
        let filename = disposition_param(&disposition, "filename");
        let part_content_type = headers
            .iter()
            .find(|(n, _)| n == "content-type")
            .map(|(_, v)| v.clone());
        match filename {
            Some(filename) => form.files.push(UploadedFile {
                name,
                filename: Some(filename),
                content_type: part_content_type,
                bytes: content.to_vec(),
            }),
            None => form
                .fields
                .push((name, String::from_utf8_lossy(content).into_owned())),
        }

        // `next_delim` points at the CRLF/LF before "--boundary".
        pos = next_delim + delim_prefix_len + delim.len();
        if body[pos..].starts_with(b"--") {
            // Closing delimiter "--boundary--": done.
            return Ok(form);
        }
        // Otherwise the next part follows (the loop consumes its CRLF).
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDARY: &str = "----WebKitFormBoundary7MA4YWxkTrZu0gW";
    const CT: &str = "multipart/form-data; boundary=----WebKitFormBoundary7MA4YWxkTrZu0gW";

    #[allow(clippy::type_complexity)]
    fn build_body(parts: &[(&str, Option<&str>, Option<&str>, &[u8])]) -> Vec<u8> {
        // (field name, filename, content-type, bytes)
        let mut out = Vec::new();
        for (name, filename, ct, bytes) in parts {
            out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            match filename {
                Some(f) => {
                    out.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\n"
                        )
                        .as_bytes(),
                    );
                    out.extend_from_slice(
                        format!(
                            "Content-Type: {}\r\n",
                            ct.unwrap_or("application/octet-stream")
                        )
                        .as_bytes(),
                    );
                }
                None => {
                    out.extend_from_slice(
                        format!("Content-Disposition: form-data; name=\"{name}\"\r\n").as_bytes(),
                    );
                }
            }
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(bytes);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        out
    }

    #[test]
    fn parses_fields_and_a_binary_file_byte_identically() {
        // Binary payload: PNG magic + NULs + CR/LF bytes + a fake
        // delimiter-like sequence that is NOT the real boundary.
        let png: Vec<u8> = [
            vec![
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff, 0x00,
            ],
            b"\r\n--not-the-boundary\r\n".to_vec(),
            vec![0xde, 0xad, 0xbe, 0xef],
        ]
        .concat();
        let body = build_body(&[
            ("title", None, None, b"My Photo"),
            ("alt", None, None, "A café photo".as_bytes()),
            ("file", Some("photo.png"), Some("image/png"), &png),
        ]);
        let form = parse_multipart(&body, CT).unwrap();
        assert_eq!(form.field("title"), Some("My Photo"));
        assert_eq!(form.field("alt"), Some("A café photo"));
        let file = form.file("file").unwrap();
        assert_eq!(file.filename.as_deref(), Some("photo.png"));
        assert_eq!(file.content_type.as_deref(), Some("image/png"));
        assert_eq!(
            file.bytes, png,
            "binary payload must round-trip byte-identically"
        );
        assert_eq!(form.files.len(), 1);
    }

    #[test]
    fn parses_multiple_files_and_empty_fields() {
        let body = build_body(&[
            ("prefix", None, None, b"uploads"),
            ("file", Some("a.png"), Some("image/png"), b"\x89PNG"),
            ("file", Some("b.pdf"), Some("application/pdf"), b"%PDF-1.4"),
            ("note", None, None, b""),
        ]);
        let form = parse_multipart(&body, CT).unwrap();
        assert_eq!(form.field("prefix"), Some("uploads"));
        assert_eq!(form.field("note"), Some(""));
        assert_eq!(form.files.len(), 2);
        assert_eq!(form.files[0].filename.as_deref(), Some("a.png"));
        assert_eq!(form.files[1].filename.as_deref(), Some("b.pdf"));
        assert_eq!(form.files[1].bytes, b"%PDF-1.4");
    }

    #[test]
    fn boundary_extraction_handles_quoted_and_missing() {
        assert_eq!(
            extract_boundary("multipart/form-data; boundary=abc123").unwrap(),
            "abc123"
        );
        assert_eq!(
            extract_boundary("multipart/form-data; boundary=\"abc-123\"").unwrap(),
            "abc-123"
        );
        assert_eq!(
            extract_boundary("multipart/form-data; charset=utf-8; boundary=xyz").unwrap(),
            "xyz"
        );
        assert!(extract_boundary("application/json").is_err());
        assert!(extract_boundary("multipart/form-data").is_err());
        assert!(extract_boundary("multipart/form-data; boundary=").is_err());
    }

    #[test]
    fn rejects_truncated_and_non_multipart_bodies() {
        let body = build_body(&[("file", Some("a.png"), Some("image/png"), b"data")]);
        let truncated = &body[..body.len() - 20];
        assert!(parse_multipart(truncated, CT).is_err());
        assert!(parse_multipart(b"hello", CT).is_err());
        assert!(parse_multipart(&body, "application/json").is_err());
        // Empty body after the closing delimiter is fine.
        let empty_close = format!("--{BOUNDARY}--\r\n").into_bytes();
        assert!(parse_multipart(&empty_close, CT).unwrap().fields.is_empty());
    }

    #[test]
    fn filename_with_spaces_and_escaped_quotes() {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"my \\\"photo\\\".png\"\r\n",
        );
        body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
        body.extend_from_slice(b"data-bytes");
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        let form = parse_multipart(&body, CT).unwrap();
        assert_eq!(form.files[0].filename.as_deref(), Some("my \"photo\".png"));
        assert_eq!(form.files[0].bytes, b"data-bytes");
    }
}
