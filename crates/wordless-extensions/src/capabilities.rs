//! Capability model — decide-now #4 (§9.4): the capability-checked
//! extension host API.
//!
//! An extension gets only declared capabilities, nothing else. Every
//! `wp.*` method the converter can emit maps to exactly one capability
//! (or to "local" — a pure function that runs inside the extension
//! Worker with no host call). Loading an extension requires the admin
//! to grant every capability the extension's code needs; a missing
//! grant is a load failure that names the capability, never a silent
//! runtime surprise.
//!
//! Capability ids are stable strings — they are what the admin UI
//! shows, what `wl_extension_instances.permissions_json` stores, and
//! what the P6 host endpoint checks per call.

use serde::{Deserialize, Serialize};

/// One declared capability an extension may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Read options (`get_option`). Any option key — options never hold
    /// secrets in Wordless by design (P3 integrations reject them).
    OptionsRead,
    /// Write options (`update_option`, `add_option`, `delete_option`),
    /// scoped to the extension's own `<slug>_*` keys plus the option
    /// names the conversion carried over as defaults.
    OptionsWrite,
    /// Register action/filter callbacks (`add_action`, `add_filter`,
    /// `remove_action`, `remove_filter`, local `do_action` /
    /// `apply_filters`). Granted on every load — registering hooks is
    /// the point of an extension.
    HooksRegister,
    /// Read posts, terms, categories, menus, revisions
    /// (`get_post`, `get_posts`, `wp_get_post_terms`, `get_the_terms`,
    /// `wp_list_categories`, `wp_get_post_revisions`, `wp_nav_menu`).
    PostsRead,
    /// Create posts as drafts only, update/delete drafts, restore a
    /// revision to draft (`wp_insert_post`, `wp_update_post`,
    /// `wp_delete_post`, `wp_restore_post_revision`). Never publishes.
    PostsWriteDraft,
    /// Read media (`get_media_items`, `wp_get_attachment_url`).
    MediaRead,
    /// Record stylesheet/script enqueues (`wp_enqueue_style`,
    /// `wp_enqueue_script`). The enqueue is recorded in the extension
    /// log only — the theme engine serves the actual assets
    /// (theme.json / style.css), so an extension can never inject an
    /// arbitrary stylesheet or script URL into a page through this
    /// call. Added for TT25: its `functions.php` enqueues its style
    /// on `wp_enqueue_scripts`, and without this capability the whole
    /// theme refused to load.
    ThemeEnqueue,
    /// Read user meta for the invoking actor (`get_user_meta`).
    UsersRead,
    /// Write user meta for the invoking actor (`update_user_meta`).
    UsersWrite,
    /// `fetch()` to hosts declared in the extension manifest. Default:
    /// no hosts declared, so any `fetch(` in converted code blocks the
    /// load. The shim enforces this again at runtime.
    NetworkDeclared,
    /// Write to the extension log. Always granted.
    Log,
}

/// All capabilities, in the order the admin UI lists them.
pub const ALL_CAPABILITIES: &[Capability] = &[
    Capability::OptionsRead,
    Capability::OptionsWrite,
    Capability::HooksRegister,
    Capability::PostsRead,
    Capability::PostsWriteDraft,
    Capability::MediaRead,
    Capability::ThemeEnqueue,
    Capability::UsersRead,
    Capability::UsersWrite,
    Capability::NetworkDeclared,
    Capability::Log,
];

impl Capability {
    /// Stable id, as stored in `permissions_json` and shown in the API.
    pub fn id(self) -> &'static str {
        match self {
            Capability::OptionsRead => "options.read",
            Capability::OptionsWrite => "options.write",
            Capability::HooksRegister => "hooks.register",
            Capability::PostsRead => "posts.read",
            Capability::PostsWriteDraft => "posts.write.draft",
            Capability::MediaRead => "media.read",
            Capability::ThemeEnqueue => "theme.enqueue",
            Capability::UsersRead => "users.read",
            Capability::UsersWrite => "users.write",
            Capability::NetworkDeclared => "network.declared",
            Capability::Log => "log",
        }
    }

    /// Plain-language description for the admin UI ("what this extension
    /// can do").
    pub fn description(self) -> &'static str {
        match self {
            Capability::OptionsRead => "Read site settings and options",
            Capability::OptionsWrite => {
                "Change its own settings (keys starting with its slug, plus settings the conversion carried over)"
            }
            Capability::HooksRegister => "Register actions and filters (run code when things happen)",
            Capability::PostsRead => "Read posts, pages, categories, menus and revisions",
            Capability::PostsWriteDraft => {
                "Create and edit drafts (never publish — publishing always needs approval)"
            }
            Capability::MediaRead => "Read media library items and file URLs",
            Capability::ThemeEnqueue => {
                "Record stylesheet and script enqueues (the theme engine serves the actual assets)"
            }
            Capability::UsersRead => "Read user details for the person using the site",
            Capability::UsersWrite => "Change user details for the person using the site",
            Capability::NetworkDeclared => {
                "Contact outside websites listed in its manifest (none by default)"
            }
            Capability::Log => "Write to its own log",
        }
    }

    /// Capabilities granted on every load without an admin decision.
    /// Everything else must be granted explicitly at load time.
    pub fn granted_by_default(self) -> bool {
        matches!(self, Capability::HooksRegister | Capability::Log)
    }

    /// Parse a stored capability id. Unknown ids are rejected — a grant
    /// that does not exist cannot be honoured silently.
    pub fn from_id(id: &str) -> Option<Self> {
        ALL_CAPABILITIES.iter().copied().find(|c| c.id() == id)
    }
}

/// Where a `wp.*` call executes: inside the extension Worker with no
/// host round-trip, or via the host endpoint (async, capability-checked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// Pure/local — implemented by the shim, no host call, no
    /// capability beyond what the call itself implies.
    Local,
    /// Async host call — the shim fetches the main Worker, which checks
    /// the capability before running the real implementation.
    Host,
    /// Not implemented in the P6 host API — calling it is a load-time
    /// blocker with the exact reason (fail closed, never a silent stub).
    NotImplemented,
}

/// Map a `wp.<name>` call to its capability and kind. Returns `None`
/// for names the P6 host API does not know at all (the emitter only
/// produces names from the converter's known set, so an unknown name
/// means the converted code was hand-edited or comes from a newer
/// converter — also a load blocker).
pub fn capability_for_wp_call(name: &str) -> Option<(Capability, CallKind)> {
    use Capability::*;
    let lower = name.to_ascii_lowercase();
    let found: Option<(Capability, CallKind)> = match lower.as_str() {
        // --- local (sync, pure) ---
        "add_action" | "add_filter" | "remove_action" | "remove_filter" | "do_action"
        | "apply_filters" => Some((HooksRegister, CallKind::Local)),
        // --- host: extension log (shim-internal: logDenied) ---
        // Without this entry the endpoint returns not_implemented for
        // "log" and an undeclared-fetch denial is never recorded.
        "log" => Some((Log, CallKind::Host)),
        // --- host: options ---
        "get_option" => Some((OptionsRead, CallKind::Host)),
        "update_option" | "delete_option" | "add_option" => Some((OptionsWrite, CallKind::Host)),
        // --- host: posts / terms / menus / revisions ---
        "get_post"
        | "get_posts"
        | "wp_get_post_terms"
        | "get_the_terms"
        | "wp_list_categories"
        | "wp_get_post_revisions"
        | "wp_nav_menu" => Some((PostsRead, CallKind::Host)),
        "wp_insert_post" | "wp_update_post" | "wp_delete_post" | "wp_restore_post_revision" => {
            Some((PostsWriteDraft, CallKind::Host))
        }
        // --- host: media ---
        "get_media_items" | "wp_get_attachment_url" => Some((MediaRead, CallKind::Host)),
        // --- host: theme asset enqueues (recorded only, see ThemeEnqueue) ---
        "wp_enqueue_style" | "wp_enqueue_script" => Some((ThemeEnqueue, CallKind::Host)),
        // --- host: users ---
        "get_user_meta" => Some((UsersRead, CallKind::Host)),
        "update_user_meta" => Some((UsersWrite, CallKind::Host)),
        // --- host: pure text/locale (local in the shim) ---
        "__" | "_e" | "esc_html" | "esc_attr" | "wptexturize" | "get_user_locale"
        | "get_locale" => None, // handled by `is_local_pure_call`
        // --- not implemented in P6 (explicit, load-blocking) ---
        "media_handle_upload" | "wp_schedule_event" => Some((Log, CallKind::NotImplemented)),
        _ => return None,
    };
    found
}

/// `wp.*` calls that are pure functions implemented inside the shim
/// with no host round-trip and no capability. (`__`, `_e`, `esc_html`,
/// `esc_attr`, `wptexturize`, `get_user_locale`, `get_locale`.)
pub fn is_local_pure_call(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "__" | "_e" | "esc_html" | "esc_attr" | "wptexturize" | "get_user_locale" | "get_locale"
    )
}

/// Scan converted code for `wp.<name>(` calls and return the distinct
/// capabilities the code needs, in first-seen order. Calls to methods
/// the P6 host API does not know, or to methods marked NotImplemented,
/// are returned as blockers (`load refused — exact reason`).
pub fn required_capabilities_for_code(code: &str) -> (Vec<Capability>, Vec<String>) {
    let mut required: Vec<Capability> = Vec::new();
    let mut blockers: Vec<String> = Vec::new();
    let push = |caps: &mut Vec<Capability>, c: Capability| {
        if !caps.contains(&c) {
            caps.push(c);
        }
    };
    for name in wp_calls_in(code) {
        if is_local_pure_call(&name) {
            continue;
        }
        match capability_for_wp_call(&name) {
            Some((cap, CallKind::Local)) | Some((cap, CallKind::Host)) => push(&mut required, cap),
            Some((_, CallKind::NotImplemented)) => blockers.push(format!(
                "wp_{name}() is not available in the P6 host API — this extension cannot run yet"
            )),
            None => blockers.push(format!(
                "wp_{name}() is not a known host method — this extension cannot run yet"
            )),
        }
    }
    (required, blockers)
}

/// Extract the distinct `wp.<name>` call names from code, ignoring
/// string literals and comments (a mention inside a lyric or a comment
/// is not a call). Only `wp.` followed by an identifier and `(` counts.
pub fn wp_calls_in(code: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = code.as_bytes();
    let mut i = 0;
    let mut state = ScanState::Code;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            ScanState::Code => {
                if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    state = ScanState::LineComment;
                    i += 2;
                    continue;
                }
                if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    state = ScanState::BlockComment;
                    i += 2;
                    continue;
                }
                if b == b'\'' {
                    state = ScanState::SingleQuote;
                    i += 1;
                    continue;
                }
                if b == b'"' {
                    state = ScanState::DoubleQuote;
                    i += 1;
                    continue;
                }
                if b == b'`' {
                    state = ScanState::Template;
                    i += 1;
                    continue;
                }
                if b == b'w' && code[i..].starts_with("wp.") {
                    let mut j = i + 3;
                    let start = j;
                    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                    {
                        j += 1;
                    }
                    let name = &code[start..j];
                    // Skip whitespace, then require `(`.
                    let mut k = j;
                    while k < bytes.len() && (bytes[k] as char).is_whitespace() {
                        k += 1;
                    }
                    if !name.is_empty()
                        && k < bytes.len()
                        && bytes[k] == b'('
                        && !out.iter().any(|n| n == name)
                    {
                        out.push(name.to_string());
                    }
                    i = j;
                    continue;
                }
                i += 1;
            }
            ScanState::LineComment => {
                if b == b'\n' {
                    state = ScanState::Code;
                }
                i += 1;
            }
            ScanState::BlockComment => {
                if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    state = ScanState::Code;
                    i += 2;
                    continue;
                }
                i += 1;
            }
            ScanState::SingleQuote | ScanState::DoubleQuote => {
                let quote = if state == ScanState::SingleQuote {
                    b'\''
                } else {
                    b'"'
                };
                if b == b'\\' {
                    i += 2;
                    continue;
                }
                if b == quote {
                    state = ScanState::Code;
                }
                i += 1;
            }
            ScanState::Template => {
                if b == b'\\' {
                    i += 2;
                    continue;
                }
                if b == b'`' {
                    state = ScanState::Code;
                }
                i += 1;
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Code,
    LineComment,
    BlockComment,
    SingleQuote,
    DoubleQuote,
    Template,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_ids_round_trip() {
        for c in ALL_CAPABILITIES {
            assert_eq!(Capability::from_id(c.id()), Some(*c), "id {}", c.id());
        }
        assert_eq!(Capability::from_id("nope"), None);
        assert!(Capability::HooksRegister.granted_by_default());
        assert!(Capability::Log.granted_by_default());
        assert!(!Capability::OptionsRead.granted_by_default());
    }

    #[test]
    fn wp_calls_detected_and_string_mentions_ignored() {
        let code = r#"
            // wp.get_option("x") in a comment is not a call
            const s = "wp.delete_option('y')";
            await wp.get_option("blogname");
            wp.add_action("init", () => {});
            wp.wp_insert_post({ title: "t" });
        "#;
        let calls = wp_calls_in(code);
        assert_eq!(calls, vec!["get_option", "add_action", "wp_insert_post"]);
    }

    #[test]
    fn required_capabilities_for_hello_dolly() {
        // The real converted Hello Dolly: only add_action + pure calls.
        let code = r#"
            export function activate(wp: WordPressApi): void {
              wp.add_action("admin_notices", () => { renderedOutput.push(helloDolly(wp)); });
              wp.add_action("admin_head", () => { renderedOutput.push(dollyCss(wp)); });
            }
            const t = wp.__("Quote");
            const u = wp.get_user_locale();
        "#;
        let (required, blockers) = required_capabilities_for_code(code);
        assert!(blockers.is_empty());
        assert_eq!(required, vec![Capability::HooksRegister]);
    }

    #[test]
    fn unimplemented_host_method_blocks_load() {
        let code = "await wp.wp_schedule_event(123, 'hourly', 'my_hook');";
        let (_, blockers) = required_capabilities_for_code(code);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("wp_schedule_event"));
    }

    #[test]
    fn enqueue_calls_require_theme_enqueue_capability() {
        // TT25's functions.php enqueues its stylesheet on
        // wp_enqueue_scripts — the call must map to a real capability
        // (Host, recorded only) instead of blocking the load.
        assert_eq!(
            capability_for_wp_call("wp_enqueue_style"),
            Some((Capability::ThemeEnqueue, CallKind::Host))
        );
        assert_eq!(
            capability_for_wp_call("wp_enqueue_script"),
            Some((Capability::ThemeEnqueue, CallKind::Host))
        );
        let code =
            "wp.wp_enqueue_style('tt25-style', 'https://example.test/style.css', [], '1.0');";
        let (required, blockers) = required_capabilities_for_code(code);
        assert!(blockers.is_empty(), "{blockers:?}");
        assert_eq!(required, vec![Capability::ThemeEnqueue]);
        // The shim's logDenied path needs "log" routable, or
        // undeclared-fetch denials are never recorded in D1.
        assert_eq!(
            capability_for_wp_call("log"),
            Some((Capability::Log, CallKind::Host))
        );
    }

    #[test]
    fn unknown_wp_call_blocks_load() {
        let code = "wp.some_future_function(1);";
        let (_, blockers) = required_capabilities_for_code(code);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("some_future_function"));
    }
}
