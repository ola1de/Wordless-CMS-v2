//! PHP AST node types — a mirror of the TypeScript converter's `ast.ts`
//! (`wordless-cms/src/conversion/ast.ts`), so the JSON this module produces
//! is structurally comparable with the TypeScript implementation's output.
//!
//! Serialization rules follow the TS shapes exactly:
//! * every node carries `kind` + `pos` (`{line, column, offset}`),
//! * `altSyntax` is only present when true,
//! * `docblock` is always present (always `null` — the TS `findDocblock`
//!   is a stub that returns null),
//! * `if.alternate` is `null`, a statement array, or a nested `if` node.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Pos {
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}

impl Pos {
    pub fn new(line: usize, column: usize, offset: usize) -> Self {
        Pos {
            line,
            column,
            offset,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Param {
    pub kind: &'static str,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub default_value: Option<Expr>,
    pub by_ref: bool,
    pub variadic: bool,
    pub pos: Pos,
}

impl Param {
    pub fn new(
        name: String,
        type_: Option<String>,
        default_value: Option<Expr>,
        by_ref: bool,
        variadic: bool,
        pos: Pos,
    ) -> Self {
        Param {
            kind: "param",
            name,
            type_,
            default_value,
            by_ref,
            variadic,
            pos,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterpPart {
    /// "text" | "var"
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArrayEntry {
    pub key: Option<Expr>,
    pub value: Expr,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatchClause {
    pub var_name: Option<String>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticVarEntry {
    pub name: String,
    pub value: Option<Expr>,
}

/// `if.alternate`: null | Stmt[] | IfNode — untagged so it serializes
/// exactly like the TypeScript union.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum IfAlternate {
    None,
    Stmts(Vec<Stmt>),
    If(Box<Stmt>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
#[serde(rename_all = "snake_case")]
pub enum Stmt {
    #[serde(rename_all = "camelCase")]
    Function {
        name: String,
        params: Vec<Param>,
        return_type: Option<String>,
        body: Vec<Stmt>,
        docblock: Option<String>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    Method {
        name: String,
        visibility: String,
        is_static: bool,
        params: Vec<Param>,
        body: Vec<Stmt>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    Class {
        name: String,
        extends_name: Option<String>,
        implements_names: Vec<String>,
        methods: Vec<Stmt>,
        properties: Vec<String>,
        pos: Pos,
    },
    Block {
        body: Vec<Stmt>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    ExprStmt {
        expr: Expr,
        pos: Pos,
    },
    Return {
        value: Option<Expr>,
        pos: Pos,
    },
    Echo {
        values: Vec<Expr>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    If {
        test: Expr,
        consequent: Vec<Stmt>,
        alternate: IfAlternate,
        #[serde(skip_serializing_if = "Option::is_none")]
        alt_syntax: Option<bool>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    Foreach {
        iterable: Expr,
        key_var: Option<String>,
        value_var: String,
        body: Vec<Stmt>,
        #[serde(skip_serializing_if = "Option::is_none")]
        alt_syntax: Option<bool>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    For {
        init: Option<Expr>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Vec<Stmt>,
        #[serde(skip_serializing_if = "Option::is_none")]
        alt_syntax: Option<bool>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    While {
        test: Expr,
        body: Vec<Stmt>,
        #[serde(skip_serializing_if = "Option::is_none")]
        alt_syntax: Option<bool>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    DoWhile {
        body: Vec<Stmt>,
        test: Expr,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    Require {
        mode: String,
        literal_path: Option<String>,
        arg: Expr,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    InlineHtml {
        text: String,
        pos: Pos,
    },
    Unsupported {
        label: String,
        reason: String,
        source: String,
        pos: Pos,
    },
    Break {
        pos: Pos,
    },
    Continue {
        pos: Pos,
    },
    Global {
        names: Vec<String>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    ConstDecl {
        name: String,
        value: Expr,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    Switch {
        discriminant: Expr,
        #[serde(skip_serializing_if = "Option::is_none")]
        alt_syntax: Option<bool>,
        source: String,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    Try {
        body: Vec<Stmt>,
        catches: Vec<CatchClause>,
        finally_body: Option<Vec<Stmt>>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    StaticVar {
        vars: Vec<StaticVarEntry>,
        pos: Pos,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
#[serde(rename_all = "snake_case")]
pub enum Expr {
    #[serde(rename_all = "camelCase")]
    Literal {
        literal_kind: String,
        value: serde_json::Value,
        pos: Pos,
    },
    Var {
        name: String,
        pos: Pos,
    },
    Array {
        entries: Vec<ArrayEntry>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    ArrayAccess {
        object: Box<Expr>,
        index: Option<Box<Expr>>,
        append: bool,
        pos: Pos,
    },
    Binary {
        operator: String,
        left: Box<Expr>,
        right: Box<Expr>,
        pos: Pos,
    },
    Unary {
        operator: String,
        operand: Box<Expr>,
        prefix: bool,
        pos: Pos,
    },
    Ternary {
        test: Box<Expr>,
        consequent: Option<Box<Expr>>,
        alternate: Box<Expr>,
        pos: Pos,
    },
    Call {
        name: Option<String>,
        callee: Option<Box<Expr>>,
        args: Vec<Expr>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    MethodCall {
        object: Box<Expr>,
        method: String,
        args: Vec<Expr>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    StaticCall {
        class_name: String,
        member: String,
        args: Vec<Expr>,
        is_constant: bool,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    PropertyFetch {
        object: Box<Expr>,
        property: String,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    ConstFetch {
        name: String,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    New {
        class_name: String,
        args: Vec<Expr>,
        pos: Pos,
    },
    Closure {
        params: Vec<Param>,
        uses: Vec<String>,
        body: Vec<Stmt>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    InterpolatedString {
        parts: Vec<InterpPart>,
        pos: Pos,
    },
    Assign {
        operator: String,
        target: Box<Expr>,
        value: Box<Expr>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    Cast {
        cast_type: String,
        operand: Box<Expr>,
        pos: Pos,
    },
    /// Extension beyond the TS parser (see README): a comma-separated
    /// expression list, e.g. the init slot of
    /// `for ($i = 1, $c = count($a); ...)`. The TS AST has no node for this.
    #[serde(rename_all = "camelCase")]
    Sequence {
        exprs: Vec<Expr>,
        pos: Pos,
    },
    #[serde(rename_all = "camelCase")]
    UnsupportedExpr {
        label: String,
        reason: String,
        source: String,
        pos: Pos,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Program {
    pub kind: &'static str,
    pub body: Vec<Stmt>,
    pub pos: Pos,
}

impl Program {
    pub fn new(body: Vec<Stmt>) -> Self {
        Program {
            kind: "program",
            body,
            pos: Pos::new(1, 1, 0),
        }
    }
}

// ---------------------------------------------------------------------------
// Node counting — mirrors `childNodes` / `countAstNodes` in the TS ast.ts,
// so the Rust `count_nodes` export and the TS `countAstNodes` agree.
// ---------------------------------------------------------------------------

fn stmt_children(s: &Stmt) -> Vec<Node<'_>> {
    use Stmt::*;
    match s {
        Function { params, body, .. } => {
            let mut v: Vec<Node> = params
                .iter()
                .filter_map(|p| p.default_value.as_ref().map(Node::Expr))
                .collect();
            v.extend(body.iter().map(Node::Stmt));
            v
        }
        Method { params, body, .. } => {
            let mut v: Vec<Node> = params
                .iter()
                .filter_map(|p| p.default_value.as_ref().map(Node::Expr))
                .collect();
            v.extend(body.iter().map(Node::Stmt));
            v
        }
        Class { methods, .. } => methods.iter().map(Node::Stmt).collect(),
        Block { body, .. } => body.iter().map(Node::Stmt).collect(),
        ExprStmt { expr, .. } => vec![Node::Expr(expr)],
        Return { value, .. } => value.iter().map(Node::Expr).collect(),
        Echo { values, .. } => values.iter().map(Node::Expr).collect(),
        If {
            test,
            consequent,
            alternate,
            ..
        } => {
            let mut v = vec![Node::Expr(test)];
            v.extend(consequent.iter().map(Node::Stmt));
            match alternate {
                IfAlternate::None => {}
                IfAlternate::Stmts(stmts) => v.extend(stmts.iter().map(Node::Stmt)),
                IfAlternate::If(inner) => v.push(Node::Stmt(inner)),
            }
            v
        }
        Foreach { iterable, body, .. } => {
            let mut v = vec![Node::Expr(iterable)];
            v.extend(body.iter().map(Node::Stmt));
            v
        }
        For {
            init,
            test,
            update,
            body,
            ..
        } => {
            let mut v: Vec<Node> = vec![];
            if let Some(e) = init {
                v.push(Node::Expr(e));
            }
            if let Some(e) = test {
                v.push(Node::Expr(e));
            }
            if let Some(e) = update {
                v.push(Node::Expr(e));
            }
            v.extend(body.iter().map(Node::Stmt));
            v
        }
        While { test, body, .. } => {
            let mut v = vec![Node::Expr(test)];
            v.extend(body.iter().map(Node::Stmt));
            v
        }
        DoWhile { body, test, .. } => {
            let mut v: Vec<Node> = body.iter().map(Node::Stmt).collect();
            v.push(Node::Expr(test));
            v
        }
        Require { arg, .. } => vec![Node::Expr(arg)],
        Try {
            body,
            catches,
            finally_body,
            ..
        } => {
            let mut v: Vec<Node> = body.iter().map(Node::Stmt).collect();
            for c in catches {
                v.extend(c.body.iter().map(Node::Stmt));
            }
            if let Some(fb) = finally_body {
                v.extend(fb.iter().map(Node::Stmt));
            }
            v
        }
        StaticVar { vars, .. } => vars
            .iter()
            .filter_map(|e| e.value.as_ref().map(Node::Expr))
            .collect(),
        ConstDecl { value, .. } => vec![Node::Expr(value)],
        _ => vec![],
    }
}

fn expr_children(e: &Expr) -> Vec<Node<'_>> {
    use Expr::*;
    match e {
        Array { entries, .. } => entries
            .iter()
            .flat_map(|en| {
                let mut v = vec![];
                if let Some(k) = &en.key {
                    v.push(Node::Expr(k));
                }
                v.push(Node::Expr(&en.value));
                v
            })
            .collect(),
        ArrayAccess { object, index, .. } => {
            let mut v = vec![Node::Expr(object)];
            if let Some(i) = index {
                v.push(Node::Expr(i));
            }
            v
        }
        Binary { left, right, .. } => vec![Node::Expr(left), Node::Expr(right)],
        Unary { operand, .. } => vec![Node::Expr(operand)],
        Ternary {
            test,
            consequent,
            alternate,
            ..
        } => {
            let mut v = vec![Node::Expr(test)];
            if let Some(c) = consequent {
                v.push(Node::Expr(c));
            }
            v.push(Node::Expr(alternate));
            v
        }
        Call { callee, args, .. } => {
            let mut v: Vec<Node> = vec![];
            if let Some(c) = callee {
                v.push(Node::Expr(c));
            }
            v.extend(args.iter().map(Node::Expr));
            v
        }
        MethodCall { object, args, .. } => {
            let mut v = vec![Node::Expr(object)];
            v.extend(args.iter().map(Node::Expr));
            v
        }
        StaticCall { args, .. } => args.iter().map(Node::Expr).collect(),
        PropertyFetch { object, .. } => vec![Node::Expr(object)],
        New { args, .. } => args.iter().map(Node::Expr).collect(),
        Closure { params, body, .. } => {
            let mut v: Vec<Node> = params
                .iter()
                .filter_map(|p| p.default_value.as_ref().map(Node::Expr))
                .collect();
            v.extend(body.iter().map(Node::Stmt));
            v
        }
        Assign { target, value, .. } => vec![Node::Expr(target), Node::Expr(value)],
        Cast { operand, .. } => vec![Node::Expr(operand)],
        Sequence { exprs, .. } => exprs.iter().map(Node::Expr).collect(),
        _ => vec![],
    }
}

/// A borrowed AST node — either a statement or an expression.
/// Public so walkers (emitter, analyser) can visit both kinds with one
/// callback, mirroring the TypeScript `walkAst` visitor.
#[derive(Debug, Clone, Copy)]
pub enum Node<'a> {
    Stmt(&'a Stmt),
    Expr(&'a Expr),
}

impl<'a> Node<'a> {
    /// Position of the node (every node carries `pos`).
    pub fn pos(self) -> Pos {
        match self {
            Node::Stmt(s) => s.pos(),
            Node::Expr(e) => e.pos(),
        }
    }

    /// Node kind string, as serialized (`"function"`, `"call"`, …).
    pub fn kind(self) -> &'static str {
        match self {
            Node::Stmt(s) => match s {
                Stmt::Function { .. } => "function",
                Stmt::Method { .. } => "method",
                Stmt::Class { .. } => "class",
                Stmt::Block { .. } => "block",
                Stmt::ExprStmt { .. } => "expr_stmt",
                Stmt::Return { .. } => "return",
                Stmt::Echo { .. } => "echo",
                Stmt::If { .. } => "if",
                Stmt::Foreach { .. } => "foreach",
                Stmt::For { .. } => "for",
                Stmt::While { .. } => "while",
                Stmt::DoWhile { .. } => "do_while",
                Stmt::Require { .. } => "require",
                Stmt::InlineHtml { .. } => "inline_html",
                Stmt::Unsupported { .. } => "unsupported",
                Stmt::Break { .. } => "break",
                Stmt::Continue { .. } => "continue",
                Stmt::Global { .. } => "global",
                Stmt::ConstDecl { .. } => "const_decl",
                Stmt::Switch { .. } => "switch",
                Stmt::Try { .. } => "try",
                Stmt::StaticVar { .. } => "static_var",
            },
            Node::Expr(e) => match e {
                Expr::Literal { .. } => "literal",
                Expr::Var { .. } => "var",
                Expr::Array { .. } => "array",
                Expr::ArrayAccess { .. } => "array_access",
                Expr::Binary { .. } => "binary",
                Expr::Unary { .. } => "unary",
                Expr::Ternary { .. } => "ternary",
                Expr::Call { .. } => "call",
                Expr::MethodCall { .. } => "method_call",
                Expr::StaticCall { .. } => "static_call",
                Expr::PropertyFetch { .. } => "property_fetch",
                Expr::ConstFetch { .. } => "const_fetch",
                Expr::New { .. } => "new",
                Expr::Closure { .. } => "closure",
                Expr::InterpolatedString { .. } => "interpolated_string",
                Expr::Assign { .. } => "assign",
                Expr::Cast { .. } => "cast",
                Expr::Sequence { .. } => "sequence",
                Expr::UnsupportedExpr { .. } => "unsupported_expr",
            },
        }
    }
}

impl Stmt {
    /// Position of this statement.
    pub fn pos(&self) -> Pos {
        use Stmt::*;
        match self {
            Function { pos, .. }
            | Method { pos, .. }
            | Class { pos, .. }
            | Block { pos, .. }
            | ExprStmt { pos, .. }
            | Return { pos, .. }
            | Echo { pos, .. }
            | If { pos, .. }
            | Foreach { pos, .. }
            | For { pos, .. }
            | While { pos, .. }
            | DoWhile { pos, .. }
            | Require { pos, .. }
            | InlineHtml { pos, .. }
            | Unsupported { pos, .. }
            | Break { pos, .. }
            | Continue { pos, .. }
            | Global { pos, .. }
            | ConstDecl { pos, .. }
            | Switch { pos, .. }
            | Try { pos, .. }
            | StaticVar { pos, .. } => *pos,
        }
    }
}

impl Expr {
    /// Position of this expression.
    pub fn pos(&self) -> Pos {
        use Expr::*;
        match self {
            Literal { pos, .. }
            | Var { pos, .. }
            | Array { pos, .. }
            | ArrayAccess { pos, .. }
            | Binary { pos, .. }
            | Unary { pos, .. }
            | Ternary { pos, .. }
            | Call { pos, .. }
            | MethodCall { pos, .. }
            | StaticCall { pos, .. }
            | PropertyFetch { pos, .. }
            | ConstFetch { pos, .. }
            | New { pos, .. }
            | Closure { pos, .. }
            | InterpolatedString { pos, .. }
            | Assign { pos, .. }
            | Cast { pos, .. }
            | Sequence { pos, .. }
            | UnsupportedExpr { pos, .. } => *pos,
        }
    }
}

/// Depth-first pre-order walk of a whole program, invoking `visit` for
/// every node (program node itself excluded — same as TS `walkAst`,
/// which visits the program node but no caller matches on it).
/// Children are visited in `childNodes` order.
pub fn walk_program(program: &Program, visit: &mut dyn FnMut(Node<'_>)) {
    let mut stack: Vec<Node> = program.body.iter().map(Node::Stmt).collect();
    walk_stack(&mut stack, visit);
}

/// Depth-first pre-order walk rooted at one statement.
pub fn walk_stmt(stmt: &Stmt, visit: &mut dyn FnMut(Node<'_>)) {
    let mut stack: Vec<Node> = vec![Node::Stmt(stmt)];
    walk_stack(&mut stack, visit);
}

/// Depth-first pre-order walk rooted at one expression.
pub fn walk_expr(expr: &Expr, visit: &mut dyn FnMut(Node<'_>)) {
    let mut stack: Vec<Node> = vec![Node::Expr(expr)];
    walk_stack(&mut stack, visit);
}

fn walk_stack(stack: &mut Vec<Node<'_>>, visit: &mut dyn FnMut(Node<'_>)) {
    while let Some(node) = stack.pop() {
        visit(node);
        let kids: Vec<Node> = match node {
            Node::Stmt(s) => stmt_children(s),
            Node::Expr(e) => expr_children(e),
        };
        for k in kids.into_iter().rev() {
            stack.push(k);
        }
    }
}

/// Total AST node count (program node itself excluded), mirroring TS
/// `countAstNodes`.
pub fn count_ast_nodes(program: &Program) -> usize {
    let mut count = 0usize;
    let mut stack: Vec<Node> = program.body.iter().map(Node::Stmt).collect();
    while let Some(node) = stack.pop() {
        match node {
            Node::Stmt(s) => {
                count += 1;
                let kids = stmt_children(s);
                for k in kids.into_iter().rev() {
                    stack.push(k);
                }
            }
            Node::Expr(e) => {
                count += 1;
                let kids = expr_children(e);
                for k in kids.into_iter().rev() {
                    stack.push(k);
                }
            }
        }
    }
    count
}
