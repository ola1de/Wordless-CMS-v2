//! WORDLESS v2 P6 — sandboxed extension runtime.
//!
//! Converted TypeScript extensions run as their own Workers, never inside
//! the main Worker's isolate. The sandbox mechanism (doc §3 P6, §7 Q10,
//! decided 2026-11-19 by measurement) is:
//!
//! * **Production:** Workers for Platforms dispatch namespaces — the main
//!   Worker resolves an extension by slug through
//!   `env.dynamic_dispatcher("EXT_DISPATCHER").get(slug)` and fetches it.
//!   Not testable in local `wrangler dev` (wrangler 4.165.0: "needs to be
//!   run remotely"); the deployed-dispatch proof on staging is still open.
//! * **Local dev / test:** service bindings — measured working locally
//!   (`env.EXT (hello-ext) Worker local [connected]`, 200 in 72 ms).
//!   Same fetch-based protocol, behind one [`ExtensionExecutor`] trait.
//! * **QuickJS-in-WASM: rejected** — Boa 0.22 compiles to a 9,149,717-byte
//!   engine and panics at runtime on wasm32-unknown-unknown
//!   (`std::time::Instant::now` in `ContextBuilder::build`).
//!
//! The capability-checked host API in this crate is decide-now #4 (§9.4):
//! an extension gets only declared capabilities, nothing else. The load
//! gate ([`loader`]) refuses any plugin whose conversion report has
//! blocked files, or whose code calls a host method that does not exist,
//! uses `eval`/`Function`/dynamic `import`, or fetches the network
//! without declared hosts — and says exactly why.
//!
//! The extension Worker script ([`script`]) is generated JS/TS: a
//! capability-checked host shim + the converted module code + a fetch
//! handler for hook invocation. Converted TS is bundled to JS at publish
//! time by esbuild (wrangler); the Worker itself never bundles code.
//! Host calls go back to the main Worker over fetch and are checked per
//! call against the extension's granted permissions ([`host`]).
//!
//! A disabled extension is never dispatched — the kill switch is the
//! registry, checked on every invocation ([`hooks`]).

pub mod capabilities;
pub mod executor;
pub mod hooks;
pub mod host;
pub mod loader;
pub mod registry;
pub mod script;

#[cfg(test)]
pub mod test_support;

pub use capabilities::{capability_for_wp_call, CallKind, Capability, ALL_CAPABILITIES};
#[cfg(target_arch = "wasm32")]
pub use executor::FetcherTransport;
pub use executor::{
    race_with_timeout, DispatchTarget, ExtensionExecutor, ExtensionTransport, HttpExecutor,
    InvokeRequest, InvokeResult, DEFAULT_INVOKE_TIMEOUT_MS,
};
pub use hooks::{invoke_hook, ExtensionHookResult, HookOutcome};
pub use host::{
    derive_host_token, generate_host_token, token_hash, token_matches, HostCall, HostResult,
};
pub use loader::{evaluate_load_gate, BlockedFile, ConvertedFile, LoadDecision};
pub use registry::{
    delete_instance, get_instance, list_instances, list_logs, list_runs, record_instance,
    record_log, record_run, set_instance_permissions, set_instance_status, ExtensionInstance,
    ExtensionLog, ExtensionRun, InstanceStatus,
};
pub use script::{
    converted_project_path, generate_extension_project, sanitize_slug, ExtensionProjectInput,
    GeneratedProject,
};
