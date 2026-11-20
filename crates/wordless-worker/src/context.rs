//! The worker entry's request context: everything a route handler
//! needs, in platform-neutral form. The worker entry builds one per
//! request from the `worker::Request` / `Env`.

use std::collections::HashMap;
use std::rc::Rc;

use wordless_db::Db;
use wordless_kernel::capabilities::AuthUser;
use wordless_kernel::router::RequestContext;
use wordless_kernel::storage::ObjectStore;
use wordless_kernel::woo_client::WooClient;

use crate::proxy::ProxyTarget;

/// Which bindings the entry found in the environment. The site-health
/// check reports these honestly.
#[derive(Debug, Clone, Copy)]
pub struct Bindings {
    pub db: bool,
    pub media: bool,
    pub config: bool,
}

/// Resolves a loaded extension instance to an executor plus its
/// per-load host token, or `None` when the instance cannot be reached.
/// Built by the worker entry from the Env (dispatch namespace in
/// production, per-instance `dev_url` in local dev) and the host
/// secret; `None` in native tests and when no transport is configured.
pub type ExtResolver = Rc<
    dyn Fn(
        &wordless_extensions::ExtensionInstance,
    ) -> Option<(Rc<dyn wordless_extensions::ExtensionExecutor>, String)>,
>;

pub struct Ctx {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    /// Raw query string (no leading `?`), for the sub-worker proxy —
    /// the proxy forwards it verbatim so the sub-worker sees the
    /// exact query the caller sent.
    pub query_string: String,
    /// Header names lowercased, first value wins.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub params: HashMap<String, String>,
    pub user: Option<AuthUser>,
    /// `Rc`, not `Arc`: D1 futures are `!Send` (see the `Db` trait),
    /// and the Workers runtime is single-threaded.
    pub db: Rc<dyn Db>,
    pub media: Option<Rc<dyn ObjectStore>>,
    pub bindings: Bindings,
    /// P12: the resolved site for this request (1 = the main site).
    /// Every site-scoped query filters on this id; handlers pass
    /// `ctx.site_id` — never a hardcoded 1.
    pub site_id: i64,
    /// P12: the resolved site slug ("" for the main site). Used by the
    /// admin switcher display and by public URL building.
    pub site_slug: String,
    /// P12: true when the resolved site is archived. Public rendering
    /// refuses archived sites (enforced in the worker entry); the API
    /// can still administer them.
    pub site_archived: bool,
    /// Request origin (`scheme://host[:port]`, v1 `new URL(url).origin`).
    /// Used to build absolute cache-purge URLs.
    pub origin: String,
    /// Absolute URLs to delete from the Cache API after this request
    /// (post create/update purges, redirect creates). Handlers push;
    /// the worker entry purges best-effort after routing.
    pub purge_urls: Vec<String>,
    /// P3: the AI environment (Workers AI backend when the `[ai]`
    /// binding is present, `OPENAI_API_KEY` when set; empty when
    /// neither is configured — providers report honestly).
    pub ai_env: wordless_ai::AiEnv,
    /// P3: payment secrets from `wrangler secret` (`PAYSTACK_*`,
    /// `FLUTTERWAVE_*`, `STRIPE_*`, `MOCK_PAYMENT_WEBHOOK_SECRET`).
    /// Never logged, never stored in D1.
    pub payment_secrets: wordless_commerce::payments::PaymentSecrets,
    /// P3: integration secrets (`EMAIL_API_KEY` + payment keys for
    /// the integrations registry's configured detection).
    pub int_secrets: wordless_integrations::EnvSecrets,
    /// P3: which bindings exist (for the integrations cloudflare
    /// status: D1/R2/KV/ASSETS/AI presence, never invented).
    pub int_bindings: wordless_integrations::Bindings,
    /// P3: outbound webhook delivery (`worker::Fetch` POST).
    pub http_post: Rc<dyn wordless_webhooks::HttpPost>,
    /// P6: the extension host secret (`WORDLESS_EXT_HOST_SECRET`),
    /// for deriving per-load host tokens. `None` → extension loading
    /// refuses (fail closed). Never logged, never stored in D1.
    pub ext_secret: Option<String>,
    /// P6: true when the `EXT_DISPATCHER` dispatch-namespace binding
    /// is present (production). Local dev uses per-instance `dev_url`.
    pub ext_dispatcher_present: bool,
    /// P6: resolves a loaded extension to an executor + host token.
    /// `None` in native tests and when no transport is configured.
    pub ext_resolver: Option<ExtResolver>,
    /// Split (2026-11-20): the converter Worker client (service
    /// binding `CONVERTER` in production, `WORDLESS_CONVERTER_URL`
    /// in local dev). `None` in native tests and when no transport is
    /// configured — conversion jobs then fail with a clear error,
    /// and public Elementor rendering falls back to block content.
    pub converter: Option<Rc<dyn wordless_kernel::converter::ConverterClient>>,
    /// Size remedy (2026-11-20): the social Worker proxy target
    /// (service binding `SOCIAL` in production,
    /// `WORDLESS_SOCIAL_URL` in local dev). `None` in native tests
    /// and when no transport is configured — proxied social routes
    /// then answer 503 with a clear error.
    pub social: Option<ProxyTarget>,
    /// Size remedy (2026-11-20): the WooCommerce Worker proxy target
    /// (service binding `WOO` in production, `WORDLESS_WOO_URL` in
    /// local dev). `None` in native tests and when no transport is
    /// configured — proxied Woo routes then answer 503 with a clear
    /// error.
    pub woo: Option<ProxyTarget>,
    /// Size remedy (2026-11-20): the school Worker proxy target
    /// (service binding `SCHOOL` in production,
    /// `WORDLESS_SCHOOL_URL` in local dev). `None` in native tests
    /// and when no transport is configured — proxied school routes
    /// then answer 503 with a clear error.
    pub school: Option<ProxyTarget>,
    /// SEO split (2026-11-20): the SEO Worker proxy target
    /// (service binding `SEO` in production, `WORDLESS_SEO_URL`
    /// in local dev). `None` in native tests and when no transport
    /// is configured — proxied SEO routes then answer 503 with a
    /// clear error.
    pub seo: Option<ProxyTarget>,
    /// Size remedy (2026-11-20): the WooCommerce Worker client for
    /// the storefront (`/shop/*` routes + render). `None` in native
    /// tests and when no transport is configured — shop handlers that
    /// need Woo data then fail with a clear error.
    pub woo_client: Option<Rc<dyn WooClient>>,
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
