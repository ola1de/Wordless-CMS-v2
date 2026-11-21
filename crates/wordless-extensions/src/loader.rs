//! Load gate — decides whether a converted extension may run, and
//! says exactly why when it may not.
//!
//! The gate reads two truths: the compatibility report
//! (`wl_extensions.report_json`, static analysis — never a guess) and
//! the converted code itself (scanned for hostile constructs and for
//! the `wp.*` calls it needs). It fails closed:
//!
//! * any file with verdict `blocked` → refuse, with per-file reasons;
//! * no converted code at all → refuse;
//! * `eval(`, `new Function(`, dynamic `import(`, `WebAssembly` in the
//!   converted code → refuse (hostile — the analyser should already
//!   have blocked the file, so this is the second layer);
//! * a direct `fetch(` with no declared hosts → refuse;
//! * a call to a `wp.*` method the P6 host API does not implement (or
//!   does not know) → refuse, naming the method.
//!
//! Partial files are loadable with a warning: their unconverted parts
//! throw `__unsupported` at runtime and the throw is logged per run.
//! The required capabilities are derived from the code (the report's
//! `wpCalls` plus a code scan — both must agree; the scan wins on
//! disagreement because it sees the final code).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capabilities::{required_capabilities_for_code, Capability};

/// One converted file offered to the loader.
#[derive(Debug, Clone)]
pub struct ConvertedFile {
    /// Source PHP path, e.g. `hello.php`.
    pub path: String,
    /// R2 key of the generated TypeScript, e.g.
    /// `extensions/hello-dolly/converted/hello.ts`.
    pub generated_path: String,
    /// Converted TypeScript source (as stored in R2).
    pub code: String,
}

/// One file the gate refused, with the exact reasons (shown in the UI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedFile {
    pub path: String,
    pub reasons: Vec<String>,
}

/// The gate's verdict.
#[derive(Debug, Clone)]
pub enum LoadDecision {
    /// May run. `required` are the capabilities the admin must grant;
    /// `warnings` are non-fatal notes (partial files, unmapped calls
    /// that throw at runtime).
    Allowed {
        required: Vec<Capability>,
        warnings: Vec<String>,
    },
    /// May not run. `reasons` is the complete list for the UI;
    /// `blocked_files` names the files and their reasons.
    Refused {
        reasons: Vec<String>,
        blocked_files: Vec<BlockedFile>,
    },
}

/// Evaluate the load gate for one extension.
///
/// * `report_json` — the parsed `wl_extensions.report_json` (analyser
///   shape; the auto-convert shape is also accepted). `None` means the
///   extension was never analysed → refuse.
/// * `files` — the converted files with their code (from R2).
/// * `declared_hosts` — network hosts declared in the extension
///   manifest (empty by default).
pub fn evaluate_load_gate(
    report_json: Option<&Value>,
    files: &[ConvertedFile],
    declared_hosts: &[String],
) -> LoadDecision {
    let mut reasons: Vec<String> = Vec::new();
    let mut blocked_files: Vec<BlockedFile> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let Some(report) = report_json else {
        return LoadDecision::Refused {
            reasons: vec![
                "This extension was never analysed — run the compatibility analysis before loading it."
                    .to_string(),
            ],
            blocked_files: vec![],
        };
    };

    // 1. Blocked files in the report refuse the load, with reasons.
    for entry in report_files(report) {
        if entry.verdict == "blocked" {
            blocked_files.push(BlockedFile {
                path: entry.path.clone(),
                reasons: entry.reasons.clone(),
            });
        } else if entry.verdict == "partial" {
            warnings.push(format!(
                "{} is only partially converted — its unconverted parts will throw if called (see the conversion report)",
                entry.path
            ));
        }
    }
    if !blocked_files.is_empty() {
        let mut all: Vec<String> = blocked_files
            .iter()
            .flat_map(|f| f.reasons.iter().map(move |r| format!("{}: {r}", f.path)))
            .collect();
        all.insert(
            0,
            format!(
                "{} file{} cannot run — blocked by the conversion sandbox",
                blocked_files.len(),
                if blocked_files.len() == 1 { "" } else { "s" }
            ),
        );
        reasons.append(&mut all);
        return LoadDecision::Refused {
            reasons,
            blocked_files,
        };
    }

    // 2. There must be converted code to run.
    if files.is_empty() {
        return LoadDecision::Refused {
            reasons: vec![
                "No converted code was stored for this extension — nothing to run.".to_string(),
            ],
            blocked_files: vec![],
        };
    }

    // 3. Scan the final code: hostile constructs, network use, and the
    //    capabilities the code needs. The scan sees the code as it will
    //    run, so it wins over the report on disagreement.
    let mut required: Vec<Capability> = Vec::new();
    for file in files {
        let (mut file_caps, mut blockers) = required_capabilities_for_code(&file.code);
        for b in blockers.drain(..) {
            reasons.push(format!("{}: {b}", file.path));
        }
        for cap in file_caps.drain(..) {
            if !required.contains(&cap) {
                required.push(cap);
            }
        }
        for hostile in scan_hostile(&file.code, declared_hosts) {
            reasons.push(format!("{}: {hostile}", file.path));
        }
        // The report's wpCalls are a second opinion: if the report
        // lists a call the scan did not see, trust the report (the
        // code may call it through a variable the scanner missed).
        if let Some(report_caps) = report_capabilities_for(report, &file.path) {
            for cap in report_caps {
                if !required.contains(&cap) {
                    required.push(cap);
                }
            }
        }
    }
    if !reasons.is_empty() {
        return LoadDecision::Refused {
            reasons,
            blocked_files: vec![],
        };
    }

    // Hooks + log are granted on every load (see Capability).
    for cap in [Capability::HooksRegister, Capability::Log] {
        if !required.contains(&cap) {
            required.push(cap);
        }
    }
    LoadDecision::Allowed { required, warnings }
}

/// One per-file entry from a report, tolerating both report shapes
/// (analyser `verdict`/`file`, auto-convert `status`/`file`).
struct ReportFileEntry {
    path: String,
    verdict: String,
    reasons: Vec<String>,
}

fn report_files(report: &Value) -> Vec<ReportFileEntry> {
    let mut out = Vec::new();
    let Some(files) = report.get("files").and_then(Value::as_array) else {
        return out;
    };
    for f in files {
        let path = f
            .get("file")
            .or_else(|| f.get("path"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if path.is_empty() {
            continue;
        }
        // Analyser verdict: convertible/partial/blocked. Auto-convert
        // status: converted/partial/blocked/skipped.
        let verdict = f
            .get("verdict")
            .or_else(|| f.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let verdict = match verdict.as_str() {
            "convertible" | "converted" => "converted".to_string(),
            "partial" => "partial".to_string(),
            "blocked" => "blocked".to_string(),
            "skipped" => "skipped".to_string(),
            other => other.to_string(),
        };
        let mut reasons: Vec<String> = Vec::new();
        if let Some(rs) = f.get("reasons").and_then(Value::as_array) {
            for r in rs {
                if let Some(s) = r.as_str() {
                    reasons.push(s.to_string());
                }
            }
        }
        if let Some(r) = f.get("reason").and_then(Value::as_str) {
            reasons.push(r.to_string());
        }
        if reasons.is_empty() && verdict == "blocked" {
            reasons.push("blocked by the conversion sandbox".to_string());
        }
        out.push(ReportFileEntry {
            path,
            verdict,
            reasons,
        });
    }
    out
}

/// Capabilities implied by a report entry's `wpCalls` list (analyser
/// shape). Unknown calls are ignored here — the code scan is the
/// authority for blockers.
fn report_capabilities_for(report: &Value, path: &str) -> Option<Vec<Capability>> {
    let files = report.get("files")?.as_array()?;
    for f in files {
        let fpath = f.get("file").or_else(|| f.get("path"))?.as_str()?;
        if fpath != path {
            continue;
        }
        let mut caps = Vec::new();
        if let Some(calls) = f.get("wpCalls").and_then(Value::as_array) {
            for c in calls {
                if let Some(name) = c.as_str() {
                    if let Some((cap, _)) = crate::capabilities::capability_for_wp_call(name) {
                        if !caps.contains(&cap) {
                            caps.push(cap);
                        }
                    }
                }
            }
        }
        return Some(caps);
    }
    None
}

/// Hostile constructs in converted code. The scan is string/comment
/// aware (a mention inside a lyric or a comment is not a call).
/// Returns human-readable blocker reasons.
fn scan_hostile(code: &str, declared_hosts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let clean = strip_strings_and_comments(code);
    if contains_call(&clean, "eval") {
        out.push("eval() is blocked by the conversion sandbox — this code cannot run".to_string());
    }
    if contains_call(&clean, "Function") {
        out.push(
            "The Function constructor is blocked by the conversion sandbox — this code cannot run"
                .to_string(),
        );
    }
    if clean.contains("import(") {
        out.push(
            "dynamic import() is blocked by the conversion sandbox — this code cannot run"
                .to_string(),
        );
    }
    if clean.contains("WebAssembly") {
        out.push(
            "WebAssembly is blocked by the conversion sandbox — this code cannot run".to_string(),
        );
    }
    if contains_call(&clean, "fetch") {
        if declared_hosts.is_empty() {
            out.push(
                "fetch() needs declared network hosts, and this extension declares none — declare the hosts in the manifest or remove the call"
                    .to_string(),
            );
        } else {
            out.push(format!(
                "fetch() is used — only these declared hosts are reachable: {}",
                declared_hosts.join(", ")
            ));
        }
    }
    out
}

/// `name(` as a call (identifier boundary before, `(` after, allowing
/// whitespace). Operates on code with strings/comments blanked.
fn contains_call(clean: &str, name: &str) -> bool {
    let bytes = clean.as_bytes();
    let needle = name.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let before_ok = i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric()
                    || bytes[i - 1] == b'_'
                    || bytes[i - 1] == b'$'
                    || bytes[i - 1] == b'.');
            let mut j = i + needle.len();
            while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                j += 1;
            }
            if before_ok && j < bytes.len() && bytes[j] == b'(' {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Blank out string literals, template literals and comments (replaced
/// by spaces, keeping newlines) so scanners never match inside them.
fn strip_strings_and_comments(code: &str) -> String {
    let bytes = code.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    #[derive(PartialEq)]
    enum S {
        Code,
        Sq,
        Dq,
        Tpl,
        Line,
        Block,
    }
    let mut state = S::Code;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            S::Code => {
                if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    state = S::Line;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                } else if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    state = S::Block;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                } else if b == b'\'' {
                    state = S::Sq;
                    out.push(b' ');
                    i += 1;
                } else if b == b'"' {
                    state = S::Dq;
                    out.push(b' ');
                    i += 1;
                } else if b == b'`' {
                    state = S::Tpl;
                    out.push(b' ');
                    i += 1;
                } else {
                    out.push(b);
                    i += 1;
                }
            }
            S::Line => {
                if b == b'\n' {
                    state = S::Code;
                    out.push(b);
                } else {
                    out.push(b' ');
                }
                i += 1;
            }
            S::Block => {
                if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    state = S::Code;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                } else {
                    out.push(if b == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
            }
            S::Sq | S::Dq => {
                let q = if state == S::Sq { b'\'' } else { b'"' };
                if b == b'\\' {
                    out.push(b' ');
                    if i + 1 < bytes.len() {
                        out.push(b' ');
                    }
                    i += 2;
                } else if b == q {
                    state = S::Code;
                    out.push(b' ');
                    i += 1;
                } else {
                    out.push(if b == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
            }
            S::Tpl => {
                if b == b'\\' {
                    out.push(b' ');
                    if i + 1 < bytes.len() {
                        out.push(b' ');
                    }
                    i += 2;
                } else if b == b'`' {
                    state = S::Code;
                    out.push(b' ');
                    i += 1;
                } else {
                    out.push(if b == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hello_dolly_file() -> ConvertedFile {
        ConvertedFile {
            path: "hello.php".to_string(),
            generated_path: "extensions/hello-dolly/converted/hello.ts".to_string(),
            code: std::fs::read_to_string(
                format!("{}/../../tests/fixtures/wordless/hello-dolly-index.ts", env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap_or_else(|_| {
                "export function activate(wp: WordPressApi): void {\n  wp.add_action(\"admin_notices\", () => {});\n}\n".to_string()
            }),
        }
    }

    fn report_for(files: Value) -> Value {
        json!({ "verdict": "compatible", "phpFiles": 1, "files": files })
    }

    #[test]
    fn hello_dolly_loads() {
        let report = report_for(json!([{
            "file": "hello.php", "verdict": "convertible", "reasons": [],
            "wpCalls": ["add_action"], "httpCalls": [], "optionKeys": []
        }]));
        let decision = evaluate_load_gate(Some(&report), &[hello_dolly_file()], &[]);
        match decision {
            LoadDecision::Allowed { required, warnings } => {
                assert!(required.contains(&Capability::HooksRegister));
                assert!(warnings.is_empty());
            }
            LoadDecision::Refused { reasons, .. } => panic!("refused: {reasons:?}"),
        }
    }

    #[test]
    fn blocked_file_refuses_with_reasons() {
        let report = report_for(json!([
            { "file": "hello.php", "verdict": "convertible", "reasons": [], "wpCalls": [], "httpCalls": [] },
            { "file": "evil.php", "verdict": "blocked", "reasons": ["eval() is blocked by the conversion sandbox."], "wpCalls": [], "httpCalls": [] }
        ]));
        let decision = evaluate_load_gate(Some(&report), &[hello_dolly_file()], &[]);
        match decision {
            LoadDecision::Refused {
                reasons,
                blocked_files,
            } => {
                assert_eq!(blocked_files.len(), 1);
                assert_eq!(blocked_files[0].path, "evil.php");
                assert!(reasons.iter().any(|r| r.contains("evil.php")));
            }
            LoadDecision::Allowed { .. } => panic!("should refuse"),
        }
    }

    #[test]
    fn hostile_code_refuses_even_without_report_block() {
        let report = report_for(json!([{
            "file": "x.php", "verdict": "convertible", "reasons": [], "wpCalls": [], "httpCalls": []
        }]));
        let evil = ConvertedFile {
            path: "x.php".to_string(),
            generated_path: "extensions/x/converted/x.ts".to_string(),
            code: "export function activate(wp: WordPressApi): void {\n  const c = eval(\"1+1\");\n  void c;\n}\n".to_string(),
        };
        let decision = evaluate_load_gate(Some(&report), &[evil], &[]);
        match decision {
            LoadDecision::Refused { reasons, .. } => {
                assert!(reasons.iter().any(|r| r.contains("eval")));
            }
            LoadDecision::Allowed { .. } => panic!("eval must refuse"),
        }
    }

    #[test]
    fn fetch_without_declared_hosts_refuses_but_mentions_do_not() {
        let report = report_for(json!([{
            "file": "x.php", "verdict": "convertible", "reasons": [], "wpCalls": [], "httpCalls": []
        }]));
        let mentioning = ConvertedFile {
            path: "x.php".to_string(),
            generated_path: "extensions/x/converted/x.ts".to_string(),
            code: "const s = \"fetch the data\"; // fetch( is mentioned here\n".to_string(),
        };
        assert!(matches!(
            evaluate_load_gate(Some(&report), &[mentioning], &[]),
            LoadDecision::Allowed { .. }
        ));
        let fetching = ConvertedFile {
            path: "x.php".to_string(),
            generated_path: "extensions/x/converted/x.ts".to_string(),
            code: "await fetch(\"https://evil.example/x\");\n".to_string(),
        };
        match evaluate_load_gate(Some(&report), &[fetching], &[]) {
            LoadDecision::Refused { reasons, .. } => {
                assert!(reasons.iter().any(|r| r.contains("fetch")));
            }
            LoadDecision::Allowed { .. } => panic!("fetch must refuse"),
        }
    }

    #[test]
    fn no_report_refuses() {
        match evaluate_load_gate(None, &[hello_dolly_file()], &[]) {
            LoadDecision::Refused { reasons, .. } => {
                assert!(reasons[0].contains("never analysed"));
            }
            LoadDecision::Allowed { .. } => panic!("no report must refuse"),
        }
    }
}
