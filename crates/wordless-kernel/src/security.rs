//! Security headers, applied to EVERY response.
//! (Port of v1 `src/modules/security/headers.ts` — byte-identical values.
//! The `script-src 'unsafe-inline'` caveat from v1 carries over: a
//! nonce-based policy is a later hardening step, not claimed here.)

pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
    script-src 'self' 'unsafe-inline' https://www.googletagmanager.com https://www.google-analytics.com https://plausible.io; \
    style-src 'self' 'unsafe-inline'; \
    img-src 'self' data: https:; \
    font-src 'self' data:; \
    connect-src 'self' https://www.google-analytics.com https://plausible.io; \
    media-src 'self'; \
    object-src 'none'; \
    base-uri 'self'; \
    form-action 'self'; \
    frame-ancestors 'none'";

/// The full set of security headers, as name/value pairs.
pub fn security_headers() -> Vec<(String, String)> {
    vec![
        (
            "Content-Security-Policy".to_string(),
            CONTENT_SECURITY_POLICY.to_string(),
        ),
        ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
        ("X-Frame-Options".to_string(), "DENY".to_string()),
        (
            "Referrer-Policy".to_string(),
            "strict-origin-when-cross-origin".to_string(),
        ),
        (
            "Permissions-Policy".to_string(),
            "camera=(), microphone=(), geolocation=()".to_string(),
        ),
        ("Vary".to_string(), "Accept-Encoding".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_cover_the_v1_set() {
        let headers = security_headers();
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        for expected in [
            "Content-Security-Policy",
            "X-Content-Type-Options",
            "X-Frame-Options",
            "Referrer-Policy",
            "Permissions-Policy",
            "Vary",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
        let csp = headers
            .iter()
            .find(|(n, _)| n == "Content-Security-Policy")
            .unwrap()
            .1
            .clone();
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("object-src 'none'"));
    }
}
