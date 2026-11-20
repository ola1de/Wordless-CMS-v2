//! The social Worker's request context: everything a social route
//! handler needs, in platform-neutral form. Mirrors the main Worker's
//! `Ctx` (same fields the P7 handlers use: session user, shared D1,
//! integration secrets for the best-effort notification mail) —
//! nothing else, so this Worker stays lean.

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
    /// Integration secrets (`EMAIL_API_KEY` + payment keys for the
    /// integrations registry's configured detection). Only the mail
    /// key is used here (best-effort notification mail).
    pub int_secrets: wordless_integrations::EnvSecrets,
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
