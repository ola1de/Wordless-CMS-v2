//! WORDLESS v2 integrations (P3).
//!
//! Port of the v1 module under `src/modules/integrations/` in the
//! v1.1.0 TypeScript repo (read-only reference). Same 8 integrations,
//! same config rules, same mail-log table, same marketplace ZIP rules
//! (10,000 files / 50 MB, `wordless.json` manifest, R2 under
//! `extensions/<slug>/`), same compatibility-report storage
//! (`wl_extensions.report_json`).
//!
//! # Standing rules (from v1, unchanged)
//!
//! * **Secrets are never stored in D1 or code.** Any config field
//!   whose name looks like a secret (`secret`, `apiKey`, `token`,
//!   `password`, `private`, `passwd`, …) is rejected with a 400 that
//!   tells the admin to use `wrangler secret put`. See [`config`].
//! * **The mock email provider never claims delivery.** With no
//!   provider configured, a message is recorded in `wl_mail_log`
//!   with status `logged` and `delivered: false`. See [`email`].
//! * **Installed code is NEVER executed.** Marketplace install only
//!   stores files in R2 and records a row in `wl_extensions`.
//!   Activation only records the status. Running an extension needs
//!   the P6 sandboxed runtime; a compatibility report is a static
//!   map, not a promise. See [`marketplace`] and [`compat`].
//! * **No `wordless-conversion` dependency.** The PHP analyser lives
//!   in the parallel `wordless-conversion` crate. The worker wiring
//!   calls the analyser and passes its report JSON to
//!   [`compat::store_compat_report`] (or to
//!   [`marketplace::install_extension_from_zip`] as the `report`
//!   argument). This crate never analyses PHP itself.
//!
//! # Modules
//!
//! * [`config`] — per-integration non-secret config in `wp_options`
//!   (`wl_integration_<id>`), with secret rejection.
//! * [`registry`] — the 8 integration definitions + live
//!   configured/not-configured detection.
//! * [`email`] — mock + generic HTTP email provider, `wl_mail_log`.
//! * [`analytics`] — GA4 / Plausible status + the exact snippet.
//! * [`social`] — plain share links (X, Facebook, WhatsApp,
//!   Telegram). No API, no share counts.
//! * [`cloudflare`] — read-only Worker binding status.
//! * [`extensions`] — `wl_extensions` rows (install / get / list /
//!   activate / deactivate), ported from v1
//!   `src/modules/extensions/service.ts`.
//! * [`marketplace`] — ZIP install / list / report / status.
//! * [`compat`] — compatibility-report storage + summary. Exposes
//!   `store_compat_report`, `get_compat_report`, `summarize_report`
//!   with the signatures the worker wiring uses.
//! * [`platform`] — the `HttpClient` trait + the `worker::Fetch`
//!   implementation. The only place that touches the JS environment.
//! * [`env`] — plain input structs for environment secrets and
//!   binding presence (no Worker types, so everything stays
//!   testable on native targets).
//! * [`util`] — small pure helpers (site URL, host extraction, HTML
//!   escaping, percent-encoding, HTTP(S) URL validation).
//!
//! Service functions take `db: &dyn Db` (and `store: &dyn
//! ObjectStore` where R2 is involved) plus plain parameters and
//! return `Result<_, WordlessError>`, like the P2 crates. The one
//! exception is [`compat::get_compat_report`], whose signature is
//! fixed by the task as `-> Option<serde_json::Value>`: a missing
//! extension, a missing/invalid report, or a database error all
//! surface as `None` — callers that need the distinction use
//! [`extensions::get_extension`].
//!
//! Route handlers live in `wordless-worker` and are wired by the P3
//! coordinator; this crate owns no routes.

pub mod analytics;
pub mod cloudflare;
pub mod compat;
pub mod config;
pub mod email;
pub mod env;
pub mod extensions;
pub mod marketplace;
pub mod platform;
pub mod registry;
pub mod social;
pub mod util;

#[cfg(test)]
mod test_support;

pub use analytics::{AnalyticsProvider, AnalyticsStatus};
pub use cloudflare::BindingStatus;
pub use compat::ReportSummary;
pub use config::{get_integration_config, set_integration_config};
pub use email::{list_mail_log, send_mail, MailLogEntry, SendMailInput, SendMailResult};
pub use env::{Bindings, EnvSecrets};
pub use extensions::{
    get_extension, install_extension, list_extensions, list_plugin_summaries, ExtensionRecord,
    PluginSummary,
};
pub use marketplace::{
    install_extension_from_zip, MarketplaceInstallResult, MarketplaceManifest,
    MAX_MARKETPLACE_FILES, MAX_MARKETPLACE_ZIP_BYTES,
};
pub use platform::{HttpClient, HttpResponse, WorkerHttpClient};
pub use registry::{list_integrations, IntegrationStatus};
pub use social::ShareLinks;
