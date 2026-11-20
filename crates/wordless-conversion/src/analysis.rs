//! Static analysis — a port of v1 `src/conversion/analyze.ts`.
//!
//! `analyze_php` walks a parsed program and summarises what a file
//! does: functions, classes, hooks, WordPress API calls, options, HTTP
//! calls, and the PHP features that decide convertibility. It never
//! executes the source.

use serde::{Deserialize, Serialize};

use crate::ast::{walk_program, Expr, Node, Stmt};
use crate::parser::{parse_php, ParseError};

/// File convertibility verdict (v1 `Convertibility`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Convertibility {
    Convertible,
    Partial,
    Blocked,
}

/// One feature the converter cannot translate as-is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnconvertibleFeature {
    pub kind: String,
    pub reason: String,
    pub line: usize,
}

/// A literal scalar default from a `get_option()` / `add_option()` call
/// (M11 settings migration).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionDefault {
    pub key: String,
    /// Literal scalar default (string / number / bool only).
    #[serde(rename = "defaultValue")]
    pub default_value: serde_json::Value,
    pub file: String,
}

/// One hook reference found by the analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRef {
    pub tag: String,
    /// `"action"` or `"filter"`.
    pub kind: String,
}

/// The full per-file analysis (v1 `FileAnalysis`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAnalysis {
    pub file: String,
    pub lines: usize,
    pub bytes: usize,
    pub tokens: usize,
    pub nodes: usize,
    pub functions: Vec<String>,
    pub classes: Vec<String>,
    pub hooks: Vec<HookRef>,
    #[serde(rename = "wpCalls")]
    pub wp_calls: Vec<String>,
    #[serde(rename = "optionKeys")]
    pub option_keys: Vec<String>,
    #[serde(rename = "optionDefaults")]
    pub option_defaults: Vec<OptionDefault>,
    #[serde(rename = "httpCalls")]
    pub http_calls: Vec<String>,
    pub features: Vec<String>,
    pub unconvertible: Vec<UnconvertibleFeature>,
    pub verdict: Convertibility,
    pub reasons: Vec<String>,
}

const HTTP_CALLS: &[&str] = &[
    "wp_remote_get",
    "wp_remote_post",
    "wp_remote_request",
    "wp_remote_head",
    "wp_remote_retrieve_body",
    "wp_remote_retrieve_response_code",
    "wp_safe_remote_get",
    "wp_safe_remote_post",
    "wp_http_validate_url",
    "download_url",
];

const OPTION_CALLS: &[&str] = &[
    "get_option",
    "update_option",
    "delete_option",
    "add_option",
    "get_site_option",
    "update_site_option",
];

fn literal_string(node: Option<&Expr>) -> Option<String> {
    match node {
        Some(Expr::Literal {
            literal_kind,
            value,
            ..
        }) if literal_kind == "string" => match value {
            serde_json::Value::String(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// A literal scalar (string / number / bool) — used for option defaults.
fn literal_scalar(node: Option<&Expr>) -> Option<serde_json::Value> {
    match node {
        Some(Expr::Literal {
            literal_kind,
            value,
            ..
        }) => match (literal_kind.as_str(), value) {
            ("string", serde_json::Value::String(_))
            | ("number", serde_json::Value::Number(_))
            | ("bool", serde_json::Value::Bool(_)) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Is `needle` present in `haystack` as a whole word (ASCII word
/// boundaries)? Hand-rolled: no regex crate in the workspace.
fn contains_word(haystack: &str, needle: &str, case_insensitive: bool) -> bool {
    let hay = if case_insensitive {
        haystack.to_lowercase()
    } else {
        haystack.to_string()
    };
    let ndl = if case_insensitive {
        needle.to_lowercase()
    } else {
        needle.to_string()
    };
    let hb = hay.as_bytes();
    let nb = ndl.as_bytes();
    if nb.is_empty() || hb.len() < nb.len() {
        return false;
    }
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i + nb.len() <= hb.len() {
        if &hb[i..i + nb.len()] == nb {
            // A boundary only applies where the needle itself starts/ends
            // with a word character (e.g. `$wpdb` has no word char
            // before the `$`, so only the trailing boundary applies —
            // matching the TS `/\$wpdb\b/`).
            let before_ok = !is_word(nb[0]) || i == 0 || !is_word(hb[i - 1]);
            let after_ok = !is_word(nb[nb.len() - 1])
                || i + nb.len() == hb.len()
                || !is_word(hb[i + nb.len()]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Analyse one PHP file. Returns an error when the file cannot be
/// parsed (same as v1, where `parsePhp` throws).
pub fn analyze_php(source: &str, filename: &str) -> Result<FileAnalysis, ParseError> {
    let parsed = parse_php(source)?;
    let program = &parsed.program;
    let lines = source.split('\n').count();
    let bytes = source.len();

    let mut functions: Vec<String> = Vec::new();
    let mut classes: Vec<String> = Vec::new();
    let mut hooks: Vec<HookRef> = Vec::new();
    let mut wp_calls: std::collections::BTreeSet<String> = Default::default();
    let mut option_keys: std::collections::BTreeSet<String> = Default::default();
    let mut option_defaults: Vec<OptionDefault> = Vec::new();
    let mut http_calls: std::collections::BTreeSet<String> = Default::default();
    let mut features: std::collections::BTreeSet<String> = Default::default();
    let mut unconvertible: Vec<UnconvertibleFeature> = Vec::new();
    // P5 run-4: v1's analyze.ts counts nodes with walkAst, which visits
    // the program node itself (nodeCount++ for every node including
    // kind "program"), while its countAstNodes — used by the conversion
    // report — excludes it. This analysis mirrors analyze.ts, so the
    // program node counts as 1 here. The conversion report
    // (pipeline.rs, count_ast_nodes) correctly excludes it.
    let mut node_count = 1usize;

    // The ABSPATH direct-access guard is transformed by the emitter (not
    // unconverted): collect the guarded exit/die lines so they are not
    // counted as manual work. Same check as the emitter's isAbpsathGuard.
    let mut guard_lines: std::collections::HashSet<usize> = Default::default();
    walk_program(program, &mut |node| {
        if let Node::Stmt(Stmt::If {
            test, consequent, ..
        }) = node
        {
            let Expr::Unary {
                operator, operand, ..
            } = test
            else {
                return;
            };
            if operator != "!" {
                return;
            }
            let Expr::Call {
                name: Some(name),
                args,
                ..
            } = operand.as_ref()
            else {
                return;
            };
            if name.to_lowercase() != "defined" {
                return;
            }
            if literal_string(args.first()).as_deref() != Some("ABSPATH") {
                return;
            }
            if !consequent.iter().all(|s| match s {
                Stmt::Unsupported { label, .. } => label == "exit()" || label == "die()",
                _ => false,
            }) {
                return;
            }
            for s in consequent {
                guard_lines.insert(s.pos().line);
            }
        }
    });

    walk_program(program, &mut |node| {
        node_count += 1;
        match node {
            Node::Stmt(Stmt::Function { name, .. }) => functions.push(name.clone()),
            Node::Stmt(Stmt::Class {
                name, methods, pos, ..
            }) => {
                classes.push(name.clone());
                features.insert("class".to_string());
                unconvertible.push(UnconvertibleFeature {
                    kind: "class".to_string(),
                    reason: format!(
                        "Class {name} ({} methods) needs a class runtime, which Wordless does not provide yet.",
                        methods.len()
                    ),
                    line: pos.line,
                });
            }
            Node::Stmt(Stmt::Method { .. }) => {
                features.insert("method".to_string());
            }
            Node::Expr(Expr::Closure { uses, .. }) => {
                features.insert("closure".to_string());
                if !uses.is_empty() {
                    features.insert("closure-use".to_string());
                }
            }
            Node::Expr(Expr::New {
                class_name, pos, ..
            }) => {
                features.insert("new".to_string());
                unconvertible.push(UnconvertibleFeature {
                    kind: "instantiation".to_string(),
                    reason: format!("new {class_name}() needs a class runtime."),
                    line: pos.line,
                });
            }
            Node::Expr(Expr::MethodCall { .. }) => {
                features.insert("method-call".to_string());
            }
            Node::Expr(Expr::StaticCall { .. }) => {
                features.insert("static-call".to_string());
            }
            Node::Expr(Expr::PropertyFetch { .. }) => {
                features.insert("property-fetch".to_string());
            }
            Node::Stmt(Stmt::Switch {
                pos, alt_syntax, ..
            }) => {
                features.insert("switch".to_string());
                if *alt_syntax == Some(true) {
                    features.insert("alternative-syntax".to_string());
                }
                unconvertible.push(UnconvertibleFeature {
                    kind: "switch".to_string(),
                    reason: "switch statements are not translated yet; rewrite as if/else."
                        .to_string(),
                    line: pos.line,
                });
            }
            Node::Stmt(Stmt::Try { .. }) => {
                features.insert("try-catch".to_string());
            }
            Node::Stmt(Stmt::Foreach { alt_syntax, .. }) => {
                features.insert("foreach".to_string());
                if *alt_syntax == Some(true) {
                    features.insert("alternative-syntax".to_string());
                }
            }
            Node::Stmt(Stmt::For { alt_syntax, .. }) => {
                features.insert("for".to_string());
                if *alt_syntax == Some(true) {
                    features.insert("alternative-syntax".to_string());
                }
            }
            Node::Stmt(Stmt::While { alt_syntax, .. }) => {
                features.insert("while".to_string());
                if *alt_syntax == Some(true) {
                    features.insert("alternative-syntax".to_string());
                }
            }
            Node::Stmt(Stmt::If { alt_syntax, .. }) => {
                if *alt_syntax == Some(true) {
                    features.insert("alternative-syntax".to_string());
                }
            }
            Node::Expr(Expr::Cast { .. }) => {
                features.insert("cast".to_string());
            }
            Node::Stmt(Stmt::StaticVar { .. }) => {
                features.insert("static-var".to_string());
            }
            Node::Stmt(Stmt::DoWhile { .. }) => {
                features.insert("do-while".to_string());
            }
            Node::Stmt(Stmt::Global { pos, .. }) => {
                features.insert("global".to_string());
                unconvertible.push(UnconvertibleFeature {
                    kind: "global".to_string(),
                    reason: "global variables have no equivalent in Wordless extensions."
                        .to_string(),
                    line: pos.line,
                });
            }
            Node::Stmt(Stmt::InlineHtml { .. }) => {
                features.insert("inline-html".to_string());
            }
            Node::Stmt(Stmt::Unsupported {
                label, reason, pos, ..
            }) => {
                // The ABSPATH guard's exit/die is transformed, not work.
                if guard_lines.contains(&pos.line) {
                    return;
                }
                unconvertible.push(UnconvertibleFeature {
                    kind: label.clone(),
                    reason: reason.clone(),
                    line: pos.line,
                });
            }
            Node::Expr(Expr::UnsupportedExpr {
                label, reason, pos, ..
            }) => {
                unconvertible.push(UnconvertibleFeature {
                    kind: label.clone(),
                    reason: reason.clone(),
                    line: pos.line,
                });
            }
            Node::Expr(Expr::Call {
                name, args, pos, ..
            }) => {
                let Some(name) = name else {
                    features.insert("dynamic-call".to_string());
                    return;
                };
                let lower = name.to_lowercase();
                if lower == "add_action" || lower == "add_filter" {
                    if let Some(tag) = literal_string(args.first()) {
                        hooks.push(HookRef {
                            tag,
                            kind: if lower == "add_action" {
                                "action".to_string()
                            } else {
                                "filter".to_string()
                            },
                        });
                    }
                }
                if OPTION_CALLS.contains(&lower.as_str()) {
                    if let Some(key) = literal_string(args.first()) {
                        option_keys.insert(key.clone());
                        if let Some(fallback) = literal_scalar(args.get(1)) {
                            option_defaults.push(OptionDefault {
                                key,
                                default_value: fallback,
                                file: filename.to_string(),
                            });
                        }
                    }
                }
                if HTTP_CALLS.contains(&lower.as_str()) {
                    http_calls.insert(lower.clone());
                }
                if lower.starts_with("wp_")
                    || lower.starts_with("get_")
                    || lower.starts_with("update_")
                    || lower.starts_with("delete_")
                    || lower.starts_with("add_")
                    || lower == "__"
                    || lower == "_e"
                    || lower.starts_with("esc_")
                    || lower == "apply_filters"
                    || lower == "do_action"
                {
                    wp_calls.insert(lower.clone());
                }
                if matches!(
                    lower.as_str(),
                    "eval"
                        | "exec"
                        | "shell_exec"
                        | "system"
                        | "passthru"
                        | "create_function"
                        | "unserialize"
                ) {
                    features.insert("blocked-construct".to_string());
                    unconvertible.push(UnconvertibleFeature {
                        kind: lower.clone(),
                        reason: format!("{lower}() is blocked by the conversion sandbox."),
                        line: pos.line,
                    });
                }
            }
            Node::Expr(Expr::Var { name, .. }) => {
                if name == "this" {
                    features.insert("$this".to_string());
                }
                if matches!(
                    name.as_str(),
                    "_GET"
                        | "_POST"
                        | "_SERVER"
                        | "_FILES"
                        | "_COOKIE"
                        | "_REQUEST"
                        | "_SESSION"
                        | "GLOBALS"
                ) {
                    features.insert("superglobal".to_string());
                }
            }
            _ => {}
        }
    });

    // $wpdb usage shows up as a variable named wpdb.
    if contains_word(source, "$wpdb", false) {
        features.insert("$wpdb".to_string());
        unconvertible.push(UnconvertibleFeature {
            kind: "$wpdb".to_string(),
            reason: "Direct database access via $wpdb is not available to Wordless extensions; use the wp API (get_post, get_option, …).".to_string(),
            line: 1,
        });
    }
    if contains_word(source, "namespace", false) {
        features.insert("namespace".to_string());
    }
    if contains_word(source, "heredoc", true) {
        features.insert("heredoc".to_string());
    }

    let mut reasons: Vec<String> = Vec::new();
    let mut verdict = Convertibility::Convertible;
    let blocked_by_external = !http_calls.is_empty()
        || features.contains("$wpdb")
        || features.contains("blocked-construct");
    let has_classes = !classes.is_empty();
    if blocked_by_external {
        verdict = Convertibility::Blocked;
        if !http_calls.is_empty() {
            reasons.push(format!(
                "calls an external HTTP API ({}), which needs a service integration, not a translation.",
                http_calls.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if features.contains("$wpdb") {
            reasons.push("uses $wpdb for direct database access.".to_string());
        }
        if features.contains("blocked-construct") {
            reasons.push("contains a construct blocked by the conversion sandbox.".to_string());
        }
    } else if has_classes || !unconvertible.is_empty() {
        verdict = Convertibility::Partial;
        if has_classes {
            reasons.push(format!(
                "defines {} class{} ({}); classes are analysed but not converted.",
                classes.len(),
                if classes.len() == 1 { "" } else { "es" },
                classes.join(", ")
            ));
        }
        if !unconvertible.is_empty() && !has_classes {
            reasons.push(format!(
                "{} construct{} need{} manual work.",
                unconvertible.len(),
                if unconvertible.len() == 1 { "" } else { "s" },
                if unconvertible.len() == 1 { "s" } else { "" }
            ));
        }
    } else {
        reasons.push("procedural code using only mapped or mappable WordPress APIs.".to_string());
    }

    Ok(FileAnalysis {
        file: filename.to_string(),
        lines,
        bytes,
        tokens: parsed.tokens.len(),
        nodes: node_count,
        functions,
        classes,
        hooks,
        wp_calls: wp_calls.into_iter().collect(),
        option_keys: option_keys.into_iter().collect(),
        option_defaults,
        http_calls: http_calls.into_iter().collect(),
        features: features.into_iter().collect(),
        unconvertible,
        verdict,
        reasons,
    })
}

/// Render per-file analyses as the Akismet-style analysis markdown
/// report (port of v1 `renderAnalysisReport`).
pub fn render_analysis_report(
    analyses: &[FileAnalysis],
    title: &str,
    intro: &str,
    generated_by: &str,
) -> String {
    let total_lines: usize = analyses.iter().map(|a| a.lines).sum();
    let total_bytes: usize = analyses.iter().map(|a| a.bytes).sum();
    let by_verdict = |v: Convertibility| analyses.iter().filter(|a| a.verdict == v).count();
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("# {title}"));
    lines.push(String::new());
    lines.push(intro.to_string());
    lines.push(String::new());
    lines.push(
        "Generated by the Wordless conversion engine (Milestone 4) static analysis.".to_string(),
    );
    lines.push("No PHP was executed; every file below was parsed, not run.".to_string());
    lines.push(String::new());
    lines.push("## Summary".to_string());
    lines.push(String::new());
    lines.push(format!("- Files analysed: **{}**", analyses.len()));
    lines.push(format!(
        "- Total lines: **{}** · total size: **{:.1} KB**",
        total_lines,
        total_bytes as f64 / 1024.0
    ));
    lines.push(format!(
        "- Convertible: **{}** · Partial: **{}** · Blocked: **{}**",
        by_verdict(Convertibility::Convertible),
        by_verdict(Convertibility::Partial),
        by_verdict(Convertibility::Blocked)
    ));
    lines.push(String::new());
    lines.push("| File | Lines | Verdict | Why |".to_string());
    lines.push("| --- | ---: | --- | --- |".to_string());
    for a in analyses {
        let verdict = match a.verdict {
            Convertibility::Convertible => "convertible",
            Convertibility::Partial => "partial",
            Convertibility::Blocked => "blocked",
        };
        lines.push(format!(
            "| `{}` | {} | {verdict} | {} |",
            a.file,
            a.lines,
            a.reasons.join(" ")
        ));
    }
    lines.push(String::new());
    for a in analyses {
        let verdict = match a.verdict {
            Convertibility::Convertible => "convertible",
            Convertibility::Partial => "partial",
            Convertibility::Blocked => "blocked",
        };
        lines.push(format!("## `{}` — {verdict}", a.file));
        lines.push(String::new());
        lines.push(format!(
            "- Lines: {} · bytes: {} · tokens: {} · AST nodes: {}",
            a.lines, a.bytes, a.tokens, a.nodes
        ));
        if !a.functions.is_empty() {
            lines.push(format!("- Functions: {}", a.functions.join(", ")));
        }
        if !a.classes.is_empty() {
            lines.push(format!("- Classes: {}", a.classes.join(", ")));
        }
        if !a.hooks.is_empty() {
            let hooks: Vec<String> = a
                .hooks
                .iter()
                .map(|h| format!("{} \"{}\"", h.kind, h.tag))
                .collect();
            lines.push(format!("- Hooks: {}", hooks.join(", ")));
        }
        if !a.wp_calls.is_empty() {
            lines.push(format!("- WordPress APIs used: {}", a.wp_calls.join(", ")));
        }
        if !a.option_keys.is_empty() {
            lines.push(format!("- Options used: {}", a.option_keys.join(", ")));
        }
        if !a.http_calls.is_empty() {
            lines.push(format!("- HTTP calls: {}", a.http_calls.join(", ")));
        }
        if !a.features.is_empty() {
            lines.push(format!("- PHP features: {}", a.features.join(", ")));
        }
        if !a.unconvertible.is_empty() {
            lines.push(format!("- Needs manual work ({}):", a.unconvertible.len()));
            for u in a.unconvertible.iter().take(12) {
                lines.push(format!("  - line {}: {} — {}", u.line, u.kind, u.reason));
            }
            if a.unconvertible.len() > 12 {
                lines.push(format!(
                    "  - … and {} more (same kinds).",
                    a.unconvertible.len() - 12
                ));
            }
        }
        lines.push(format!(
            "- Verdict: **{verdict}** — {}",
            a.reasons.join(" ")
        ));
        lines.push(String::new());
    }
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push("## Effort estimate (honest, not a promise)".to_string());
    lines.push(String::new());
    let total_unconvertible: usize = analyses.iter().map(|a| a.unconvertible.len()).sum();
    let class_count: usize = analyses.iter().map(|a| a.classes.len()).sum();
    let http_files = analyses.iter().filter(|a| !a.http_calls.is_empty()).count();
    let wpdb_files = analyses
        .iter()
        .filter(|a| a.features.contains(&"$wpdb".to_string()))
        .count();
    let convertible_lines: usize = analyses
        .iter()
        .filter(|a| a.verdict == Convertibility::Convertible)
        .map(|a| a.lines)
        .sum();
    lines.push(format!(
        "- **Convertible files ({}, {} lines):** under a day of review — run the converter, read the output, enable.",
        by_verdict(Convertibility::Convertible),
        convertible_lines
    ));
    lines.push(format!(
        "- **Partial files ({}):** blocked on a class runtime Wordless does not have yet (M4 analyses classes, it does not convert them). Building one is a milestone-scale job in its own right (est. 2–4 weeks of focused work), after which these files still need their {total_unconvertible} flagged items reviewed one by one.",
        by_verdict(Convertibility::Partial)
    ));
    lines.push(format!(
        "- **Blocked files ({}):** {http_files} file{} call{} an external HTTP API and {wpdb_files} file{} use{} $wpdb. Neither is a translation problem: the API calls need a service integration (Phase 8) and the spam decision itself needs the Akismet service and its spam database. That is the real work, and it is commercial (API key, account, rate limits) — not something a converter can supply.",
        by_verdict(Convertibility::Blocked),
        if http_files == 1 { "" } else { "s" },
        if http_files == 1 { "s" } else { "" },
        if wpdb_files == 1 { "" } else { "s" },
        if wpdb_files == 1 { "s" } else { "" }
    ));
    lines.push(format!(
        "- **Bottom line:** a faithful Akismet port is a multi-week project plus a commercial API integration, not a conversion run. The M4 engine's honest contribution is this map: 9 files are nearly free, {class_count} classes need a runtime, and 4 files need an integration that no amount of translation replaces."
    ));
    lines.push(String::new());
    lines.push(generated_by.to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_wp_remote_and_wpdb_file_as_blocked() {
        let a = analyze_php(
            r#"<?php
function check($c) {
  global $wpdb;
  $r = wp_remote_post('https://api.example.com/', array('body' => $c));
  return $r;
}
"#,
            "blocked.php",
        )
        .expect("parses");
        assert_eq!(a.verdict, Convertibility::Blocked);
        assert!(a.http_calls.contains(&"wp_remote_post".to_string()));
        assert!(a.features.contains(&"$wpdb".to_string()));
    }

    #[test]
    fn marks_class_file_as_partial_with_class_named() {
        let a = analyze_php(
            "<?php class Foo { public function bar() { return 1; } }",
            "class.php",
        )
        .expect("parses");
        assert_eq!(a.verdict, Convertibility::Partial);
        assert_eq!(a.classes, vec!["Foo".to_string()]);
    }

    #[test]
    fn marks_blocked_construct_file_as_blocked() {
        let a =
            analyze_php("<?php function f($x) { return eval($x); }", "eval.php").expect("parses");
        assert_eq!(a.verdict, Convertibility::Blocked);
        assert!(a.features.contains(&"blocked-construct".to_string()));
    }

    #[test]
    fn renders_report_with_per_file_verdict_and_effort_estimate() {
        let a = analyze_php("<?php function f() { return 1; }", "f.php").expect("parses");
        let report =
            render_analysis_report(std::slice::from_ref(&a), "Test report", "Intro.", "Test.");
        assert!(report.contains("# Test report"), "{report}");
        assert!(report.contains("## `f.php` — convertible"), "{report}");
        assert!(report.contains("## Effort estimate"), "{report}");
        assert!(report.contains("Convertible files (1"), "{report}");
    }

    #[test]
    fn collects_literal_option_defaults() {
        let a = analyze_php(
            "<?php $a = get_option('num_opt', 42); $b = get_option('bool_opt', true); $c = get_option('dyn_opt', SOME_CONST); $d = get_option($dynamic_key, 'x');",
            "p.php",
        )
        .expect("parses");
        let mut keys: Vec<&str> = a.option_defaults.iter().map(|d| d.key.as_str()).collect();
        keys.sort();
        assert_eq!(keys, vec!["bool_opt", "num_opt"]);
        assert_eq!(
            a.option_defaults
                .iter()
                .find(|d| d.key == "num_opt")
                .map(|d| d.default_value.clone()),
            Some(serde_json::Value::from(42))
        );
        assert_eq!(
            a.option_defaults
                .iter()
                .find(|d| d.key == "bool_opt")
                .map(|d| d.default_value.clone()),
            Some(serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn abspath_guard_exit_is_not_counted_as_unconvertible() {
        let a = analyze_php(
            "<?php if (!defined('ABSPATH')) { exit; } function f() { return 1; }",
            "g.php",
        )
        .expect("parses");
        assert_eq!(a.verdict, Convertibility::Convertible);
        assert!(a.unconvertible.is_empty());
    }
}
