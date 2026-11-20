//! Small pure helpers shared by the integrations modules.
//!
//! No regex crate (not in the workspace), no `url` crate — the
//! checks below are hand-rolled to the exact shapes v1 needs.

use wordless_db::{options::get_option, Db};
use wordless_kernel::WordlessError;

/// Site base URL (option `siteurl`, default
/// `http://localhost:8787`, trailing slashes stripped) — same as v1
/// `feeds/service.ts` and the P2 `wordless-seo` helper.
pub async fn site_url(db: &dyn Db) -> Result<String, WordlessError> {
    let url: String = get_option(db, "siteurl", "http://localhost:8787".to_string())
        .await
        .map_err(WordlessError::from)?;
    Ok(url.trim_end_matches('/').to_string())
}

/// Host part of an absolute URL (`https://example.com/a` →
/// `example.com`), or `""` when it cannot be parsed. Ports are kept
/// (`localhost:8787` stays intact); userinfo, path, query and
/// fragment are dropped.
pub fn url_host(url: &str) -> String {
    let after_scheme = match url.find("://") {
        Some(i) => &url[i + 3..],
        None => return String::new(),
    };
    let end = after_scheme
        .find(|c| ['/', '?', '#'].contains(&c))
        .unwrap_or(after_scheme.len());
    let host = &after_scheme[..end];
    // A host with whitespace or an empty host is not a host.
    if host.is_empty() || host.chars().any(|c| c.is_whitespace()) {
        return String::new();
    }
    host.to_string()
}

/// True when `url` is a non-empty absolute `http:`/`https:` URL with
/// a host — the v1 `apiUrl` rule (empty is allowed separately by the
/// caller; this function answers "is it a valid URL").
pub fn is_valid_http_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return false;
    }
    !url_host(trimmed).is_empty()
}

/// HTML-escape for attribute/text interpolation (same five
/// replacements as the P2 crates' `escape_html`).
pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Percent-encode like JavaScript `encodeURIComponent`: UTF-8 bytes,
/// leaving `A-Z a-z 0-9 - _ . ! ~ * ' ( )` unescaped, everything else
/// as uppercase `%XX`. Used for the social share links.
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        let ok = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if ok {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_extraction() {
        assert_eq!(url_host("https://example.com/a?b=c"), "example.com");
        assert_eq!(url_host("http://localhost:8787/"), "localhost:8787");
        assert_eq!(url_host("https://user@host.com/x"), "user@host.com");
        assert_eq!(url_host("not a url"), "");
        assert_eq!(url_host("https:///x"), "");
    }

    #[test]
    fn http_url_validation() {
        assert!(is_valid_http_url("https://api.example.com/v1/send"));
        assert!(is_valid_http_url("http://localhost:3000/hook"));
        assert!(!is_valid_http_url("not a url"));
        assert!(!is_valid_http_url("ftp://example.com/x"));
        assert!(!is_valid_http_url("https://"));
        assert!(!is_valid_http_url(""));
        assert!(!is_valid_http_url("https://exa mple.com"));
    }

    #[test]
    fn encode_matches_encode_uri_component() {
        assert_eq!(percent_encode("Hello World!"), "Hello%20World!");
        assert_eq!(
            percent_encode("https://example.com/a post?x=1&y=2"),
            "https%3A%2F%2Fexample.com%2Fa%20post%3Fx%3D1%26y%3D2"
        );
        assert_eq!(
            percent_encode("don't (stop) *now* ~ok"),
            "don't%20(stop)%20*now*%20~ok"
        );
    }

    #[test]
    fn html_escape_basics() {
        assert_eq!(
            escape_html("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }
}
