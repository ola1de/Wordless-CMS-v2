//! PHP AST → TypeScript emitter — a port of v1 `src/conversion/emitter.ts`
//! (Milestone 4).
//!
//! Translates a parsed PHP program into a Wordless extension module.
//! Every PHP function becomes a TS function taking the WordPress API
//! (`wp`) as its first parameter, `add_action` / `add_filter` become hook
//! registrations in `activate()`, and `echo` / `printf` output is
//! collected into a returned string.
//!
//! Honesty rules (same as v1):
//! - A WordPress/PHP function with no mapping is emitted as a call to a
//!   generated `__unmapped_<name>` helper that throws at runtime, and is
//!   listed in the report.
//! - Constructs the parser marked `unsupported` are emitted as comments
//!   plus a report entry, never silently dropped.
//! - WordPress data functions are async in Wordless (they hit D1) while
//!   PHP code is synchronous: the emitter propagates asyncness through
//!   the file's own call graph and awaits where needed.
//!
//! The emitter is a stateful struct (`Emitter`) because the TypeScript
//! original closes over mutable report state; the methods below follow
//! the TS function order (`emitExpr`, `emitCall`, `emitBuiltin`,
//! `emitStmt`, `emitIf`, `emitFunction`) so the two can be diffed.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::ast::{walk_stmt, Expr, IfAlternate, Node, Param, Program, Stmt};

/// One item the emitter could not translate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnconvertedItem {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub reason: String,
    pub line: usize,
}

/// One hook registration (`add_action` / `add_filter`) found at the top
/// level and translated into `activate()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookRegistration {
    pub tag: String,
    /// `"action"` or `"filter"`.
    pub kind: String,
    #[serde(rename = "functionName")]
    pub function_name: String,
    pub priority: f64,
    pub line: usize,
}

/// A transformation the emitter applied deliberately (not a gap).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transformed {
    pub what: String,
    pub note: String,
}

/// The emitter's report for one file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmitReport {
    /// PHP names of the converted functions, in source order.
    pub functions: Vec<String>,
    #[serde(rename = "asyncFunctions")]
    pub async_functions: Vec<String>,
    pub hooks: Vec<HookRegistration>,
    pub unconverted: Vec<UnconvertedItem>,
    pub warnings: Vec<String>,
    pub transformed: Vec<Transformed>,
    #[serde(rename = "helpersUsed")]
    pub helpers_used: Vec<String>,
}

/// The emitter's output for one file.
#[derive(Debug, Clone)]
pub struct EmitResult {
    pub code: String,
    pub report: EmitReport,
}

/// Options for one emission (mirrors TS `EmitOptions`).
#[derive(Debug, Clone, Default)]
pub struct EmitOptions {
    pub filename: String,
    pub slug: String,
    /// Import path for the WordPressApi type in the generated module.
    pub compat_import: Option<String>,
    /// Plugin header fields (Plugin Name, Version, Author, Description).
    pub plugin_header: HashMap<String, String>,
    /// Human-readable source label for the header comment.
    pub source_label: Option<String>,
}

/// WordPress API functions that are async in Wordless (they hit D1).
pub const WP_ASYNC: &[&str] = &[
    "get_option",
    "update_option",
    "delete_option",
    "add_option",
    "get_post",
    "get_posts",
    "wp_insert_post",
    "wp_update_post",
    "wp_delete_post",
    "get_user_meta",
    "update_user_meta",
    "wp_get_post_terms",
    "get_the_terms",
    "wp_list_categories",
    "wp_get_post_revisions",
    "wp_restore_post_revision",
    "wp_nav_menu",
    "get_media_items",
    "wp_get_attachment_url",
    "media_handle_upload",
    "wp_schedule_event",
];

/// WordPress API functions that are synchronous in Wordless.
pub const WP_SYNC: &[&str] = &[
    "get_user_locale",
    "get_locale",
    "add_action",
    "add_filter",
    "remove_action",
    "remove_filter",
    "apply_filters",
    "do_action",
    "register_post_type",
    "register_taxonomy",
    "add_shortcode",
    "current_user_can",
    "wp_enqueue_script",
    "wp_enqueue_style",
    "register_rest_route",
    "__",
    "_e",
    "esc_html",
    "esc_attr",
    "esc_html__",
    "esc_attr__",
    "wptexturize",
];

const TS_RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "await",
    "enum",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "abstract",
    "boolean",
    "byte",
    "char",
    "double",
    "final",
    "float",
    "goto",
    "int",
    "long",
    "native",
    "short",
    "transient",
    "volatile",
    "of",
    "as",
    "from",
    "get",
    "set",
    "constructor",
    "string",
    "number",
];

const SUPERGLOBALS: &[&str] = &[
    "_GET", "_POST", "_SERVER", "_FILES", "_COOKIE", "_REQUEST", "_SESSION", "_ENV", "GLOBALS",
];

/// snake_case PHP name → camelCase TypeScript name.
pub fn to_ts_name(php_name: &str) -> String {
    let chars: Vec<char> = php_name.chars().collect();
    let mut out = String::with_capacity(php_name.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '_' && i + 1 < chars.len() && chars[i + 1].is_ascii_alphanumeric() {
            out.extend(chars[i + 1].to_uppercase());
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    if TS_RESERVED.contains(&out.as_str()) {
        format!("_{out}")
    } else {
        out
    }
}

/// Map a PHP parameter type to a TypeScript type.
fn php_type_to_ts(type_: Option<&str>) -> String {
    let Some(type_) = type_ else {
        return "unknown".to_string();
    };
    let nullable = type_.starts_with('?');
    let base = if nullable { &type_[1..] } else { type_ };
    let parts: Vec<String> = base
        .split('|')
        .map(|p| match p.to_lowercase().as_str() {
            "int" | "integer" | "float" | "double" => "number".to_string(),
            "string" => "string".to_string(),
            "bool" | "boolean" => "boolean".to_string(),
            "array" => "unknown[]".to_string(),
            "callable" => "(...args: unknown[]) => unknown".to_string(),
            "mixed" | "object" => "unknown".to_string(),
            "void" => "void".to_string(),
            "null" => "null".to_string(),
            "false" => "false".to_string(),
            "true" => "true".to_string(),
            _ => "unknown".to_string(),
        })
        .collect();
    let joined = parts.join(" | ");
    if nullable {
        format!("{joined} | null")
    } else {
        joined
    }
}

/// A JS string literal for a Rust string — same escaping as
/// `JSON.stringify` for the values the converter sees.
pub fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// A JS literal for a JSON number value (`String(value)` in TS).
fn js_number(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::String(s) => js_string(s),
        _ => "null".to_string(),
    }
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

struct FunctionInfo {
    php_name: String,
    ts_name: String,
    params: Vec<Param>,
    body: Vec<Stmt>,
    is_async: bool,
    returns_output: bool,
    has_return_value: bool,
}

struct ModuleStatic {
    fn_name: String,
    var_name: String,
    init: Option<Expr>,
}

#[derive(Clone)]
struct EmitContext {
    /// Variable names assigned anywhere in the current function
    /// (pre-declared with `let`).
    declared_vars: HashSet<String>,
    /// True while emitting inside a function that collects echo output.
    in_function: bool,
    /// Depth, for indentation.
    depth: usize,
    /// Function-local static vars renamed to their module-level names.
    rename_vars: Option<HashMap<String, String>>,
}

impl EmitContext {
    fn fresh() -> Self {
        EmitContext {
            declared_vars: HashSet::new(),
            in_function: false,
            depth: 0,
            rename_vars: None,
        }
    }
}

/// The emitter. Construct per file via [`emit_typescript`].
pub struct Emitter {
    unconverted: Vec<UnconvertedItem>,
    warnings: Vec<String>,
    transformed: Vec<Transformed>,
    helpers_used: BTreeSet<String>,
    functions: Vec<FunctionInfo>,
    hook_regs: Vec<HookRegistration>,
    module_statics: Vec<ModuleStatic>,
    unmapped_helpers: BTreeSet<String>,
}

impl Emitter {
    fn note_unconverted(&mut self, kind: &str, reason: &str, line: usize, name: Option<&str>) {
        let name = name.map(str::to_string);
        if !self
            .unconverted
            .iter()
            .any(|u| u.kind == kind && u.name == name && u.line == line)
        {
            self.unconverted.push(UnconvertedItem {
                kind: kind.to_string(),
                name,
                reason: reason.to_string(),
                line,
            });
        }
    }

    fn fn_by_php(&self, php_name: &str) -> Option<&FunctionInfo> {
        self.functions.iter().find(|f| f.php_name == php_name)
    }

    fn calls_wp_async(body: &[Stmt]) -> bool {
        let mut found = false;
        for stmt in body {
            walk_stmt(stmt, &mut |n| {
                if let Node::Expr(Expr::Call {
                    name: Some(name), ..
                }) = n
                {
                    if WP_ASYNC.contains(&name.as_str()) {
                        found = true;
                    }
                }
            });
        }
        found
    }

    fn calls_function(body: &[Stmt], target: &str) -> bool {
        let mut found = false;
        for stmt in body {
            walk_stmt(stmt, &mut |n| {
                if let Node::Expr(Expr::Call {
                    name: Some(name), ..
                }) = n
                {
                    if name == target {
                        found = true;
                    }
                }
            });
        }
        found
    }
}

/// Push one function into the collection unless a function with the
/// same PHP name is already there (first definition wins).
fn push_function_info(out: &mut Vec<FunctionInfo>, name: &str, params: &[Param], body: &[Stmt]) {
    if out.iter().any(|f| f.php_name == name) {
        return;
    }
    out.push(FunctionInfo {
        ts_name: to_ts_name(name),
        php_name: name.to_string(),
        params: params.to_vec(),
        body: body.to_vec(),
        is_async: false,
        returns_output: false,
        has_return_value: false,
    });
}

/// Collect function declarations nested inside control-flow containers
/// (`if`/`foreach`/`for`/`while`/`do-while`/`try`/`block`, including the
/// alternative `:` … `endif` syntax) into `out`. First definition wins;
/// a name already collected (e.g. a top-level declaration) is kept.
/// Function and class bodies are NOT descended into — a function
/// declared inside another function stays local to it.
fn collect_nested_functions(stmt: &Stmt, out: &mut Vec<FunctionInfo>) {
    match stmt {
        Stmt::Function {
            name, params, body, ..
        } => push_function_info(out, name, params, body),
        Stmt::If {
            consequent,
            alternate,
            ..
        } => {
            for s in consequent {
                collect_nested_functions(s, out);
            }
            match alternate {
                IfAlternate::Stmts(stmts) => {
                    for s in stmts {
                        collect_nested_functions(s, out);
                    }
                }
                IfAlternate::If(inner) => collect_nested_functions(inner, out),
                IfAlternate::None => {}
            }
        }
        Stmt::Foreach { body, .. }
        | Stmt::For { body, .. }
        | Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. } => {
            for s in body {
                collect_nested_functions(s, out);
            }
        }
        Stmt::Block { body, .. } => {
            for s in body {
                collect_nested_functions(s, out);
            }
        }
        Stmt::Try {
            body,
            catches,
            finally_body,
            ..
        } => {
            for s in body {
                collect_nested_functions(s, out);
            }
            for c in catches {
                for s in &c.body {
                    collect_nested_functions(s, out);
                }
            }
            if let Some(finally_stmts) = finally_body {
                for s in finally_stmts {
                    collect_nested_functions(s, out);
                }
            }
        }
        // Function/Class/Method bodies, Switch (no statement body in
        // this AST), and leaf statements hold no hoistable functions.
        _ => {}
    }
}

/// Emit TypeScript for a parsed program. Mirrors TS `emitTypeScript`.
pub fn emit_typescript(program: &Program, options: &EmitOptions) -> EmitResult {
    let compat_import = options
        .compat_import
        .clone()
        .unwrap_or_else(|| "../../src/compat/wordpress".to_string());
    let mut e = Emitter {
        unconverted: Vec::new(),
        warnings: Vec::new(),
        transformed: Vec::new(),
        helpers_used: BTreeSet::new(),
        functions: Vec::new(),
        hook_regs: Vec::new(),
        module_statics: Vec::new(),
        unmapped_helpers: BTreeSet::new(),
    };

    // ---------------------------------------------------------- first pass
    let mut top_level_stmts: Vec<&Stmt> = Vec::new();
    let mut class_decls: Vec<&Stmt> = Vec::new();
    for stmt in &program.body {
        match stmt {
            Stmt::Function {
                name,
                params,
                body,
                pos: _,
                ..
            } => {
                e.functions.push(FunctionInfo {
                    ts_name: to_ts_name(name),
                    php_name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                    is_async: false,
                    returns_output: false,
                    has_return_value: false,
                });
            }
            Stmt::Class { .. } => class_decls.push(stmt),
            _ => top_level_stmts.push(stmt),
        }
    }
    // Functions declared inside control-flow containers — the
    // `if (!function_exists('x')) : function x() ... endif;` guard
    // every WordPress theme uses — are hoisted to module level too.
    // PHP defines them at compile time regardless of the guard, so
    // collecting them here is faithful; without it their
    // add_action/add_filter registrations are silently dropped
    // (fn_by_php cannot find the callback) and the guard's
    // function_exists() call has nothing to resolve against. Only
    // control-flow containers are descended into: a function
    // declared inside another function's body stays local to it.
    for stmt in &program.body {
        collect_nested_functions(stmt, &mut e.functions);
    }

    // Collect hook registrations at the top level.
    for stmt in &top_level_stmts {
        let Stmt::ExprStmt { expr, .. } = stmt else {
            continue;
        };
        let Expr::Call {
            name: Some(name),
            args,
            pos,
            ..
        } = expr
        else {
            continue;
        };
        if name != "add_action" && name != "add_filter" {
            continue;
        }
        let Some(tag_arg) = args.first() else {
            continue;
        };
        let Some(cb_arg) = args.get(1) else {
            continue;
        };
        let Expr::Literal {
            literal_kind,
            value,
            ..
        } = tag_arg
        else {
            e.note_unconverted(
                "hook registration",
                "add_action/add_filter with a non-literal hook tag is not translated.",
                pos.line,
                None,
            );
            continue;
        };
        if literal_kind != "string" {
            e.note_unconverted(
                "hook registration",
                "add_action/add_filter with a non-literal hook tag is not translated.",
                pos.line,
                None,
            );
            continue;
        }
        let tag = match value {
            serde_json::Value::String(s) => s.clone(),
            _ => String::new(),
        };
        let priority = match args.get(2) {
            Some(Expr::Literal {
                literal_kind,
                value,
                ..
            }) if literal_kind == "number" => value.as_f64().unwrap_or(10.0),
            _ => 10.0,
        };
        match cb_arg {
            Expr::Literal {
                literal_kind,
                value,
                ..
            } if literal_kind == "string" => {
                let fn_name = match value {
                    serde_json::Value::String(s) => s.clone(),
                    _ => String::new(),
                };
                if e.fn_by_php(&fn_name).is_some() {
                    e.hook_regs.push(HookRegistration {
                        tag,
                        kind: if name == "add_action" {
                            "action".to_string()
                        } else {
                            "filter".to_string()
                        },
                        function_name: fn_name,
                        priority,
                        line: pos.line,
                    });
                } else {
                    e.note_unconverted(
                        "hook callback",
                        &format!(
                            "Hook \"{tag}\" references function {fn_name}(), which is not defined in this file."
                        ),
                        pos.line,
                        Some(&fn_name),
                    );
                }
            }
            Expr::Closure { .. } => {
                e.note_unconverted(
                    "hook callback",
                    &format!(
                        "Hook \"{tag}\" uses a closure callback; closures as hook callbacks are not translated yet."
                    ),
                    pos.line,
                    None,
                );
            }
            _ => {
                e.note_unconverted(
                    "hook callback",
                    &format!(
                        "Hook \"{tag}\" uses a non-literal callback (array callable or variable); only a function-name string is translated."
                    ),
                    pos.line,
                    None,
                );
            }
        }
    }

    // Async analysis: a function is async when it calls an async WP
    // function or another async function from this file. Fixpoint over
    // the call graph.
    for info in e.functions.iter_mut() {
        info.is_async = Emitter::calls_wp_async(&info.body);
    }
    loop {
        let mut changed = false;
        let snapshot: Vec<(String, bool)> = e
            .functions
            .iter()
            .map(|f| (f.php_name.clone(), f.is_async))
            .collect();
        for info in e.functions.iter_mut() {
            if info.is_async {
                continue;
            }
            for (other_name, other_async) in &snapshot {
                if *other_async && Emitter::calls_function(&info.body, other_name) {
                    info.is_async = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Per-function output/return analysis.
    for info in e.functions.iter_mut() {
        let mut returns_output = false;
        let mut has_return_value = false;
        for stmt in &info.body {
            walk_stmt(stmt, &mut |n| match n {
                Node::Stmt(Stmt::Echo { .. }) => returns_output = true,
                Node::Expr(Expr::Call {
                    name: Some(name), ..
                }) if name == "printf" => {
                    returns_output = true;
                }
                Node::Stmt(Stmt::Return { value: Some(_), .. }) => has_return_value = true,
                _ => {}
            });
        }
        info.returns_output = returns_output;
        info.has_return_value = has_return_value;
    }

    // ------------------------------------------------------------ emission
    let header = &options.plugin_header;
    let ext_name = header
        .get("Plugin Name")
        .cloned()
        .unwrap_or_else(|| options.slug.clone());
    let ext_version = header
        .get("Version")
        .cloned()
        .unwrap_or_else(|| "0.1.0".to_string());
    let ext_author = header.get("Author").cloned().unwrap_or_default();
    let ext_description = header.get("Description").cloned().unwrap_or_default();

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "// Generated by the Wordless conversion engine (Milestone 4) from {}.",
        options.filename
    ));
    lines.push(format!(
        "// Source: {} — converted statically, never executed.",
        options
            .source_label
            .clone()
            .unwrap_or_else(|| options.filename.clone())
    ));
    lines.push(
        "// DO NOT EDIT BY HAND: regenerate with the converter. Review before enabling."
            .to_string(),
    );
    lines.push("// Sandbox: this module runs only through the Wordless extension API,".to_string());
    lines.push("// with the same capability checks as any extension. It cannot load".to_string());
    lines.push("// code dynamically, run shell commands, or touch PHP.".to_string());
    lines.push(format!(
        "import type {{ WordPressApi }} from \"{compat_import}\";"
    ));
    lines.push(String::new());

    // -- function emission (populates module_statics, helpers, warnings)
    let mut function_blocks: Vec<String> = Vec::new();
    // Clone the function list: emit_function borrows &mut self.
    let fn_count = e.functions.len();
    for idx in 0..fn_count {
        let block = e.emit_function(idx);
        function_blocks.push(block);
    }

    // Top-level statements that are not functions/classes/hooks become
    // module-level code (rare; usually only stray echo/inline HTML).
    let mut top_level_code: Vec<String> = Vec::new();
    for stmt in &top_level_stmts {
        if let Stmt::ExprStmt { expr, .. } = stmt {
            if let Expr::Call {
                name: Some(name), ..
            } = expr
            {
                if name == "add_action" || name == "add_filter" {
                    continue;
                }
            }
        }
        if matches!(stmt, Stmt::Class { .. }) {
            continue;
        }
        let ctx = EmitContext::fresh();
        let text = e.emit_stmt(stmt, &ctx);
        if !text.is_empty() {
            top_level_code.push(text);
        }
    }

    for cls in &class_decls {
        let (name, methods, extends_name, line) = match cls {
            Stmt::Class {
                name,
                methods,
                extends_name,
                pos,
                ..
            } => (name.clone(), methods.len(), extends_name.clone(), pos.line),
            _ => unreachable!(),
        };
        let plural = if methods == 1 { "" } else { "s" };
        let extends = match &extends_name {
            Some(x) => format!(", extends {x}"),
            None => String::new(),
        };
        e.note_unconverted(
            "class declaration",
            &format!(
                "Class {name} ({methods} method{plural}{extends}) is analysed but not converted: Wordless has no class runtime yet."
            ),
            line,
            Some(&name),
        );
        top_level_code.push(format!(
            "/* NOT CONVERTED (line {line}): class {name} — analysed but not converted; Wordless has no class runtime yet. See the conversion report. */"
        ));
    }

    // Hook wrappers for activate(): action callbacks that produce output
    // have their output captured in renderedOutput; filter callbacks pass
    // the value through.
    let needs_rendered_output = e.hook_regs.iter().any(|h| {
        h.kind == "action"
            && e.fn_by_php(&h.function_name)
                .map(|info| info.returns_output)
                .unwrap_or(false)
    }) || top_level_code
        .iter()
        .any(|c| c.contains("renderedOutput.push"));

    let mut activate_lines: Vec<String> = Vec::new();
    activate_lines
        .push("/** Register this extension's hooks with the Wordless kernel. */".to_string());
    activate_lines.push("export function activate(wp: WordPressApi): void {".to_string());
    if e.hook_regs.is_empty() {
        activate_lines.push(
            "  // No hooks registered: this file declares no add_action/add_filter calls."
                .to_string(),
        );
    }
    for reg in e.hook_regs.clone() {
        let info = e
            .fn_by_php(&reg.function_name)
            .map(|f| (f.ts_name.clone(), f.returns_output, f.is_async));
        let ts_name = info
            .as_ref()
            .map(|(t, _, _)| t.clone())
            .unwrap_or_else(|| to_ts_name(&reg.function_name));
        let prio = if reg.priority != 10.0 {
            format!(", {}", fmt_priority(reg.priority))
        } else {
            String::new()
        };
        if reg.kind == "action" {
            match &info {
                Some((_, returns_output, is_async)) if *returns_output => {
                    if *is_async {
                        e.warnings.push(format!(
                            "Async action callback {}() on \"{}\": the hook system is synchronous, so its output cannot be captured — the callback runs but its output is discarded.",
                            reg.function_name, reg.tag
                        ));
                        activate_lines.push(format!(
                            "  wp.add_action(\"{}\", () => {{ void {}(wp); }}{});",
                            reg.tag, ts_name, prio
                        ));
                    } else {
                        activate_lines.push(format!(
                            "  wp.add_action(\"{}\", () => {{ renderedOutput.push({}(wp)); }}{});",
                            reg.tag, ts_name, prio
                        ));
                    }
                }
                Some((_, _, true)) => {
                    activate_lines.push(format!(
                        "  wp.add_action(\"{}\", (...hookArgs: unknown[]) => {{ void {}(wp, ...hookArgs); }}{});",
                        reg.tag, ts_name, prio
                    ));
                }
                _ => {
                    activate_lines.push(format!(
                        "  wp.add_action(\"{}\", (...hookArgs: unknown[]) => {}(wp, ...hookArgs){});",
                        reg.tag, ts_name, prio
                    ));
                }
            }
        } else {
            activate_lines.push(format!(
                "  wp.add_filter(\"{}\", (value: unknown, ...hookArgs: unknown[]) => {}(wp, value, ...hookArgs){});",
                reg.tag, ts_name, prio
            ));
        }
    }
    activate_lines.push("}".to_string());

    if needs_rendered_output {
        lines.push(
            "/** Output captured from action callbacks (admin notices, head output). */"
                .to_string(),
        );
        lines.push("export const renderedOutput: string[] = [];".to_string());
        lines.push(String::new());
    }
    if !e.module_statics.is_empty() {
        lines.push("/** Function-local static variables, hoisted to module level (PHP statics persist across calls). */".to_string());
        let statics = std::mem::take(&mut e.module_statics);
        for s in &statics {
            let init = match &s.init {
                Some(expr) => {
                    let ctx = EmitContext::fresh();
                    e.emit_expr(expr, &ctx)
                }
                None => "null".to_string(),
            };
            lines.push(format!(
                "let __static_{}_{} = {};",
                s.fn_name, s.var_name, init
            ));
        }
        lines.push(String::new());
    }
    if !function_blocks.is_empty() {
        lines.extend(function_blocks);
        lines.push(String::new());
    }
    if !top_level_code.is_empty() {
        lines.push(
            "// Top-level statements and unconverted declarations from the source file."
                .to_string(),
        );
        lines.extend(top_level_code);
        lines.push(String::new());
    }
    lines.extend(activate_lines);
    lines.push(String::new());

    // Extension manifest (from the plugin header when present).
    lines.push("export const extensionMeta = {".to_string());
    lines.push(format!("  slug: {},", js_string(&options.slug)));
    lines.push(format!("  name: {},", js_string(&ext_name)));
    lines.push(format!("  version: {},", js_string(&ext_version)));
    if !ext_author.is_empty() {
        lines.push(format!("  author: {},", js_string(&ext_author)));
    }
    if !ext_description.is_empty() {
        lines.push(format!("  description: {},", js_string(&ext_description)));
    }
    lines.push(format!("  sourceFile: {},", js_string(&options.filename)));
    lines.push("  generatedBy: \"wordless-conversion-m4\",".to_string());
    lines.push("} as const;".to_string());

    // Helpers (only the ones actually used).
    let mut helper_blocks: Vec<String> = Vec::new();
    if e.helpers_used.contains("__sprintf") {
        helper_blocks.push(SPRINTF_HELPER.to_string());
    }
    if e.helpers_used.contains("__rand") {
        helper_blocks.push(RAND_HELPER.to_string());
    }
    if e.helpers_used.contains("__substr") {
        helper_blocks.push(SUBSTR_HELPER.to_string());
    }
    if e.helpers_used.contains("__escHtml") {
        helper_blocks.push(ESC_HTML_HELPER.to_string());
    }
    if e.helpers_used.contains("__unsupported") {
        helper_blocks.push(UNSUPPORTED_HELPER.to_string());
    }
    let mut unmapped: Vec<String> = e.unmapped_helpers.iter().cloned().collect();
    unmapped.sort();
    for helper in unmapped {
        let fn_name = helper.strip_prefix("__unmapped_").unwrap_or(&helper);
        helper_blocks.push(format!(
            "/** {fn_name}() has no Wordless mapping yet — see the conversion report. */\nfunction {helper}(...args: unknown[]): unknown {{\n  void args;\n  throw new Error(\"Unmapped function {fn_name}() — this converted extension needs manual work (see the conversion report).\");\n}}"
        ));
    }
    if !helper_blocks.is_empty() {
        lines.push(String::new());
        lines.extend(helper_blocks);
    }

    let report = EmitReport {
        functions: e.functions.iter().map(|f| f.php_name.clone()).collect(),
        async_functions: e
            .functions
            .iter()
            .filter(|f| f.is_async)
            .map(|f| f.php_name.clone())
            .collect(),
        hooks: e.hook_regs.clone(),
        unconverted: e.unconverted.clone(),
        warnings: e.warnings.clone(),
        transformed: e.transformed.clone(),
        helpers_used: e.helpers_used.iter().cloned().collect(),
    };

    EmitResult {
        code: lines.join("\n") + "\n",
        report,
    }
}

fn fmt_priority(p: f64) -> String {
    if p.fract() == 0.0 && p.abs() < 9007199254740992.0 {
        format!("{}", p as i64)
    } else {
        format!("{p}")
    }
}

const SPRINTF_HELPER: &str = r#"/** Minimal sprintf: %s %d %f %x %X %c and %% (enough for converted plugins). */
function __sprintf(format: string, ...args: unknown[]): string {
  let i = 0;
  return format.replace(/%([%bcdeEfFgGosuxX])/g, (_m: string, spec: string) => {
    if (spec === "%") return "%";
    const arg = args[i++];
    if (arg === undefined) return "";
    if (spec === "d" || spec === "i" || spec === "u") return String(Math.trunc(Number(arg)));
    if (spec === "f" || spec === "e" || spec === "E" || spec === "g" || spec === "G") return String(Number(arg));
    if (spec === "x") return Number(arg).toString(16);
    if (spec === "X") return Number(arg).toString(16).toUpperCase();
    if (spec === "c") return String.fromCharCode(Number(arg));
    return String(arg);
  });
}"#;

const RAND_HELPER: &str = r#"/** mt_rand($min, $max): inclusive random integer. */
function __rand(min: number, max: number): number {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}"#;

const SUBSTR_HELPER: &str = r#"/** PHP substr() semantics (negative start/length count from the end). */
function __substr(value: string, start: number, length?: number): string {
  const str = String(value);
  const len = str.length;
  const s = start < 0 ? Math.max(len + start, 0) : Math.min(start, len);
  if (length === undefined) return str.slice(s);
  if (length < 0) return str.slice(s, Math.max(len + length, s));
  return str.slice(s, s + length);
}"#;

const ESC_HTML_HELPER: &str = r#"/** htmlspecialchars(): escape HTML special characters. */
function __escHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}"#;

const UNSUPPORTED_HELPER: &str = r#"/** A PHP construct the converter could not translate. Throws if reached. */
function __unsupported(reason: string): never {
  throw new Error("Unconverted PHP construct: " + reason);
}"#;

// ---------------------------------------------------------------------------
// Expression emission
// ---------------------------------------------------------------------------

impl Emitter {
    fn emit_expr(&mut self, expr: &Expr, ctx: &EmitContext) -> String {
        match expr {
            Expr::Literal {
                literal_kind,
                value,
                ..
            } => {
                if literal_kind == "string" {
                    match value {
                        serde_json::Value::String(s) => js_string(s),
                        _ => js_string(""),
                    }
                } else if literal_kind == "null" {
                    "null".to_string()
                } else {
                    js_number(value)
                }
            }
            Expr::Var { name, pos } => {
                if let Some(rename) = &ctx.rename_vars {
                    if let Some(hoisted) = rename.get(name) {
                        return hoisted.clone();
                    }
                }
                if name == "this" {
                    self.note_unconverted(
                        "this",
                        "$this has no equivalent: Wordless extensions are functions, not objects.",
                        pos.line,
                        None,
                    );
                    self.helpers_used.insert("__unsupported".to_string());
                    return "__unsupported(\"$this is not available in a Wordless extension\")"
                        .to_string();
                }
                if SUPERGLOBALS.contains(&name.as_str()) {
                    self.note_unconverted(
                        "superglobal",
                        &format!(
                            "${name} is not available in a Wordless extension; use the request context API (Phase 6)."
                        ),
                        pos.line,
                        Some(&format!("${name}")),
                    );
                    self.helpers_used.insert("__unsupported".to_string());
                    return format!("__unsupported(\"superglobal ${name} is not available\")");
                }
                name.clone()
            }
            Expr::Array { entries, pos } => {
                let all_keyless = entries.iter().all(|en| en.key.is_none());
                if all_keyless {
                    let vals: Vec<String> = entries
                        .iter()
                        .map(|en| self.emit_expr(&en.value, ctx))
                        .collect();
                    return format!("[{}]", vals.join(", "));
                }
                let sequential = entries.iter().enumerate().all(|(i, en)| match &en.key {
                    Some(Expr::Literal {
                        literal_kind,
                        value,
                        ..
                    }) if literal_kind == "number" => value.as_i64() == Some(i as i64),
                    _ => false,
                });
                if sequential {
                    let vals: Vec<String> = entries
                        .iter()
                        .map(|en| self.emit_expr(&en.value, ctx))
                        .collect();
                    return format!("[{}]", vals.join(", "));
                }
                let mut parts: Vec<String> = Vec::new();
                for en in entries {
                    let value = self.emit_expr(&en.value, ctx);
                    match &en.key {
                        None => parts.push(value),
                        Some(Expr::Literal {
                            literal_kind,
                            value: kv,
                            ..
                        }) if literal_kind == "string" || literal_kind == "number" => {
                            parts.push(format!("{}: {}", js_string(&js_number(kv)), value));
                        }
                        Some(k) => parts.push(format!("[{}]: {}", self.emit_expr(k, ctx), value)),
                    }
                }
                if entries.iter().any(|en| en.key.is_none()) {
                    self.warnings.push(format!(
                        "Mixed list/object PHP array at line {} was emitted as an object; list entries use numeric keys.",
                        pos.line
                    ));
                }
                format!("{{ {} }}", parts.join(", "))
            }
            Expr::ArrayAccess {
                object,
                index,
                append,
                ..
            } => {
                if *append {
                    return self.emit_expr(object, ctx);
                }
                let obj = self.emit_expr(object, ctx);
                let idx = match index {
                    Some(i) => self.emit_expr(i, ctx),
                    None => String::new(),
                };
                format!("{obj}[{idx}]")
            }
            Expr::Binary {
                operator,
                left,
                right,
                pos,
            } => {
                let l = self.emit_expr(left, ctx);
                let r = self.emit_expr(right, ctx);
                match operator.as_str() {
                    "." => format!("({l} + {r})"),
                    "and" => format!("({l} && {r})"),
                    "or" => format!("({l} || {r})"),
                    "xor" => {
                        self.warnings.push(format!(
                            "PHP \"xor\" at line {} was translated as !== (boolean xor).",
                            pos.line
                        ));
                        format!("({l} !== {r})")
                    }
                    "<=>" => format!("({l} < {r} ? -1 : {l} > {r} ? 1 : 0)"),
                    _ => format!("({l} {operator} {r})"),
                }
            }
            Expr::Unary {
                operator,
                operand,
                prefix,
                pos,
            } => {
                let operand = self.emit_expr(operand, ctx);
                if operator == "@" {
                    self.warnings.push(format!(
                        "Error-suppression operator @ at line {} was removed (Wordless never suppresses errors).",
                        pos.line
                    ));
                    return operand;
                }
                if *prefix {
                    format!("({operator}{operand})")
                } else {
                    format!("({operand}{operator})")
                }
            }
            Expr::Ternary {
                test,
                consequent,
                alternate,
                ..
            } => {
                let t = self.emit_expr(test, ctx);
                let alt = self.emit_expr(alternate, ctx);
                match consequent {
                    None => format!("({t} ?? {alt})"),
                    Some(c) => {
                        let c = self.emit_expr(c, ctx);
                        format!("({t} ? {c} : {alt})")
                    }
                }
            }
            Expr::InterpolatedString { parts, .. } => {
                let mut out = String::new();
                for p in parts {
                    if p.kind == "text" {
                        out.push_str(&escape_template_text(&p.value));
                    } else {
                        out.push_str(&format!("${{{}}}", p.value));
                    }
                }
                format!("`{out}`")
            }
            Expr::Assign { .. } => self.emit_assign(expr, ctx),
            Expr::Call { .. } => self.emit_call(expr, ctx),
            Expr::MethodCall { method, pos, .. } => {
                self.note_unconverted(
                    "method call",
                    &format!(
                        "Method call $...->{method}() needs a class runtime, which Wordless does not provide yet."
                    ),
                    pos.line,
                    Some(method),
                );
                self.helpers_used.insert("__unsupported".to_string());
                format!("__unsupported(\"method call ->{method}() is not converted\")")
            }
            Expr::StaticCall {
                class_name,
                member,
                pos,
                ..
            } => {
                self.note_unconverted(
                    "static call",
                    &format!(
                        "Static call {class_name}::{member}() needs a class runtime, which Wordless does not provide yet."
                    ),
                    pos.line,
                    Some(member),
                );
                self.helpers_used.insert("__unsupported".to_string());
                format!("__unsupported(\"static call {class_name}::{member}() is not converted\")")
            }
            Expr::PropertyFetch { property, pos, .. } => {
                self.note_unconverted(
                    "property fetch",
                    &format!(
                        "Property fetch ->{property} needs a class runtime, which Wordless does not provide yet."
                    ),
                    pos.line,
                    Some(property),
                );
                self.helpers_used.insert("__unsupported".to_string());
                format!("__unsupported(\"property fetch ->{property} is not converted\")")
            }
            Expr::ConstFetch { name, pos } => match name.as_str() {
                "__FILE__" | "__DIR__" | "__LINE__" | "__FUNCTION__" | "__CLASS__"
                | "__METHOD__" | "__NAMESPACE__" => {
                    self.note_unconverted(
                        "magic constant",
                        &format!("{name} has no equivalent in a converted extension."),
                        pos.line,
                        Some(name),
                    );
                    self.helpers_used.insert("__unsupported".to_string());
                    format!("__unsupported(\"magic constant {name} is not available\")")
                }
                "DIRECTORY_SEPARATOR" => "\"/\"".to_string(),
                "PHP_EOL" => "\"\\n\"".to_string(),
                "PHP_VERSION" => "\"8.0\"".to_string(),
                _ => {
                    self.note_unconverted(
                        "constant",
                        &format!(
                            "Constant {name} is not defined in this file; it may come from another file or WordPress core."
                        ),
                        pos.line,
                        Some(name),
                    );
                    self.helpers_used.insert("__unsupported".to_string());
                    format!("__unsupported(\"constant {name} is not defined\")")
                }
            },
            Expr::New {
                class_name, pos, ..
            } => {
                self.note_unconverted(
                    "instantiation",
                    &format!(
                        "new {class_name}() needs a class runtime, which Wordless does not provide yet."
                    ),
                    pos.line,
                    Some(class_name),
                );
                self.helpers_used.insert("__unsupported".to_string());
                format!("__unsupported(\"new {class_name}() is not converted\")")
            }
            Expr::Closure {
                params,
                uses,
                body,
                pos,
            } => {
                if !uses.is_empty() {
                    self.note_unconverted(
                        "closure",
                        "Closures that capture variables with `use` are not translated.",
                        pos.line,
                        None,
                    );
                    self.helpers_used.insert("__unsupported".to_string());
                    return "__unsupported(\"closure with use() is not converted\")".to_string();
                }
                let params_text = params
                    .iter()
                    .map(|p| self.emit_param(p))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut sub_vars = HashSet::new();
                self.collect_assigned_vars(body, &mut sub_vars);
                for p in params {
                    sub_vars.remove(&p.name);
                }
                let mut sub = ctx.clone();
                sub.declared_vars = sub_vars;
                sub.depth = ctx.depth + 1;
                let mut sorted: Vec<String> = sub.declared_vars.iter().cloned().collect();
                sorted.sort();
                let decls = sorted
                    .iter()
                    .map(|n| format!("{}let {n};", indent(ctx.depth + 1)))
                    .collect::<Vec<_>>()
                    .join("\n");
                let body_lines = self.emit_stmt_list(body, &sub);
                let inner = [decls, body_lines]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("({params_text}) => {{\n{inner}\n{}}}", indent(ctx.depth))
            }
            Expr::Cast {
                cast_type,
                operand,
                pos,
            } => {
                let inner = self.emit_expr(operand, ctx);
                match cast_type.as_str() {
                    "int" | "integer" => format!("Math.trunc(Number({inner}))"),
                    "float" | "double" | "real" => format!("Number({inner})"),
                    "string" | "binary" => format!("String({inner})"),
                    "bool" | "boolean" => format!("Boolean({inner})"),
                    "array" => format!("(Array.isArray({inner}) ? {inner} : [{inner}])"),
                    "unset" => "null".to_string(),
                    _ => {
                        self.note_unconverted(
                            "cast",
                            "(object) cast has no direct equivalent; the value is used as-is.",
                            pos.line,
                            None,
                        );
                        inner
                    }
                }
            }
            Expr::UnsupportedExpr {
                label, reason, pos, ..
            } => {
                self.note_unconverted("expression", reason, pos.line, Some(label));
                self.helpers_used.insert("__unsupported".to_string());
                format!("__unsupported({})", js_string(reason))
            }
            Expr::Sequence { exprs, .. } => {
                let parts: Vec<String> = exprs.iter().map(|ex| self.emit_expr(ex, ctx)).collect();
                format!("({})", parts.join(", "))
            }
        }
    }

    fn emit_param(&mut self, p: &Param) -> String {
        let ts_type = php_type_to_ts(p.type_.as_deref());
        let name = if p.variadic {
            format!("...{}", p.name)
        } else {
            p.name.clone()
        };
        if let Some(default) = &p.default_value {
            let ctx = EmitContext::fresh();
            return format!("{name}: {ts_type} = {}", self.emit_expr(default, &ctx));
        }
        if p.type_.is_none() && !p.variadic {
            return format!("{name}: unknown");
        }
        format!("{name}: {ts_type}")
    }

    /// Collect variable names assigned anywhere in a statement list.
    fn collect_assigned_vars(&self, stmts: &[Stmt], into: &mut HashSet<String>) {
        for stmt in stmts {
            walk_stmt(stmt, &mut |n| match n {
                Node::Expr(Expr::Assign { target, .. }) => {
                    if let Expr::Var { name, .. } = target.as_ref() {
                        into.insert(name.clone());
                    }
                }
                Node::Stmt(Stmt::Foreach {
                    key_var, value_var, ..
                }) => {
                    into.insert(value_var.clone());
                    if let Some(k) = key_var {
                        into.insert(k.clone());
                    }
                }
                Node::Stmt(Stmt::For {
                    init: Some(Expr::Assign { target, .. }),
                    ..
                }) => {
                    if let Expr::Var { name, .. } = target.as_ref() {
                        into.insert(name.clone());
                    }
                }
                _ => {}
            });
        }
    }

    fn emit_assign(&mut self, node: &Expr, ctx: &EmitContext) -> String {
        let Expr::Assign {
            operator,
            target,
            value,
            pos,
        } = node
        else {
            return String::new();
        };
        // Assignment through an untranslatable target (e.g. $$key = $val).
        if matches!(target.as_ref(), Expr::UnsupportedExpr { .. }) {
            self.note_unconverted(
                "variable variable",
                "Assignment through a variable variable ($$key) is not translated; name the variable directly.",
                pos.line,
                None,
            );
            return format!("void ({})", self.emit_expr(value, ctx));
        }
        // list($a, $b) = expr → destructuring.
        if let Expr::Call {
            name: Some(name),
            args,
            ..
        } = target.as_ref()
        {
            if name == "__list_assign" {
                let names: Vec<String> = args
                    .iter()
                    .filter_map(|a| match a {
                        Expr::Var { name, .. } => Some(name.clone()),
                        _ => None,
                    })
                    .collect();
                return format!("([{}] = {})", names.join(", "), self.emit_expr(value, ctx));
            }
        }
        if let Expr::ArrayAccess { object, append, .. } = target.as_ref() {
            if *append {
                if operator != "=" {
                    self.warnings.push(format!(
                        "Append assignment with operator {operator} at line {} was simplified to push().",
                        pos.line
                    ));
                }
                return format!(
                    "{}.push({})",
                    self.emit_expr(object, ctx),
                    self.emit_expr(value, ctx)
                );
            }
        }
        let target_text = self.emit_expr(target, ctx);
        let value_text = self.emit_expr(value, ctx);
        if operator == "=" {
            format!("({target_text} = {value_text})")
        } else {
            format!("({target_text} {operator} {value_text})")
        }
    }

    /// Emit a call argument. A direct array read ($arr[$i]) is asserted
    /// non-null (same rule as the TS emitter's `emitCallArg`).
    fn emit_call_arg(&mut self, a: &Expr, ctx: &EmitContext) -> String {
        let text = self.emit_expr(a, ctx);
        if matches!(a, Expr::ArrayAccess { .. }) {
            format!("{text}!")
        } else {
            text
        }
    }

    fn emit_call(&mut self, expr: &Expr, ctx: &EmitContext) -> String {
        let Expr::Call {
            name, args, pos, ..
        } = expr
        else {
            return String::new();
        };
        // Dynamic call ($fn()): not translatable.
        let Some(name) = name else {
            self.note_unconverted(
                "dynamic call",
                "Calling a function through a variable is not translated; name the function directly.",
                pos.line,
                None,
            );
            self.helpers_used.insert("__unsupported".to_string());
            return "__unsupported(\"dynamic function call is not converted\")".to_string();
        };
        let lower = name.to_lowercase();

        // isset / empty (parsed as calls by the parser).
        if lower == "isset" {
            let Some(target) = args.first() else {
                return "false".to_string();
            };
            if let Expr::Var { name: v, .. } = target {
                return format!("(typeof {v} !== \"undefined\" && {v} !== null)");
            }
            let t = self.emit_expr(target, ctx);
            return format!("({t} !== undefined && {t} !== null)");
        }
        if lower == "empty" {
            let Some(target) = args.first() else {
                return "true".to_string();
            };
            return format!("(!{})", self.emit_expr(target, ctx));
        }

        // printf / sprintf.
        if lower == "printf" || lower == "sprintf" {
            self.helpers_used.insert("__sprintf".to_string());
            let a: Vec<String> = args.iter().map(|x| self.emit_expr(x, ctx)).collect();
            return format!("__sprintf({})", a.join(", "));
        }

        // WordPress API (sync + async).
        if WP_ASYNC.contains(&lower.as_str()) || WP_SYNC.contains(&lower.as_str()) {
            return self.emit_wp_call(&lower, expr, ctx);
        }

        // Local function from this file.
        if let Some(local) = self.fn_by_php(name) {
            let (ts_name, is_async) = (local.ts_name.clone(), local.is_async);
            let a: Vec<String> = args.iter().map(|x| self.emit_expr(x, ctx)).collect();
            let awaited = if is_async { "await " } else { "" };
            let rest = if a.is_empty() {
                String::new()
            } else {
                format!(", {}", a.join(", "))
            };
            return format!("{awaited}{ts_name}(wp{rest})");
        }

        // Built-in PHP functions.
        if let Some(builtin) = self.emit_builtin(&lower, expr, ctx) {
            return builtin;
        }

        // add_action / add_filter inside a function body (top-level ones
        // are handled by activate() generation). Note: in practice this
        // branch is unreachable — add_action/add_filter are in WP_SYNC
        // and handled above — but it is kept to mirror the TS emitter.
        if lower == "add_action" || lower == "add_filter" {
            return self.emit_hook_call(&lower, expr, ctx);
        }

        // Unmapped: throwing helper + report entry.
        self.note_unconverted(
            "function call",
            &format!("No Wordless mapping for {name}() yet; the generated code throws if this line runs."),
            pos.line,
            Some(name),
        );
        let helper = format!(
            "__unmapped_{}",
            name.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                })
                .collect::<String>()
        );
        self.unmapped_helpers.insert(helper.clone());
        self.helpers_used.insert(helper.clone());
        let a: Vec<String> = args.iter().map(|x| self.emit_call_arg(x, ctx)).collect();
        format!("{helper}({})", a.join(", "))
    }

    fn emit_wp_call(&mut self, lower: &str, expr: &Expr, ctx: &EmitContext) -> String {
        let Expr::Call { args, .. } = expr else {
            return String::new();
        };
        let a: Vec<String> = args.iter().map(|x| self.emit_call_arg(x, ctx)).collect();
        let awaited = if WP_ASYNC.contains(&lower) {
            "await "
        } else {
            ""
        };
        match lower {
            "esc_html__" => format!(
                "wp.esc_html(wp.__({}))",
                a.first().cloned().unwrap_or_else(|| "\"\"".to_string())
            ),
            "esc_attr__" => format!(
                "wp.esc_attr(wp.__({}))",
                a.first().cloned().unwrap_or_else(|| "\"\"".to_string())
            ),
            "_e" => format!(
                "wp.__({})",
                a.first().cloned().unwrap_or_else(|| "\"\"".to_string())
            ),
            _ => format!("{awaited}wp.{lower}({})", a.join(", ")),
        }
    }

    fn emit_hook_call(&mut self, kind: &str, expr: &Expr, ctx: &EmitContext) -> String {
        let Expr::Call { args, pos, .. } = expr else {
            return String::new();
        };
        let tag = match args.first() {
            Some(a) => self.emit_expr(a, ctx),
            None => "\"\"".to_string(),
        };
        let priority = match args.get(2) {
            Some(Expr::Literal {
                literal_kind,
                value,
                ..
            }) if literal_kind == "number" => {
                format!(
                    ", {}",
                    value
                        .as_f64()
                        .map(fmt_priority)
                        .unwrap_or_else(|| "10".to_string())
                )
            }
            _ => String::new(),
        };
        if let Some(cb) = args.get(1) {
            if let Expr::Literal {
                literal_kind,
                value,
                ..
            } = cb
            {
                if literal_kind == "string" {
                    let fn_name = match value {
                        serde_json::Value::String(s) => s.clone(),
                        _ => String::new(),
                    };
                    if let Some(local) = self.fn_by_php(&fn_name) {
                        let (ts_name, is_async) = (local.ts_name.clone(), local.is_async);
                        let awaited = if is_async { "void " } else { "" };
                        return format!(
                            "{awaited}wp.{kind}({tag}, (...hookArgs: unknown[]) => {ts_name}(wp, ...hookArgs){priority})"
                        );
                    }
                    self.note_unconverted(
                        "hook callback",
                        &format!("Hook callback {fn_name}() is not defined in this file."),
                        pos.line,
                        Some(&fn_name),
                    );
                    self.helpers_used.insert("__unsupported".to_string());
                    return format!("__unsupported(\"hook callback {fn_name}() is not defined\")");
                }
            }
            if let Expr::Closure { .. } = cb {
                return format!("wp.{kind}({tag}, {}{priority})", self.emit_expr(cb, ctx));
            }
        }
        self.note_unconverted(
            "hook callback",
            "Only a function-name string (or a closure) can be a hook callback.",
            pos.line,
            None,
        );
        self.helpers_used.insert("__unsupported".to_string());
        "__unsupported(\"hook callback is not a function name\")".to_string()
    }
}

fn escape_template_text(text: &str) -> String {
    // Same order as the TS emitter: backslash first, then backtick,
    // dollar, and the control characters.
    text.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

// ---------------------------------------------------------------------------
// Built-in PHP → TypeScript mapping. Returns None when unmapped.
// ---------------------------------------------------------------------------

impl Emitter {
    fn emit_builtin(&mut self, lower: &str, expr: &Expr, ctx: &EmitContext) -> Option<String> {
        let Expr::Call { args, pos, .. } = expr else {
            return None;
        };
        let a: Vec<String> = args.iter().map(|x| self.emit_call_arg(x, ctx)).collect();
        let arg =
            |i: usize| -> String { a.get(i).cloned().unwrap_or_else(|| "undefined".to_string()) };
        match lower {
            "function_exists" => {
                // Resolved statically against the file's function table
                // (collected before emission, including functions
                // hoisted out of `if (!function_exists(...))` guards).
                // A same-file function is always defined in the
                // converted module, so its guard is false and the
                // definition — emitted at module level — stands. Any
                // other name cannot exist in this module's scope.
                match args.first() {
                    Some(Expr::Literal { literal_kind, value, .. })
                        if literal_kind == "string" =>
                    {
                        let name = value.as_str().unwrap_or("");
                        Some(if self.fn_by_php(name).is_some() {
                            "true".to_string()
                        } else {
                            "false".to_string()
                        })
                    }
                    _ => {
                        self.note_unconverted(
                            "function call",
                            "function_exists() with a non-literal argument cannot be resolved statically; treated as not existing.",
                            pos.line,
                            Some("function_exists"),
                        );
                        Some("false".to_string())
                    }
                }
            }
            "explode" => {
                if a.len() == 2 {
                    Some(format!("{}.split({})", arg(1), arg(0)))
                } else {
                    None
                }
            }
            "implode" => {
                if a.len() == 2 {
                    Some(format!("{}.join({})", arg(1), arg(0)))
                } else if a.len() == 1 {
                    Some(format!("{}.join(\"\")", arg(0)))
                } else {
                    None
                }
            }
            "count" => Some(format!(
                "(Array.isArray({0}) ? ({0} as unknown[]).length : Object.keys({0} as Record<string, unknown>).length)",
                arg(0)
            )),
            "substr" => {
                self.helpers_used.insert("__substr".to_string());
                if a.len() == 2 {
                    Some(format!("__substr({}, {})", arg(0), arg(1)))
                } else if a.len() == 3 {
                    Some(format!("__substr({}, {}, {})", arg(0), arg(1), arg(2)))
                } else {
                    None
                }
            }
            "strlen" => {
                self.warnings.push(format!(
                    "strlen() at line {} counts UTF-16 code units in TypeScript, not bytes.",
                    pos.line
                ));
                Some(format!("String({}).length", arg(0)))
            }
            "strtolower" => Some(format!("String({}).toLowerCase()", arg(0))),
            "strtoupper" => Some(format!("String({}).toUpperCase()", arg(0))),
            "trim" => {
                if a.len() == 1 {
                    Some(format!("String({}).trim()", arg(0)))
                } else {
                    None
                }
            }
            "str_replace" => {
                if a.len() == 3 {
                    Some(format!("String({}).replaceAll({}, {})", arg(2), arg(0), arg(1)))
                } else {
                    None
                }
            }
            "str_contains" => Some(format!("String({}).includes({})", arg(0), arg(1))),
            "str_starts_with" => Some(format!("String({}).startsWith({})", arg(0), arg(1))),
            "str_ends_with" => Some(format!("String({}).endsWith({})", arg(0), arg(1))),
            "ucfirst" => Some(format!(
                "(String({0}).charAt(0).toUpperCase() + String({0}).slice(1))",
                arg(0)
            )),
            "lcfirst" => Some(format!(
                "(String({0}).charAt(0).toLowerCase() + String({0}).slice(1))",
                arg(0)
            )),
            "str_repeat" => Some(format!("String({}).repeat(Number({}))", arg(0), arg(1))),
            "in_array" => Some(format!("({} as unknown[]).includes({})", arg(1), arg(0))),
            "array_merge" => {
                self.warnings.push(format!(
                    "array_merge() at line {} was translated with list (spread) semantics; associative-array key overwriting is not preserved.",
                    pos.line
                ));
                let spreads: Vec<String> =
                    a.iter().map(|x| format!("...({x} as unknown[])")).collect();
                Some(format!("[{}]", spreads.join(", ")))
            }
            "array_keys" => Some(format!("Object.keys({} as Record<string, unknown>)", arg(0))),
            "array_values" => Some(format!("Object.values({} as Record<string, unknown>)", arg(0))),
            "array_map" => {
                if a.len() != 2 {
                    return None;
                }
                let fn_arg = &args[0];
                if let Expr::Literal { literal_kind, value, .. } = fn_arg {
                    if literal_kind == "string" {
                        let fn_name = match value {
                            serde_json::Value::String(s) => s.clone(),
                            _ => String::new(),
                        };
                        if let Some(local) = self.fn_by_php(&fn_name) {
                            let ts_name = local.ts_name.clone();
                            return Some(format!(
                                "({} as unknown[]).map((v: unknown) => {ts_name}(wp, v))",
                                arg(1)
                            ));
                        }
                        return match fn_name.to_lowercase().as_str() {
                            "strtolower" => Some(format!(
                                "({} as unknown[]).map((v: unknown) => String(v).toLowerCase())",
                                arg(1)
                            )),
                            "strtoupper" => Some(format!(
                                "({} as unknown[]).map((v: unknown) => String(v).toUpperCase())",
                                arg(1)
                            )),
                            "trim" => Some(format!(
                                "({} as unknown[]).map((v: unknown) => String(v).trim())",
                                arg(1)
                            )),
                            "intval" => Some(format!(
                                "({} as unknown[]).map((v: unknown) => Math.trunc(Number(v)))",
                                arg(1)
                            )),
                            _ => None,
                        };
                    }
                }
                None
            }
            "array_filter" => {
                if a.len() == 1 {
                    Some(format!("({} as unknown[]).filter(Boolean)", arg(0)))
                } else {
                    None
                }
            }
            "is_array" => Some(format!("Array.isArray({})", arg(0))),
            "is_string" => Some(format!("(typeof {} === \"string\")", arg(0))),
            "is_int" | "is_integer" => Some(format!("Number.isInteger({})", arg(0))),
            "is_bool" => Some(format!("(typeof {} === \"boolean\")", arg(0))),
            "is_null" => Some(format!("({} === null)", arg(0))),
            "is_numeric" => Some(format!("!Number.isNaN(Number({}))", arg(0))),
            "intval" => Some(format!("Math.trunc(Number({}))", arg(0))),
            "floatval" => Some(format!("Number({})", arg(0))),
            "strval" => Some(format!("String({})", arg(0))),
            "boolval" => Some(format!("Boolean({})", arg(0))),
            "json_encode" => Some(format!("JSON.stringify({})", arg(0))),
            "json_decode" => Some(format!("JSON.parse(String({}))", arg(0))),
            "mt_rand" | "rand" => {
                self.helpers_used.insert("__rand".to_string());
                if a.len() == 2 {
                    Some(format!("__rand({}, {})", arg(0), arg(1)))
                } else if a.is_empty() {
                    Some("__rand(0, 2147483647)".to_string())
                } else {
                    None
                }
            }
            "time" => Some("Math.floor(Date.now() / 1000)".to_string()),
            "min" => Some(format!("Math.min({})", a.join(", "))),
            "max" => Some(format!("Math.max({})", a.join(", "))),
            "abs" => Some(format!("Math.abs({})", arg(0))),
            "floor" => Some(format!("Math.floor({})", arg(0))),
            "ceil" => Some(format!("Math.ceil({})", arg(0))),
            "round" => Some(format!("Math.round({})", arg(0))),
            "array_push" => Some(format!(
                "({} as unknown[]).push({})",
                arg(0),
                a.iter().skip(1).cloned().collect::<Vec<_>>().join(", ")
            )),
            "array_pop" => Some(format!("({} as unknown[]).pop()", arg(0))),
            "array_shift" => Some(format!("({} as unknown[]).shift()", arg(0))),
            "array_slice" => Some(format!(
                "({} as unknown[]).slice({})",
                arg(0),
                a.iter().skip(1).cloned().collect::<Vec<_>>().join(", ")
            )),
            "array_reverse" => Some(format!("[...({} as unknown[])].reverse()", arg(0))),
            "array_search" => {
                self.warnings.push(format!(
                    "array_search() at line {} returns false when not found (PHP) but -1 in TypeScript; check for -1, not false.",
                    pos.line
                ));
                Some(format!("({} as unknown[]).indexOf({})", arg(1), arg(0)))
            }
            "sort" => Some(format!("({} as unknown[]).sort()", arg(0))),
            "rsort" => Some(format!("({} as unknown[]).sort().reverse()", arg(0))),
            "htmlspecialchars" | "htmlentities" => {
                self.helpers_used.insert("__escHtml".to_string());
                Some(format!("__escHtml(String({}))", arg(0)))
            }
            "defined" => None,
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Statement emission
// ---------------------------------------------------------------------------

impl Emitter {
    fn emit_stmt_list(&mut self, stmts: &[Stmt], ctx: &EmitContext) -> String {
        let mut out: Vec<String> = Vec::new();
        for stmt in stmts {
            let text = self.emit_stmt(stmt, ctx);
            if !text.is_empty() {
                out.push(text);
            }
        }
        out.join("\n")
    }

    fn emit_stmt(&mut self, stmt: &Stmt, ctx: &EmitContext) -> String {
        let pad = indent(ctx.depth);
        match stmt {
            Stmt::Block { body, .. } => {
                let mut sub = ctx.clone();
                sub.depth = ctx.depth + 1;
                format!("{pad}{{\n{}\n{pad}}}", self.emit_stmt_list(body, &sub))
            }
            Stmt::ExprStmt { expr, .. } => {
                if let Expr::Assign { .. } = expr {
                    let assigned = self.emit_assign(expr, ctx);
                    let inner = strip_parens(&assigned);
                    return format!("{pad}{inner};");
                }
                if let Expr::Call {
                    name: Some(name), ..
                } = expr
                {
                    if name.to_lowercase() == "printf" {
                        if ctx.in_function {
                            return format!("{pad}__out += {};", self.emit_expr(expr, ctx));
                        }
                        return format!("{pad}renderedOutput.push({});", self.emit_expr(expr, ctx));
                    }
                }
                format!("{pad}{};", self.emit_expr(expr, ctx))
            }
            Stmt::Return { value, .. } => match value {
                None => format!("{pad}return;"),
                Some(v) => format!("{pad}return {};", self.emit_expr(v, ctx)),
            },
            Stmt::Echo { values, .. } => {
                let mut lines = Vec::new();
                for v in values {
                    let e = self.emit_expr(v, ctx);
                    if ctx.in_function {
                        lines.push(format!("{pad}__out += {e};"));
                    } else {
                        lines.push(format!("{pad}renderedOutput.push({e});"));
                    }
                }
                lines.join("\n")
            }
            Stmt::If { .. } => self.emit_if(stmt, ctx),
            Stmt::Foreach {
                iterable,
                key_var,
                value_var,
                body,
                pos,
                ..
            } => {
                let iter = self.emit_expr(iterable, ctx);
                let mut sub = ctx.clone();
                sub.depth = ctx.depth + 1;
                let body_text = self.emit_stmt_list(body, &sub);
                if let Some(key) = key_var {
                    self.warnings.push(format!(
                        "foreach with key at line {}: keys are strings in the TypeScript translation (Object.entries); PHP integer keys become strings.",
                        pos.line
                    ));
                    format!(
                        "{pad}for ([{key}, {value_var}] of Object.entries({iter} as Record<string, unknown>)) {{\n{body_text}\n{pad}}}"
                    )
                } else {
                    format!(
                        "{pad}for ({value_var} of ({iter} as unknown[])) {{\n{body_text}\n{pad}}}"
                    )
                }
            }
            Stmt::For {
                init,
                test,
                update,
                body,
                ..
            } => {
                let init_text = match init {
                    Some(e) => self.emit_assign_or_expr(e, ctx),
                    None => String::new(),
                };
                let test_text = match test {
                    Some(e) => self.emit_expr(e, ctx),
                    None => String::new(),
                };
                let update_text = match update {
                    Some(e) => self.emit_expr(e, ctx),
                    None => String::new(),
                };
                let mut sub = ctx.clone();
                sub.depth = ctx.depth + 1;
                let body_text = self.emit_stmt_list(body, &sub);
                format!(
                    "{pad}for ({init_text}; {test_text}; {update_text}) {{\n{body_text}\n{pad}}}"
                )
            }
            Stmt::While { test, body, .. } => {
                let mut sub = ctx.clone();
                sub.depth = ctx.depth + 1;
                let body_text = self.emit_stmt_list(body, &sub);
                format!(
                    "{pad}while ({}) {{\n{body_text}\n{pad}}}",
                    self.emit_expr(test, ctx)
                )
            }
            Stmt::DoWhile { body, test, .. } => {
                let mut sub = ctx.clone();
                sub.depth = ctx.depth + 1;
                let body_text = self.emit_stmt_list(body, &sub);
                format!(
                    "{pad}do {{\n{body_text}\n{pad}}} while ({});",
                    self.emit_expr(test, ctx)
                )
            }
            Stmt::StaticVar { .. } => String::new(),
            Stmt::Require {
                mode,
                literal_path,
                pos,
                ..
            } => match literal_path {
                Some(path) => {
                    self.note_unconverted(
                            "require/include",
                            &format!(
                                "Literal {mode} of \"{path}\" is not translated: converted extensions are single modules; move the included code into this file."
                            ),
                            pos.line,
                            Some(mode),
                        );
                    format!("{pad}/* NOT CONVERTED (line {}): {mode} \"{path}\" — see conversion report. */", pos.line)
                }
                None => {
                    self.note_unconverted(
                        "require/include",
                        &format!("{mode} with a non-literal path was blocked by the safety scan."),
                        pos.line,
                        Some(mode),
                    );
                    format!("{pad}/* NOT CONVERTED (line {}): dynamic {mode} — blocked by the sandbox. */", pos.line)
                }
            },
            Stmt::InlineHtml { text, .. } => {
                let json = js_string(text);
                if ctx.in_function {
                    format!("{pad}__out += {json};")
                } else {
                    format!("{pad}renderedOutput.push({json});")
                }
            }
            Stmt::Unsupported {
                label, reason, pos, ..
            } => {
                self.note_unconverted(label, reason, pos.line, None);
                format!(
                    "{pad}/* NOT CONVERTED (line {}): {label} — {reason} */",
                    pos.line
                )
            }
            Stmt::Break { .. } => format!("{pad}break;"),
            Stmt::Continue { .. } => format!("{pad}continue;"),
            Stmt::Global { names, pos } => {
                let joined = names.join(", $");
                self.note_unconverted(
                    "global",
                    &format!(
                        "global ${joined} has no equivalent: Wordless functions receive state through parameters and the wp API."
                    ),
                    pos.line,
                    None,
                );
                format!(
                    "{pad}/* NOT CONVERTED (line {}): global ${joined} — see conversion report. */",
                    pos.line
                )
            }
            Stmt::ConstDecl { name, value, .. } => {
                format!(
                    "{pad}const {} = {};",
                    to_ts_name(name),
                    self.emit_expr(value, ctx)
                )
            }
            Stmt::Switch { pos, .. } => {
                self.note_unconverted(
                    "switch",
                    "switch statements are not translated yet (PHP loose comparison differs from TypeScript); rewrite as if/else.",
                    pos.line,
                    None,
                );
                format!("{pad}/* NOT CONVERTED (line {}): switch — rewrite as if/else. See conversion report. */", pos.line)
            }
            Stmt::Try {
                body,
                catches,
                finally_body,
                ..
            } => {
                let mut sub = ctx.clone();
                sub.depth = ctx.depth + 1;
                let mut out = format!("{pad}try {{\n{}\n{pad}}}", self.emit_stmt_list(body, &sub));
                for c in catches {
                    out.push_str(&format!(
                        " catch {{\n{}\n{pad}}}",
                        self.emit_stmt_list(&c.body, &sub)
                    ));
                }
                if let Some(fb) = finally_body {
                    out.push_str(&format!(
                        " finally {{\n{}\n{pad}}}",
                        self.emit_stmt_list(fb, &sub)
                    ));
                }
                out
            }
            Stmt::Function { .. } | Stmt::Class { .. } | Stmt::Method { .. } => String::new(),
        }
    }

    fn emit_assign_or_expr(&mut self, expr: &Expr, ctx: &EmitContext) -> String {
        if matches!(expr, Expr::Assign { .. }) {
            strip_parens(&self.emit_assign(expr, ctx))
        } else {
            self.emit_expr(expr, ctx)
        }
    }

    /// The `if (!defined('ABSPATH')) { exit; }` guard is a known idiom:
    /// drop it with a note.
    fn is_abspath_guard(stmt: &Stmt) -> bool {
        let Stmt::If {
            test, consequent, ..
        } = stmt
        else {
            return false;
        };
        let Expr::Unary {
            operator, operand, ..
        } = test
        else {
            return false;
        };
        if operator != "!" {
            return false;
        }
        let Expr::Call {
            name: Some(name),
            args,
            ..
        } = operand.as_ref()
        else {
            return false;
        };
        if name.to_lowercase() != "defined" {
            return false;
        }
        let Some(Expr::Literal {
            literal_kind,
            value,
            ..
        }) = args.first()
        else {
            return false;
        };
        if literal_kind != "string" {
            return false;
        }
        let is_abspath = matches!(value, serde_json::Value::String(s) if s == "ABSPATH");
        if !is_abspath {
            return false;
        }
        consequent.iter().all(|s| match s {
            Stmt::Unsupported { label, .. } => label == "exit()" || label == "die()",
            _ => false,
        })
    }

    fn emit_if(&mut self, stmt: &Stmt, ctx: &EmitContext) -> String {
        let Stmt::If {
            test,
            consequent,
            alternate,
            pos,
            ..
        } = stmt
        else {
            return String::new();
        };
        let pad = indent(ctx.depth);
        if Self::is_abspath_guard(stmt) {
            self.transformed.push(Transformed {
                what: "ABSPATH guard".to_string(),
                note: format!(
                    "Line {}: \"if (!defined('ABSPATH')) {{ exit; }}\" was removed — Wordless extensions are never loaded directly, so the guard has no meaning here.",
                    pos.line
                ),
            });
            return format!(
                "{pad}// Wordless: the ABSPATH direct-access guard (line {}) is not needed — extensions load only through the extension API.",
                pos.line
            );
        }
        let mut sub = ctx.clone();
        sub.depth = ctx.depth + 1;
        let mut out = format!(
            "{pad}if ({}) {{\n{}\n{pad}}}",
            self.emit_expr(test, ctx),
            self.emit_stmt_list(consequent, &sub)
        );
        match alternate {
            IfAlternate::None => {}
            IfAlternate::Stmts(stmts) => {
                out.push_str(&format!(
                    " else {{\n{}\n{pad}}}",
                    self.emit_stmt_list(stmts, &sub)
                ));
            }
            IfAlternate::If(inner) => {
                let nested = self.emit_if(inner, ctx);
                out.push_str(&format!(" else {}", nested[pad.len()..].to_string()));
            }
        }
        out
    }

    fn emit_function(&mut self, idx: usize) -> String {
        // Snapshot the function info: emission borrows &mut self.
        let (php_name, ts_name, params, body, is_async, returns_output, has_return_value) = {
            let info = &self.functions[idx];
            (
                info.php_name.clone(),
                info.ts_name.clone(),
                info.params.clone(),
                info.body.clone(),
                info.is_async,
                info.returns_output,
                info.has_return_value,
            )
        };
        let mut declared_vars = HashSet::new();
        self.collect_assigned_vars(&body, &mut declared_vars);
        for p in &params {
            declared_vars.remove(&p.name);
        }
        // Function-local `static` variables persist across calls in PHP,
        // so they are hoisted to module-level lets and every reference is
        // renamed.
        let mut rename_vars: HashMap<String, String> = HashMap::new();
        {
            let fn_name = ts_name.clone();
            let mut statics: Vec<ModuleStatic> = Vec::new();
            for stmt in &body {
                walk_stmt(stmt, &mut |n| {
                    if let Node::Stmt(Stmt::StaticVar { vars, .. }) = n {
                        for v in vars {
                            let hoisted = format!("__static_{fn_name}_{}", v.name);
                            rename_vars.insert(v.name.clone(), hoisted);
                            statics.push(ModuleStatic {
                                fn_name: fn_name.clone(),
                                var_name: v.name.clone(),
                                init: v.value.clone(),
                            });
                            declared_vars.remove(&v.name);
                        }
                    }
                });
            }
            self.module_statics.extend(statics);
        }
        let ctx = EmitContext {
            declared_vars: declared_vars.clone(),
            in_function: true,
            depth: 1,
            rename_vars: Some(rename_vars),
        };
        let params_text = params
            .iter()
            .map(|p| self.emit_param(p))
            .collect::<Vec<_>>()
            .join(", ");
        let async_prefix = if is_async { "async " } else { "" };
        let return_type = if is_async {
            if returns_output || has_return_value {
                ": Promise<string>"
            } else {
                ": Promise<void>"
            }
        } else if returns_output {
            ": string"
        } else if !has_return_value {
            ": void"
        } else {
            ""
        };
        let mut out: Vec<String> = Vec::new();
        out.push(format!(
            "export {async_prefix}function {ts_name}(wp: WordPressApi{}{params_text}){return_type} {{",
            if params_text.is_empty() { "" } else { ", " }
        ));
        if returns_output {
            out.push("  let __out = \"\";".to_string());
        }
        let mut sorted: Vec<String> = declared_vars.iter().cloned().collect();
        sorted.sort();
        for name in sorted {
            out.push(format!("  let {name};"));
        }
        let body_text = self.emit_stmt_list(&body, &ctx);
        if !body_text.is_empty() {
            out.push(body_text);
        }
        if returns_output {
            let last_is_return = matches!(body.last(), Some(Stmt::Return { .. }));
            if !last_is_return {
                out.push("  return __out;".to_string());
            } else if has_return_value {
                self.warnings.push(format!(
                    "Function {php_name}() both echoes output and returns a value; the echoed output is discarded and only the returned value is used."
                ));
            }
        }
        out.push("}".to_string());
        out.join("\n")
    }
}

fn strip_parens(s: &str) -> String {
    if s.starts_with('(') && s.ends_with(')') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_php;

    fn emit_snippet(source: &str) -> String {
        let program = parse_php(source).expect("snippet parses").program;
        emit_typescript(
            &program,
            &EmitOptions {
                filename: "snippet.php".to_string(),
                slug: "snippet".to_string(),
                compat_import: None,
                plugin_header: Default::default(),
                source_label: None,
            },
        )
        .code
    }

    fn emit_report(source: &str) -> EmitResult {
        let program = parse_php(source).expect("snippet parses").program;
        emit_typescript(
            &program,
            &EmitOptions {
                filename: "snippet.php".to_string(),
                slug: "snippet".to_string(),
                compat_import: None,
                plugin_header: Default::default(),
                source_label: None,
            },
        )
    }

    #[test]
    fn maps_functions_hooks_echo_and_printf() {
        let code = emit_snippet(
            r#"<?php
function greet($name) {
  echo "Hello " . $name;
  printf('<p>%s</p>', $name);
}
add_action('init', 'greet');
"#,
        );
        assert!(
            code.contains("export function greet(wp: WordPressApi, name: unknown): string {"),
            "{code}"
        );
        assert!(code.contains("__out += (\"Hello \" + name);"), "{code}");
        assert!(
            code.contains("__out += __sprintf(\"<p>%s</p>\", name);"),
            "{code}"
        );
        // greet() echoes, so the action wrapper captures its output.
        assert!(
            code.contains("wp.add_action(\"init\", () => { renderedOutput.push(greet(wp)); });"),
            "{code}"
        );
        assert!(
            code.contains("export function activate(wp: WordPressApi): void {"),
            "{code}"
        );
        assert!(code.contains("export const extensionMeta"), "{code}");
    }

    #[test]
    fn maps_explode_count_mt_rand_substr() {
        let code = emit_snippet(
            r#"<?php
function pick($text) {
  $lines = explode("\n", $text);
  return $lines[mt_rand(0, count($lines) - 1)];
}
function short($s) {
  return substr($s, 0, 3);
}
"#,
        );
        assert!(code.contains(".split(\"\\n\")"), "{code}");
        assert!(code.contains("__rand(0,"), "{code}");
        assert!(code.contains("Array.isArray"), "{code}");
        assert!(code.contains("__substr("), "{code}");
    }

    #[test]
    fn makes_functions_async_when_they_call_async_wp_functions() {
        let code = emit_snippet(
            r#"<?php
function get_title($id) {
  $post = get_post($id);
  return $post;
}
function show($id) {
  $t = get_title($id);
  echo $t;
}
"#,
        );
        assert!(
            code.contains(
                "export async function getTitle(wp: WordPressApi, id: unknown): Promise<string> {"
            ),
            "{code}"
        );
        assert!(code.contains("await wp.get_post(id)"), "{code}");
        assert!(
            code.contains(
                "export async function show(wp: WordPressApi, id: unknown): Promise<string> {"
            ),
            "{code}"
        );
        assert!(code.contains("await getTitle(wp, id)"), "{code}");
    }

    #[test]
    fn turns_abspath_guard_into_comment_and_records_transformation() {
        let program = parse_php(
            r#"<?php
if ( ! defined( 'ABSPATH' ) ) {
  exit;
}
function f() { return 1; }
"#,
        )
        .expect("parses")
        .program;
        let result = emit_typescript(
            &program,
            &EmitOptions {
                filename: "g.php".to_string(),
                slug: "g".to_string(),
                compat_import: None,
                plugin_header: Default::default(),
                source_label: None,
            },
        );
        assert!(
            result.code.contains("ABSPATH direct-access guard"),
            "{}",
            result.code
        );
        assert!(result
            .report
            .transformed
            .iter()
            .any(|t| t.what == "ABSPATH guard"));
        assert!(!result.code.contains("exit"));
    }

    #[test]
    fn emits_unmapped_calls_as_throwing_helpers_and_lists_them() {
        let result = emit_report(
            r#"<?php
function f() {
  $x = some_unknown_wp_function(1, 2);
  return $x;
}
"#,
        );
        assert!(
            result
                .code
                .contains("__unmapped_some_unknown_wp_function(1, 2)"),
            "{}",
            result.code
        );
        assert!(
            result
                .code
                .contains("function __unmapped_some_unknown_wp_function"),
            "{}",
            result.code
        );
        assert!(result
            .report
            .unconverted
            .iter()
            .any(|u| u.name.as_deref() == Some("some_unknown_wp_function")));
    }

    #[test]
    fn records_classes_as_unconverted_with_comment_in_output() {
        let result = emit_report(
            r#"<?php
class Foo {
  public function bar() { return 1; }
}
"#,
        );
        assert!(result
            .report
            .unconverted
            .iter()
            .any(|u| u.kind == "class declaration" && u.name.as_deref() == Some("Foo")));
        assert!(
            result.code.contains("NOT CONVERTED (line 2): class Foo"),
            "{}",
            result.code
        );
    }

    #[test]
    fn converts_foreach_and_interpolation() {
        let code = emit_snippet(
            r#"<?php
function list_all($items) {
  foreach ($items as $item) {
    echo "Item: $item\n";
  }
}
"#,
        );
        assert!(
            code.contains("for (item of (items as unknown[])) {"),
            "{code}"
        );
        assert!(code.contains("__out += `Item: ${item}\\n`;"), "{code}");
    }

    #[test]
    fn handles_die_guards_the_same_way_as_exit_guards() {
        let a = emit_snippet("<?php if (!defined('ABSPATH')) { exit; } function f() { return 1; }");
        let b =
            emit_snippet("<?php if (!defined('ABSPATH')) { die(); } function f() { return 1; }");
        assert_eq!(a, b);
    }

    #[test]
    fn translates_casts_and_spaceship_operator() {
        let code = emit_snippet(
            r#"<?php
function cmp($a, $b) {
  $x = (int) $a;
  $y = (string) $b;
  return (float) $x <=> (float) $y;
}
"#,
        );
        assert!(code.contains("Math.trunc(Number(a))"), "{code}");
        assert!(code.contains("String(b)"), "{code}");
        assert!(code.contains("? -1 :"), "{code}");
    }

    #[test]
    fn hoists_function_local_static_variables() {
        let code = emit_snippet(
            r#"<?php
function memo() {
  static $cache = null;
  if (is_null($cache)) { $cache = 'v'; }
  return $cache;
}
"#,
        );
        assert!(code.contains("let __static_memo_cache = null;"), "{code}");
        assert!(code.contains("__static_memo_cache = \"v\""), "{code}");
        assert!(code.contains("return __static_memo_cache;"), "{code}");
        // No function-level `let cache;` for the hoisted variable.
        assert!(!code.lines().any(|l| l.trim() == "let cache;"), "{code}");
    }

    #[test]
    fn translates_do_while_and_alternative_syntax_ifs() {
        let code = emit_snippet(
            r#"<?php
function f($items) {
  $i = 0;
  do { $i++; } while ($i < 3);
  if ($items) :
    echo 'yes';
  else :
    echo 'no';
  endif;
  return $i;
}
"#,
        );
        assert!(code.contains("do {"), "{code}");
        assert!(code.contains("while ((i < 3))"), "{code}");
        assert!(code.contains("__out += \"yes\""), "{code}");
        assert!(code.contains("__out += \"no\""), "{code}");
    }
}
