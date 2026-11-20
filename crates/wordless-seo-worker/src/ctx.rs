//! The SEO Worker's request context: everything an SEO route
//! handler needs, in platform-neutral form. Mirrors the main Worker's
//! `Ctx` (same fields the moved SEO handlers use: session user,
//! shared D1, request origin) — nothing else, so this Worker stays
//! lean.
//!
//! `origin` is the MAIN Worker's origin, forwarded by the proxy as
//! the `X-Wordless-Origin` header (see `proxy.rs`): the redirect
//! create/update handlers build cache-purge URLs from it, and the
//! cached entries live under the main Worker's origin, not this
//! Worker's internal address. Falls back to this request's own URL
//! origin when the header is absent (direct calls in local dev).

use std::collections::HashMap;
use std::rc::Rc;

use wordless_db::Db;
use wordless_kernel::capabilities::AuthUser;
use wordless_kernel::router::RequestContext;

pub struct Ctx {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    /// Header names lowercased, first value wins.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub params: HashMap<String, String>,
    pub user: Option<AuthUser>,
    /// `Rc`, not `Arc`: D1 futures are `!Send` (see the `Db` trait),
    /// and the Workers runtime is single-threaded.
    pub db: Rc<dyn Db>,
    /// Request origin (`scheme://host[:port]`) — the main Worker's
    /// origin when proxied, else this request's own origin.
    pub origin: String,
    /// P12: the site this request is for, from the `X-Wordless-Site`
    /// header the main Worker forwards (validated against wl_sites;
    /// 1 when absent or unknown). Sitemaps, robots.txt and llms.txt
    /// are scoped to this site, so one site's sitemap never lists
    /// another site's posts.
    pub site_id: i64,
}

impl Ctx {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(|s| s.as_str())
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query.get(name).map(|s| s.as_str())
    }
}

impl RequestContext for Ctx {
    fn method(&self) -> &str {
        &self.method
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn user(&self) -> Option<&AuthUser> {
        self.user.as_ref()
    }

    fn set_param(&mut self, name: String, value: String) {
        self.params.insert(name, value);
    }

    fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(|s| s.as_str())
    }

    fn body(&self) -> &[u8] {
        &self.body
    }

    fn query(&self, name: &str) -> Option<&str> {
        self.query(name)
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.header(name)
    }
}
