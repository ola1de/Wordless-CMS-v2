//! Public-page cache rules (P2).
//!
//! Port of v1 `src/modules/performance/cache.ts` (Milestone 9) — the
//! RULES only. The Cache API itself is **not** implemented here: it
//! lives in the Worker runtime and is wired by the P2 coordinator in
//! `wordless-worker` (see `worker::Cache` findings in the P2 report).
//!
//! Rules (v1, unchanged since M2):
//! - Anonymous GETs of public pages are cached for 60 s with
//!   `stale-while-revalidate=60`, so a reader never waits on a
//!   regeneration while a fresh copy is being fetched.
//! - Publishing (or changing) a post purges its own URL, the home
//!   page, and the affected term pages from the Cache API. The purge
//!   uses the same `cache.match(url)` keying the render layer uses,
//!   so it hits.
//! - A purge failure never fails the publish — it is logged and the
//!   entry expires on its own after 60 s.
//! - Logged-in traffic is never cached; API/admin traffic is never
//!   cached; preview renders (`?wl_preview=`) are never cached.
//! - Only 200 responses are stored.
//!
//! Known limit (v1, carried over — see cache.ts header): the purge
//! covers API post create/update only, NOT imports. A post created
//! by a WXR/SQL import (or any other path that writes `wp_posts`
//! directly) does **not** purge the cache; the stale entry expires
//! after 60 s on its own.

/// `Cache-Control` max-age for anonymous public pages (seconds).
pub const PUBLIC_CACHE_MAX_AGE: u32 = 60;
/// `stale-while-revalidate` window (seconds).
pub const PUBLIC_CACHE_STALE_WHILE_REVALIDATE: u32 = 60;

/// The `Cache-Control` value the coordinator sets on cacheable
/// public 200 responses before `cache.put`.
pub fn public_cache_control() -> String {
    format!(
        "public, max-age={PUBLIC_CACHE_MAX_AGE}, stale-while-revalidate={PUBLIC_CACHE_STALE_WHILE_REVALIDATE}"
    )
}

/// Whether a request is cacheable under v1's rules
/// (`src/index.ts` `fetch()`): a GET, not under `/api/` or `/admin`,
/// no `?wl_preview=`, and no session cookie (anonymous only).
pub fn is_cacheable_public_request(
    method: &str,
    path: &str,
    has_preview_param: bool,
    has_session_cookie: bool,
) -> bool {
    method == "GET"
        && !path.starts_with("/api/")
        && !path.starts_with("/admin")
        && !has_preview_param
        && !has_session_cookie
}

/// Build the list of URLs to purge when a post is published/changed.
/// Port of v1 `urlsToPurgeForPost`: the post's own URL (unless it is
/// the home page itself), the home page, and `/category/<slug>` +
/// `/tag/<slug>` for each of the post's term slugs.
pub fn urls_to_purge_for_post(
    base_url: &str,
    post_slug: &str,
    term_slugs: &[&str],
    is_home_page: bool,
) -> Vec<String> {
    let base = base_url.trim_end_matches('/');
    let mut urls: Vec<String> = Vec::new();
    let mut push = |u: String| {
        if !urls.contains(&u) {
            urls.push(u);
        }
    };
    if !is_home_page {
        push(format!("{base}/{post_slug}"));
    }
    push(format!("{base}/"));
    for slug in term_slugs {
        push(format!("{base}/category/{slug}"));
        push(format!("{base}/tag/{slug}"));
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_control_and_purge_list_match_v1() {
        assert_eq!(
            public_cache_control(),
            "public, max-age=60, stale-while-revalidate=60"
        );
        assert!(is_cacheable_public_request("GET", "/", false, false));
        assert!(!is_cacheable_public_request("GET", "/", true, false)); // preview
        assert!(!is_cacheable_public_request("GET", "/", false, true)); // logged in
        assert!(!is_cacheable_public_request(
            "GET",
            "/api/posts",
            false,
            false
        ));
        assert!(!is_cacheable_public_request("POST", "/", false, false));
        assert_eq!(
            urls_to_purge_for_post("http://localhost:8787", "hello", &["news"], false),
            vec![
                "http://localhost:8787/hello",
                "http://localhost:8787/",
                "http://localhost:8787/category/news",
                "http://localhost:8787/tag/news",
            ]
        );
        assert_eq!(
            urls_to_purge_for_post("http://localhost:8787/", "hello", &[], true),
            vec!["http://localhost:8787/"]
        );
    }
}
