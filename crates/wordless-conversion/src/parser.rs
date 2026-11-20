//! PHP recursive-descent parser — a mirror of the TypeScript converter's
//! `parser.ts` (`wordless-cms/src/conversion/parser.ts`).
//!
//! Produces the same AST node kinds and shapes as the TypeScript parser so
//! benchmark comparisons are apples-to-apples. Known deliberate deviation:
//! the TS parser has a latent bug with the `xor` operator (its precedence
//! lookup is `undefined`, which makes the precedence-climbing loop behave
//! unpredictably); here `xor` is given precedence 2, the same as `or`.
//! See README.md.

use crate::ast::*;
use crate::lexer::{lex_php, LexComment, Token, TokenKind};

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub pos: Pos,
}

impl ParseError {
    fn new(message: impl Into<String>, pos: Pos) -> Self {
        ParseError {
            message: message.into(),
            pos,
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (line {}, column {})",
            self.message, self.pos.line, self.pos.column
        )
    }
}

pub struct ParseResult {
    pub program: Program,
    pub comments: Vec<LexComment>,
    pub tokens: Vec<Token>,
}

pub fn parse_php(source: &str) -> Result<ParseResult, ParseError> {
    let lexed = lex_php(source).map_err(|e| ParseError::new(e.message, e.pos))?;
    let mut parser = Parser {
        tokens: lexed.tokens,
        pos: 0,
        source: source.to_string(),
    };
    let program = parser.parse_program()?;
    Ok(ParseResult {
        program,
        comments: lexed.comments,
        tokens: parser.tokens,
    })
}

const ASSIGN_OPS: &[&str] = &[
    "=", ".=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>=", "**=", "??=",
];

const CAST_TYPES: &[&str] = &[
    "int", "integer", "float", "double", "real", "string", "bool", "boolean", "array", "object",
    "unset", "binary",
];

fn binary_precedence(op: &str) -> Option<u32> {
    Some(match op {
        "??" => 1,
        "||" | "or" | "xor" => 2,
        "&&" | "and" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "==" | "!=" | "===" | "!==" => 7,
        "<" | ">" | "<=" | ">=" | "<=>" => 8,
        "+" | "-" | "." => 9,
        "*" | "/" | "%" => 10,
        "**" => 11,
        // Extension beyond the TS parser (see README): PHP `instanceof`
        // binds tighter than arithmetic — precedence 12, above `**`.
        // Emitted as an ordinary `binary` node with operator "instanceof".
        "instanceof" => 12,
        _ => return None,
    })
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    source: String,
}

impl Parser {
    fn peek(&self, offset: usize) -> &Token {
        let idx = (self.pos + offset).min(self.tokens.len() - 1);
        &self.tokens[idx]
    }

    fn next(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn at_eof(&self) -> bool {
        self.peek(0).kind == TokenKind::Eof
    }

    fn is_punct(&self, value: &str, offset: usize) -> bool {
        let t = self.peek(offset);
        t.kind == TokenKind::Punct && t.value == value
    }

    fn is_ident(&self, value: &str, offset: usize) -> bool {
        let t = self.peek(offset);
        t.kind == TokenKind::Ident && t.value.to_lowercase() == value.to_lowercase()
    }

    fn match_punct(&mut self, value: &str) -> bool {
        if self.is_punct(value, 0) {
            self.next();
            true
        } else {
            false
        }
    }

    fn match_ident(&mut self, value: &str) -> bool {
        if self.is_ident(value, 0) {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, value: &str) -> Result<Token, ParseError> {
        if !self.is_punct(value, 0) {
            let t = self.peek(0).clone();
            return Err(ParseError::new(
                format!("Expected \"{}\" but found {}", value, describe_token(&t)),
                t.pos,
            ));
        }
        Ok(self.next())
    }

    fn expect_ident(&mut self, what: &str) -> Result<Token, ParseError> {
        let t = self.peek(0).clone();
        if t.kind != TokenKind::Ident {
            return Err(ParseError::new(
                format!("Expected {what} but found {}", describe_token(&t)),
                t.pos,
            ));
        }
        Ok(self.next())
    }

    fn error(&self, message: impl Into<String>, pos: Pos) -> ParseError {
        ParseError::new(message, pos)
    }

    fn slice_source(&self, from: Pos, to: Pos) -> String {
        self.source
            .get(from.offset..to.offset)
            .unwrap_or("")
            .to_string()
    }

    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut body: Vec<Stmt> = Vec::new();
        while !self.at_eof() {
            let tok = self.peek(0).clone();
            if tok.kind == TokenKind::OpenTag {
                self.next();
                if tok.short_echo {
                    let expr = self.parse_expression()?;
                    body.push(Stmt::Echo {
                        values: vec![expr],
                        pos: tok.pos,
                    });
                    if self.peek(0).kind == TokenKind::CloseTag {
                        self.next();
                    }
                    continue;
                }
                body.extend(self.parse_statement_list(false)?);
                continue;
            }
            if tok.kind == TokenKind::InlineHtml {
                self.next();
                body.push(Stmt::InlineHtml {
                    text: tok.value,
                    pos: tok.pos,
                });
                continue;
            }
            if tok.kind == TokenKind::CloseTag {
                self.next();
                continue;
            }
            self.next();
        }
        Ok(Program::new(body))
    }

    fn parse_statement_list(&mut self, in_block: bool) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts: Vec<Stmt> = Vec::new();
        while !self.at_eof() {
            let tok = self.peek(0).clone();
            if tok.kind == TokenKind::CloseTag {
                self.next();
                if in_block {
                    continue;
                }
                break;
            }
            if tok.kind == TokenKind::InlineHtml {
                self.next();
                stmts.push(Stmt::InlineHtml {
                    text: tok.value,
                    pos: tok.pos,
                });
                continue;
            }
            if tok.kind == TokenKind::OpenTag {
                self.next();
                if tok.short_echo {
                    let expr = self.parse_expression()?;
                    stmts.push(Stmt::Echo {
                        values: vec![expr],
                        pos: tok.pos,
                    });
                    if self.peek(0).kind == TokenKind::CloseTag {
                        self.next();
                    }
                }
                continue;
            }
            if self.is_punct("}", 0) {
                break;
            }
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            }
        }
        Ok(stmts)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect_punct("{")?;
        let body = self.parse_statement_list(true)?;
        self.expect_punct("}")?;
        Ok(body)
    }

    fn parse_statement(&mut self) -> Result<Option<Stmt>, ParseError> {
        let tok = self.peek(0).clone();
        if self.is_punct(";", 0) {
            self.next();
            return Ok(None);
        }
        if self.is_punct("{", 0) {
            let pos = tok.pos;
            let body = self.parse_block()?;
            return Ok(Some(Stmt::Block { body, pos }));
        }
        if tok.kind == TokenKind::Ident {
            let kw = tok.value.to_lowercase();
            match kw.as_str() {
                "function" => return Ok(Some(self.parse_function_decl()?)),
                "class" | "abstract" | "final" => return Ok(Some(self.parse_class_decl()?)),
                "interface" | "trait" | "enum" => {
                    let start = self.next().pos;
                    let from = start;
                    let mut depth = 0i32;
                    while !self.at_eof() {
                        let t = self.next();
                        if t.kind == TokenKind::Punct && t.value == "{" {
                            depth += 1;
                        }
                        if t.kind == TokenKind::Punct && t.value == "}" {
                            depth -= 1;
                            if depth <= 0 {
                                break;
                            }
                        }
                        if t.kind == TokenKind::CloseTag || t.kind == TokenKind::Eof {
                            break;
                        }
                    }
                    return Ok(Some(Stmt::Unsupported {
                        label: format!("{kw} declaration"),
                        reason: format!(
                            "Wordless has no {kw} system; the declaration is analysed but not converted."
                        ),
                        source: self.slice_source(from, self.peek(0).pos),
                        pos: start,
                    }));
                }
                "if" => return Ok(Some(self.parse_if()?)),
                "foreach" => return Ok(Some(self.parse_foreach()?)),
                "for" => return Ok(Some(self.parse_for()?)),
                "while" => return Ok(Some(self.parse_while()?)),
                "do" => {
                    self.next();
                    let body_stmt = self.parse_statement()?;
                    let body = match body_stmt {
                        Some(Stmt::Block { body, .. }) => body,
                        Some(s) => vec![s],
                        None => vec![],
                    };
                    self.expect_ident("while")?;
                    self.expect_punct("(")?;
                    let test = self.parse_expression()?;
                    self.expect_punct(")")?;
                    self.expect_punct(";")?;
                    return Ok(Some(Stmt::DoWhile {
                        body,
                        test,
                        pos: tok.pos,
                    }));
                }
                "static" => {
                    if self.peek(1).kind != TokenKind::Var {
                        // static::method() / static function () {} — expression.
                    } else {
                        self.next();
                        let mut vars: Vec<StaticVarEntry> = Vec::new();
                        while self.peek(0).kind == TokenKind::Var {
                            let name = self.next().value;
                            let value = if self.match_punct("=") {
                                Some(self.parse_expression()?)
                            } else {
                                None
                            };
                            vars.push(StaticVarEntry { name, value });
                            if !self.match_punct(",") {
                                break;
                            }
                        }
                        self.expect_punct(";")?;
                        return Ok(Some(Stmt::StaticVar { vars, pos: tok.pos }));
                    }
                }
                "switch" => return Ok(Some(self.parse_switch()?)),
                "try" => return Ok(Some(self.parse_try()?)),
                "return" => {
                    self.next();
                    let mut value: Option<Expr> = None;
                    if !self.is_punct(";", 0) {
                        value = Some(self.parse_expression()?);
                    }
                    self.expect_punct(";")?;
                    return Ok(Some(Stmt::Return {
                        value,
                        pos: tok.pos,
                    }));
                }
                "echo" | "print" => {
                    self.next();
                    let mut values: Vec<Expr> = vec![self.parse_expression()?];
                    while self.match_punct(",") {
                        values.push(self.parse_expression()?);
                    }
                    self.expect_punct(";")?;
                    return Ok(Some(Stmt::Echo {
                        values,
                        pos: tok.pos,
                    }));
                }
                "require" | "include" | "require_once" | "include_once" => {
                    self.next();
                    let has_paren = self.match_punct("(");
                    let arg = self.parse_expression()?;
                    if has_paren {
                        self.expect_punct(")")?;
                    }
                    self.expect_punct(";")?;
                    let literal_path = match &arg {
                        Expr::Literal {
                            literal_kind,
                            value,
                            ..
                        } if literal_kind == "string" => value.as_str().map(|s| s.to_string()),
                        _ => None,
                    };
                    return Ok(Some(Stmt::Require {
                        mode: kw,
                        literal_path,
                        arg,
                        pos: tok.pos,
                    }));
                }
                "global" => {
                    self.next();
                    let mut names: Vec<String> = Vec::new();
                    loop {
                        let v = self.peek(0).clone();
                        if v.kind != TokenKind::Var {
                            return Err(self.error("Expected a variable after global", v.pos));
                        }
                        self.next();
                        names.push(v.value);
                        if !self.match_punct(",") {
                            break;
                        }
                    }
                    self.expect_punct(";")?;
                    return Ok(Some(Stmt::Global {
                        names,
                        pos: tok.pos,
                    }));
                }
                "const" => {
                    self.next();
                    let name_tok = self.expect_ident("constant name")?;
                    self.expect_punct("=")?;
                    let value = self.parse_expression()?;
                    self.expect_punct(";")?;
                    return Ok(Some(Stmt::ConstDecl {
                        name: name_tok.value,
                        value,
                        pos: tok.pos,
                    }));
                }
                "break" => {
                    self.next();
                    if !self.is_punct(";", 0) {
                        self.parse_expression()?;
                    }
                    self.expect_punct(";")?;
                    return Ok(Some(Stmt::Break { pos: tok.pos }));
                }
                "continue" => {
                    self.next();
                    if !self.is_punct(";", 0) {
                        self.parse_expression()?;
                    }
                    self.expect_punct(";")?;
                    return Ok(Some(Stmt::Continue { pos: tok.pos }));
                }
                "exit" | "die" => {
                    self.next();
                    if self.match_punct("(") {
                        if !self.is_punct(")", 0) {
                            self.parse_expression()?;
                        }
                        self.expect_punct(")")?;
                    }
                    self.expect_punct(";")?;
                    return Ok(Some(Stmt::Unsupported {
                        label: format!("{kw}()"),
                        reason: "Terminating the request has no equivalent in a Wordless extension; the call site must be rewritten by hand.".to_string(),
                        source: kw,
                        pos: tok.pos,
                    }));
                }
                "unset" => {
                    self.next();
                    self.expect_punct("(")?;
                    while !self.is_punct(")", 0) && !self.at_eof() {
                        self.next();
                    }
                    self.expect_punct(")")?;
                    self.expect_punct(";")?;
                    return Ok(Some(Stmt::Unsupported {
                        label: "unset()".to_string(),
                        reason: "unset() is not translated; assign null instead.".to_string(),
                        source: "unset(...)".to_string(),
                        pos: tok.pos,
                    }));
                }
                "declare" | "namespace" | "use" => {
                    let start = self.next().pos;
                    while !self.is_punct(";", 0) && !self.is_punct("{", 0) && !self.at_eof() {
                        self.next();
                    }
                    if self.is_punct("{", 0) {
                        let mut depth = 0i32;
                        while !self.at_eof() {
                            let t = self.next();
                            if t.kind == TokenKind::Punct && t.value == "{" {
                                depth += 1;
                            }
                            if t.kind == TokenKind::Punct && t.value == "}" {
                                depth -= 1;
                                if depth <= 0 {
                                    break;
                                }
                            }
                        }
                    } else {
                        self.expect_punct(";")?;
                    }
                    return Ok(Some(Stmt::Unsupported {
                        label: format!("{kw} statement"),
                        reason: format!(
                            "The {kw} statement is not part of the convertible PHP subset."
                        ),
                        source: kw,
                        pos: start,
                    }));
                }
                _ => {}
            }
        }
        // Expression statement.
        let expr = self.parse_expression()?;
        let pos = expr_pos(&expr);
        self.expect_punct(";")?;
        Ok(Some(Stmt::ExprStmt { expr, pos }))
    }

    fn parse_function_decl(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect_ident("function")?.pos;
        if self.is_punct("(", 0) {
            let closure = self.parse_closure(start)?;
            return Err(self.error(
                "Unexpected anonymous function as a statement",
                closure_pos(&closure),
            ));
        }
        let name_tok = self.expect_ident("function name")?;
        let params = self.parse_params()?;
        let mut return_type: Option<String> = None;
        if self.match_punct(":") {
            return_type = Some(self.parse_type_name()?);
        }
        let body = self.parse_block()?;
        Ok(Stmt::Function {
            name: name_tok.value,
            params,
            return_type,
            body,
            docblock: None,
            pos: start,
        })
    }

    fn parse_type_name(&mut self) -> Result<String, ParseError> {
        let mut name = String::new();
        if self.match_punct("?") {
            name.push('?');
        }
        let first = self.peek(0).clone();
        if first.kind == TokenKind::Ident {
            name.push_str(&self.next().value);
            while self.is_punct("\\", 0) {
                self.next();
                name.push('\\');
                name.push_str(&self.expect_ident("namespace segment")?.value);
            }
        } else if self.is_punct("|", 0) {
            name.push('|');
        }
        while self.is_punct("|", 0) {
            self.next();
            name.push('|');
            name.push_str(&self.parse_type_name()?);
        }
        Ok(name)
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect_punct("(")?;
        let mut params: Vec<Param> = Vec::new();
        while !self.is_punct(")", 0) && !self.at_eof() {
            let start = self.peek(0).pos;
            while self.is_ident("public", 0)
                || self.is_ident("private", 0)
                || self.is_ident("protected", 0)
                || self.is_ident("readonly", 0)
            {
                self.next();
            }
            let mut type_: Option<String> = None;
            if self.peek(0).kind == TokenKind::Ident || self.is_punct("?", 0) {
                let save = self.pos;
                match self.parse_type_name() {
                    Ok(t) => {
                        if self.peek(0).kind != TokenKind::Var
                            && !self.is_punct("&", 0)
                            && !self.is_punct("...", 0)
                        {
                            type_ = None;
                            self.pos = save;
                        } else {
                            type_ = Some(t);
                        }
                    }
                    Err(_) => {
                        type_ = None;
                        self.pos = save;
                    }
                }
            }
            let by_ref = self.match_punct("&");
            let variadic = self.match_punct("...");
            let var_tok = self.peek(0).clone();
            if var_tok.kind != TokenKind::Var {
                return Err(self.error("Expected a parameter variable", var_tok.pos));
            }
            self.next();
            let mut default_value: Option<Expr> = None;
            if self.match_punct("=") {
                default_value = Some(self.parse_expression()?);
            }
            params.push(Param::new(
                var_tok.value,
                type_,
                default_value,
                by_ref,
                variadic,
                start,
            ));
            if !self.match_punct(",") {
                break;
            }
        }
        self.expect_punct(")")?;
        Ok(params)
    }
}

fn describe_token(t: &Token) -> String {
    if t.kind == TokenKind::Eof {
        return "end of file".to_string();
    }
    serde_json::to_string(&t.value).unwrap_or_else(|_| format!("\"{}\"", t.value))
}

/// Helpers to read a position out of an Expr without moving it.
fn expr_pos(e: &Expr) -> Pos {
    match e {
        Expr::Literal { pos, .. }
        | Expr::Var { pos, .. }
        | Expr::Array { pos, .. }
        | Expr::ArrayAccess { pos, .. }
        | Expr::Binary { pos, .. }
        | Expr::Unary { pos, .. }
        | Expr::Ternary { pos, .. }
        | Expr::Call { pos, .. }
        | Expr::MethodCall { pos, .. }
        | Expr::StaticCall { pos, .. }
        | Expr::PropertyFetch { pos, .. }
        | Expr::ConstFetch { pos, .. }
        | Expr::New { pos, .. }
        | Expr::Closure { pos, .. }
        | Expr::InterpolatedString { pos, .. }
        | Expr::Assign { pos, .. }
        | Expr::Cast { pos, .. }
        | Expr::Sequence { pos, .. }
        | Expr::UnsupportedExpr { pos, .. } => *pos,
    }
}

fn closure_pos(c: &Expr) -> Pos {
    expr_pos(c)
}

impl Parser {
    fn parse_class_decl(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek(0).pos;
        if self.is_ident("abstract", 0) {
            self.next();
        }
        if self.is_ident("final", 0) {
            self.next();
        }
        self.expect_ident("class")?;
        let name_tok = self.expect_ident("class name")?;
        let mut extends_name: Option<String> = None;
        if self.match_ident("extends") {
            let mut name = self.expect_ident("parent class name")?.value;
            while self.is_punct("\\", 0) {
                self.next();
                name.push('\\');
                name.push_str(&self.expect_ident("namespace segment")?.value);
            }
            extends_name = Some(name);
        }
        let mut implements_names: Vec<String> = Vec::new();
        if self.match_ident("implements") {
            loop {
                let mut name = self.expect_ident("interface name")?.value;
                while self.is_punct("\\", 0) {
                    self.next();
                    name.push('\\');
                    name.push_str(&self.expect_ident("namespace segment")?.value);
                }
                implements_names.push(name);
                if !self.match_punct(",") {
                    break;
                }
            }
        }
        self.expect_punct("{")?;
        let mut methods: Vec<Stmt> = Vec::new();
        let mut properties: Vec<String> = Vec::new();
        while !self.is_punct("}", 0) && !self.at_eof() {
            let tok = self.peek(0).clone();
            if tok.kind == TokenKind::Ident
                && (tok.value.to_lowercase() == "abstract" || tok.value.to_lowercase() == "final")
            {
                self.next();
                continue;
            }
            if tok.kind == TokenKind::Ident {
                let kw = tok.value.to_lowercase();
                if kw == "public" || kw == "private" || kw == "protected" {
                    let visibility = kw.clone();
                    self.next();
                    let mut is_static = false;
                    if self.is_ident("static", 0) {
                        self.next();
                        is_static = true;
                    }
                    if self.is_ident("function", 0) {
                        self.next();
                        let m_name = self.expect_ident("method name")?;
                        let params = self.parse_params()?;
                        if self.match_punct(":") {
                            self.parse_type_name()?;
                        }
                        let body = if self.is_punct(";", 0) {
                            self.next();
                            Vec::new()
                        } else {
                            self.parse_block()?
                        };
                        methods.push(Stmt::Method {
                            name: m_name.value,
                            visibility,
                            is_static,
                            params,
                            body,
                            pos: tok.pos,
                        });
                        continue;
                    }
                    if self.peek(0).kind == TokenKind::Var {
                        let v = self.next();
                        properties.push(v.value);
                        if self.match_punct("=") {
                            self.parse_expression()?;
                        }
                        self.expect_punct(";")?;
                        continue;
                    }
                    if self.is_ident("const", 0) {
                        self.next();
                        self.expect_ident("constant name")?;
                        self.expect_punct("=")?;
                        self.parse_expression()?;
                        self.expect_punct(";")?;
                        continue;
                    }
                }
                if kw == "function" {
                    self.next();
                    let m_name = self.expect_ident("method name")?;
                    let params = self.parse_params()?;
                    if self.match_punct(":") {
                        self.parse_type_name()?;
                    }
                    let body = if self.is_punct(";", 0) {
                        self.next();
                        Vec::new()
                    } else {
                        self.parse_block()?
                    };
                    methods.push(Stmt::Method {
                        name: m_name.value,
                        visibility: "public".to_string(),
                        is_static: false,
                        params,
                        body,
                        pos: tok.pos,
                    });
                    continue;
                }
                if kw == "const" {
                    self.next();
                    self.expect_ident("constant name")?;
                    self.expect_punct("=")?;
                    self.parse_expression()?;
                    self.expect_punct(";")?;
                    continue;
                }
                if kw == "var" {
                    self.next();
                    let v = self.peek(0).clone();
                    if v.kind == TokenKind::Var {
                        self.next();
                        properties.push(v.value);
                        if self.match_punct("=") {
                            self.parse_expression()?;
                        }
                        self.expect_punct(";")?;
                        continue;
                    }
                }
            }
            if tok.kind == TokenKind::Var {
                self.next();
                properties.push(tok.value);
                if self.match_punct("=") {
                    self.parse_expression()?;
                }
                self.expect_punct(";")?;
                continue;
            }
            if self.is_punct(";", 0) {
                self.next();
                continue;
            }
            // Unknown member — skip one token to stay in sync.
            self.next();
        }
        self.expect_punct("}")?;
        Ok(Stmt::Class {
            name: name_tok.value,
            extends_name,
            implements_names,
            methods,
            properties,
            pos: start,
        })
    }

    fn parse_if(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect_ident("if")?.pos;
        self.expect_punct("(")?;
        let test = self.parse_expression()?;
        self.expect_punct(")")?;
        if self.is_punct(":", 0) {
            self.next();
            let consequent = self.parse_alt_body(&["elseif", "else", "endif"])?;
            let mut alternate = IfAlternate::None;
            if self.is_ident("elseif", 0) {
                alternate = IfAlternate::If(Box::new(self.parse_alt_else_if_chain()?));
            } else if self.is_ident("else", 0) {
                self.next();
                self.expect_punct(":")?;
                alternate = IfAlternate::Stmts(self.parse_alt_body(&["endif"])?);
            }
            self.expect_ident("endif")?;
            self.optional_semi();
            return Ok(Stmt::If {
                test,
                consequent,
                alternate,
                alt_syntax: Some(true),
                pos: start,
            });
        }
        let consequent = self.parse_branch()?;
        let mut alternate = IfAlternate::None;
        if self.is_ident("else", 0) {
            self.next();
            if self.is_ident("if", 0) {
                alternate = IfAlternate::If(Box::new(self.parse_if()?));
            } else {
                alternate = IfAlternate::Stmts(self.parse_branch()?);
            }
        } else if self.is_ident("elseif", 0) {
            alternate = IfAlternate::If(Box::new(self.parse_else_if()?));
        }
        Ok(Stmt::If {
            test,
            consequent,
            alternate,
            alt_syntax: None,
            pos: start,
        })
    }

    fn parse_else_if(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect_ident("elseif")?.pos;
        self.expect_punct("(")?;
        let test = self.parse_expression()?;
        self.expect_punct(")")?;
        let consequent = self.parse_branch()?;
        let mut alternate = IfAlternate::None;
        if self.is_ident("elseif", 0) {
            alternate = IfAlternate::If(Box::new(self.parse_else_if()?));
        } else if self.is_ident("else", 0) {
            self.next();
            alternate = if self.is_ident("if", 0) {
                IfAlternate::If(Box::new(self.parse_if()?))
            } else {
                IfAlternate::Stmts(self.parse_branch()?)
            };
        }
        Ok(Stmt::If {
            test,
            consequent,
            alternate,
            alt_syntax: None,
            pos: start,
        })
    }

    fn parse_alt_else_if_chain(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect_ident("elseif")?.pos;
        self.expect_punct("(")?;
        let test = self.parse_expression()?;
        self.expect_punct(")")?;
        self.expect_punct(":")?;
        let consequent = self.parse_alt_body(&["elseif", "else", "endif"])?;
        let mut alternate = IfAlternate::None;
        if self.is_ident("elseif", 0) {
            alternate = IfAlternate::If(Box::new(self.parse_alt_else_if_chain()?));
        } else if self.is_ident("else", 0) {
            self.next();
            self.expect_punct(":")?;
            alternate = IfAlternate::Stmts(self.parse_alt_body(&["endif"])?);
        }
        Ok(Stmt::If {
            test,
            consequent,
            alternate,
            alt_syntax: Some(true),
            pos: start,
        })
    }

    fn optional_semi(&mut self) {
        if self.is_punct(";", 0) {
            self.next();
        }
    }

    fn parse_alt_body(&mut self, terminators: &[&str]) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts: Vec<Stmt> = Vec::new();
        while !self.at_eof() {
            let tok = self.peek(0).clone();
            if tok.kind == TokenKind::CloseTag {
                self.next();
                continue;
            }
            if tok.kind == TokenKind::InlineHtml {
                self.next();
                stmts.push(Stmt::InlineHtml {
                    text: tok.value,
                    pos: tok.pos,
                });
                continue;
            }
            if tok.kind == TokenKind::OpenTag {
                self.next();
                if tok.short_echo {
                    let expr = self.parse_expression()?;
                    stmts.push(Stmt::Echo {
                        values: vec![expr],
                        pos: tok.pos,
                    });
                    if self.peek(0).kind == TokenKind::CloseTag {
                        self.next();
                    }
                }
                continue;
            }
            if tok.kind == TokenKind::Ident
                && terminators.contains(&tok.value.to_lowercase().as_str())
            {
                break;
            }
            if tok.kind == TokenKind::Punct && tok.value == "}" {
                break;
            }
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            }
        }
        Ok(stmts)
    }

    fn parse_branch(&mut self) -> Result<Vec<Stmt>, ParseError> {
        if self.is_punct("{", 0) {
            return self.parse_block();
        }
        if self.is_punct(":", 0) {
            return Err(self.error(
                "Alternative if/foreach syntax (`:` … `endif`) is not supported here",
                self.peek(0).pos,
            ));
        }
        match self.parse_statement()? {
            Some(s) => Ok(vec![s]),
            None => Ok(vec![]),
        }
    }

    fn parse_foreach(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect_ident("foreach")?.pos;
        self.expect_punct("(")?;
        let iterable = self.parse_expression()?;
        if !self.match_ident("as") {
            return Err(self.error("Expected 'as' in foreach", self.peek(0).pos));
        }
        // Extension beyond the TS parser (see README): `foreach (... as &$v)`
        // — a by-reference loop variable. The `&` is consumed but not
        // recorded (the TS AST has no field for it).
        self.match_punct("&");
        let first_var = self.peek(0).clone();
        if first_var.kind != TokenKind::Var {
            return Err(self.error("Expected a loop variable in foreach", first_var.pos));
        }
        self.next();
        let mut key_var: Option<String> = None;
        let mut value_var = first_var.value;
        if self.match_punct("=>") {
            key_var = Some(value_var);
            // `foreach (... as $k => &$v)` — same by-reference extension.
            self.match_punct("&");
            let v2 = self.peek(0).clone();
            if v2.kind != TokenKind::Var {
                return Err(self.error("Expected a loop value variable in foreach", v2.pos));
            }
            self.next();
            value_var = v2.value;
        }
        self.expect_punct(")")?;
        if self.is_punct(":", 0) {
            self.next();
            let body = self.parse_alt_body(&["endforeach"])?;
            self.expect_ident("endforeach")?;
            self.optional_semi();
            return Ok(Stmt::Foreach {
                iterable,
                key_var,
                value_var,
                body,
                alt_syntax: Some(true),
                pos: start,
            });
        }
        let body = self.parse_branch()?;
        Ok(Stmt::Foreach {
            iterable,
            key_var,
            value_var,
            body,
            alt_syntax: None,
            pos: start,
        })
    }

    fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect_ident("for")?.pos;
        self.expect_punct("(")?;
        let init = self.parse_for_slot(";")?;
        self.expect_punct(";")?;
        let test = self.parse_for_slot(";")?;
        self.expect_punct(";")?;
        let update = self.parse_for_slot(")")?;
        self.expect_punct(")")?;
        if self.is_punct(":", 0) {
            self.next();
            let body = self.parse_alt_body(&["endfor"])?;
            self.expect_ident("endfor")?;
            self.optional_semi();
            return Ok(Stmt::For {
                init,
                test,
                update,
                body,
                alt_syntax: Some(true),
                pos: start,
            });
        }
        let body = self.parse_branch()?;
        Ok(Stmt::For {
            init,
            test,
            update,
            body,
            alt_syntax: None,
            pos: start,
        })
    }

    /// One slot of a `for (...)` header: a comma-separated expression list.
    /// A single expression is returned as-is; several are wrapped in a
    /// `sequence` node (extension beyond the TS parser — see README).
    fn parse_for_slot(&mut self, terminator: &str) -> Result<Option<Expr>, ParseError> {
        if self.is_punct(terminator, 0) {
            return Ok(None);
        }
        let mut exprs = vec![self.parse_expression()?];
        while self.match_punct(",") {
            exprs.push(self.parse_expression()?);
        }
        if exprs.len() == 1 {
            return Ok(exprs.into_iter().next());
        }
        let pos = expr_pos(&exprs[0]);
        Ok(Some(Expr::Sequence { exprs, pos }))
    }

    fn parse_while(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect_ident("while")?.pos;
        self.expect_punct("(")?;
        let test = self.parse_expression()?;
        self.expect_punct(")")?;
        if self.is_punct(":", 0) {
            self.next();
            let body = self.parse_alt_body(&["endwhile"])?;
            self.expect_ident("endwhile")?;
            self.optional_semi();
            return Ok(Stmt::While {
                test,
                body,
                alt_syntax: Some(true),
                pos: start,
            });
        }
        let body = self.parse_branch()?;
        Ok(Stmt::While {
            test,
            body,
            alt_syntax: None,
            pos: start,
        })
    }

    fn parse_switch(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect_ident("switch")?.pos;
        self.expect_punct("(")?;
        let discriminant = self.parse_expression()?;
        self.expect_punct(")")?;
        let from = start;
        if self.is_punct(":", 0) {
            self.next();
            while !self.at_eof() && !self.is_ident("endswitch", 0) {
                self.next();
            }
            self.expect_ident("endswitch")?;
            self.optional_semi();
            return Ok(Stmt::Switch {
                discriminant,
                alt_syntax: Some(true),
                source: self.slice_source(from, self.peek(0).pos),
                pos: start,
            });
        }
        self.expect_punct("{")?;
        let mut depth = 1i32;
        while !self.at_eof() && depth > 0 {
            let t = self.next();
            if t.kind == TokenKind::Punct && t.value == "{" {
                depth += 1;
            }
            if t.kind == TokenKind::Punct && t.value == "}" {
                depth -= 1;
            }
        }
        Ok(Stmt::Switch {
            discriminant,
            alt_syntax: None,
            source: self.slice_source(from, self.peek(0).pos),
            pos: start,
        })
    }

    fn parse_try(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect_ident("try")?.pos;
        let body = self.parse_block()?;
        let mut catches: Vec<CatchClause> = Vec::new();
        while self.is_ident("catch", 0) {
            self.next();
            self.expect_punct("(")?;
            while !self.is_punct(")", 0) && !self.at_eof() {
                self.next();
            }
            self.expect_punct(")")?;
            let catch_body = self.parse_block()?;
            catches.push(CatchClause {
                var_name: None,
                body: catch_body,
            });
        }
        let mut finally_body: Option<Vec<Stmt>> = None;
        if self.is_ident("finally", 0) {
            self.next();
            finally_body = Some(self.parse_block()?);
        }
        Ok(Stmt::Try {
            body,
            catches,
            finally_body,
            pos: start,
        })
    }

    fn parse_closure(&mut self, start: Pos) -> Result<Expr, ParseError> {
        let params = self.parse_params()?;
        let mut uses: Vec<String> = Vec::new();
        if self.match_ident("use") {
            self.expect_punct("(")?;
            while !self.is_punct(")", 0) && !self.at_eof() {
                if self.match_punct("&") {
                    continue;
                }
                let v = self.peek(0).clone();
                if v.kind == TokenKind::Var {
                    self.next();
                    uses.push(v.value);
                } else {
                    self.next();
                }
                if !self.match_punct(",") {
                    break;
                }
            }
            self.expect_punct(")")?;
        }
        if self.match_punct(":") {
            self.parse_type_name()?;
        }
        let body = self.parse_block()?;
        Ok(Expr::Closure {
            params,
            uses,
            body,
            pos: start,
        })
    }

    // ------------------------------------------------------------ expressions

    pub fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_ternary()?;
        let tok = self.peek(0).clone();
        if tok.kind == TokenKind::Punct && ASSIGN_OPS.contains(&tok.value.as_str()) {
            self.next();
            let right = self.parse_assignment()?;
            let pos = expr_pos(&left);
            return Ok(Expr::Assign {
                operator: tok.value,
                target: Box::new(left),
                value: Box::new(right),
                pos,
            });
        }
        Ok(left)
    }

    fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
        let test = self.parse_binary(1)?;
        if !self.is_punct("?", 0) {
            return Ok(test);
        }
        self.next();
        let mut consequent: Option<Expr> = None;
        if !self.is_punct(":", 0) {
            consequent = Some(self.parse_expression()?);
        }
        self.expect_punct(":")?;
        let alternate = self.parse_expression()?;
        let pos = expr_pos(&test);
        Ok(Expr::Ternary {
            test: Box::new(test),
            consequent: consequent.map(Box::new),
            alternate: Box::new(alternate),
            pos,
        })
    }

    fn parse_binary(&mut self, min_prec: u32) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let tok = self.peek(0).clone();
            let mut op: Option<String> = None;
            if tok.kind == TokenKind::Punct {
                if binary_precedence(&tok.value).is_some() {
                    op = Some(tok.value.clone());
                }
            } else if tok.kind == TokenKind::Ident {
                let kw = tok.value.to_lowercase();
                if kw == "and" || kw == "or" || kw == "xor" || kw == "instanceof" {
                    op = Some(kw);
                }
            }
            let op = match op {
                Some(o) => o,
                None => break,
            };
            let prec = binary_precedence(&op).unwrap();
            if prec < min_prec {
                break;
            }
            self.next();
            let right = self.parse_binary(if op == "**" { prec } else { prec + 1 })?;
            let pos = expr_pos(&left);
            left = Expr::Binary {
                operator: op,
                left: Box::new(left),
                right: Box::new(right),
                pos,
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.peek(0).clone();
        // Extension beyond the TS parser (see README): `&` as a reference
        // prefix (`=& $x`, `foo(&$x)`, `return &$x`). Consumed and not
        // represented — the TS AST has no reference node. (Binary `&` is
        // handled by parse_binary and never reaches this position.)
        if tok.kind == TokenKind::Punct && tok.value == "&" {
            self.next();
            return self.parse_unary();
        }
        if tok.kind == TokenKind::Punct && ["!", "-", "+", "~", "@"].contains(&tok.value.as_str()) {
            self.next();
            let operand = self.parse_unary()?;
            return Ok(Expr::Unary {
                operator: tok.value,
                operand: Box::new(operand),
                prefix: true,
                pos: tok.pos,
            });
        }
        if tok.kind == TokenKind::Punct && (tok.value == "++" || tok.value == "--") {
            self.next();
            let operand = self.parse_unary()?;
            return Ok(Expr::Unary {
                operator: tok.value,
                operand: Box::new(operand),
                prefix: true,
                pos: tok.pos,
            });
        }
        if tok.kind == TokenKind::Ident {
            let kw = tok.value.to_lowercase();
            if kw == "isset" || kw == "empty" {
                self.next();
                self.expect_punct("(")?;
                let mut args: Vec<Expr> = Vec::new();
                while !self.is_punct(")", 0) && !self.at_eof() {
                    args.push(self.parse_expression()?);
                    if !self.match_punct(",") {
                        break;
                    }
                }
                self.expect_punct(")")?;
                return Ok(Expr::Call {
                    name: Some(kw),
                    callee: None,
                    args,
                    pos: tok.pos,
                });
            }
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            let tok = self.peek(0).clone();
            if tok.kind == TokenKind::Punct && tok.value == "(" {
                self.next();
                let args = self.parse_args()?;
                self.expect_punct(")")?;
                let pos = expr_pos(&expr);
                expr = Expr::Call {
                    name: None,
                    callee: Some(Box::new(expr)),
                    args,
                    pos,
                };
                continue;
            }
            if tok.kind == TokenKind::Punct && tok.value == "[" {
                self.next();
                if self.is_punct("]", 0) {
                    self.next();
                    let pos = expr_pos(&expr);
                    expr = Expr::ArrayAccess {
                        object: Box::new(expr),
                        index: None,
                        append: true,
                        pos,
                    };
                } else {
                    let index = self.parse_expression()?;
                    self.expect_punct("]")?;
                    let pos = expr_pos(&expr);
                    expr = Expr::ArrayAccess {
                        object: Box::new(expr),
                        index: Some(Box::new(index)),
                        append: false,
                        pos,
                    };
                }
                continue;
            }
            if tok.kind == TokenKind::Punct && (tok.value == "->" || tok.value == "?->") {
                self.next();
                // Extension beyond the TS parser (see README): dynamic
                // member access `$obj->{$expr}` / `$obj->{$method}()`.
                if self.is_punct("{", 0) {
                    self.next(); // {
                    let inner_start = self.peek(0).pos;
                    let _inner = self.parse_expression()?;
                    let inner_end = self.peek(0).pos;
                    self.expect_punct("}")?;
                    let inner_src = self.slice_source(inner_start, inner_end);
                    let member = format!("{{{inner_src}}}");
                    let pos = expr_pos(&expr);
                    if self.is_punct("(", 0) {
                        self.next();
                        let args = self.parse_args()?;
                        self.expect_punct(")")?;
                        expr = Expr::MethodCall {
                            object: Box::new(expr),
                            method: member,
                            args,
                            pos,
                        };
                    } else {
                        expr = Expr::PropertyFetch {
                            object: Box::new(expr),
                            property: member,
                            pos,
                        };
                    }
                    continue;
                }
                let member = self.peek(0).clone();
                if member.kind == TokenKind::Ident {
                    self.next();
                    if self.is_punct("(", 0) {
                        self.next();
                        let args = self.parse_args()?;
                        self.expect_punct(")")?;
                        let pos = expr_pos(&expr);
                        expr = Expr::MethodCall {
                            object: Box::new(expr),
                            method: member.value,
                            args,
                            pos,
                        };
                    } else {
                        let pos = expr_pos(&expr);
                        expr = Expr::PropertyFetch {
                            object: Box::new(expr),
                            property: member.value,
                            pos,
                        };
                    }
                    continue;
                }
                if member.kind == TokenKind::Var {
                    self.next();
                    let pos = expr_pos(&expr);
                    expr = Expr::PropertyFetch {
                        object: Box::new(expr),
                        property: format!("${}", member.value),
                        pos,
                    };
                    continue;
                }
                return Err(self.error("Expected a member name after ->", member.pos));
            }
            if tok.kind == TokenKind::Punct && tok.value == "::" {
                self.next();
                let member = self.peek(0).clone();
                if member.kind != TokenKind::Ident && member.kind != TokenKind::Var {
                    return Err(self.error("Expected a member name after ::", member.pos));
                }
                self.next();
                let pos = expr_pos(&expr);
                let class_name = match &expr {
                    Expr::ConstFetch { name, .. } => name.clone(),
                    Expr::Var { name, .. } => format!("${name}"),
                    _ => "static".to_string(),
                };
                if self.is_punct("(", 0) {
                    self.next();
                    let args = self.parse_args()?;
                    self.expect_punct(")")?;
                    expr = Expr::StaticCall {
                        class_name,
                        member: member.value,
                        args,
                        is_constant: false,
                        pos,
                    };
                } else {
                    expr = Expr::StaticCall {
                        class_name,
                        member: member.value,
                        args: vec![],
                        is_constant: true,
                        pos,
                    };
                }
                continue;
            }
            if tok.kind == TokenKind::Punct && (tok.value == "++" || tok.value == "--") {
                self.next();
                let pos = expr_pos(&expr);
                expr = Expr::Unary {
                    operator: tok.value,
                    operand: Box::new(expr),
                    prefix: false,
                    pos,
                };
                continue;
            }
            break;
        }
        Ok(expr)
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args: Vec<Expr> = Vec::new();
        while !self.is_punct(")", 0) && !self.at_eof() {
            if self.match_punct("...") {
                args.push(self.parse_expression()?);
            } else {
                let save = self.pos;
                if self.peek(0).kind == TokenKind::Ident
                    && self.peek(1).kind == TokenKind::Punct
                    && self.peek(1).value == ":"
                {
                    self.next();
                    self.next();
                } else {
                    self.pos = save;
                }
                args.push(self.parse_expression()?);
            }
            if !self.match_punct(",") {
                break;
            }
        }
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.peek(0).clone();
        match tok.kind {
            TokenKind::Number => {
                self.next();
                let raw = &tok.value;
                let value: serde_json::Value = if raw.starts_with("0x") || raw.starts_with("0X") {
                    let v = i64::from_str_radix(&raw[2..], 16).unwrap_or(0);
                    serde_json::Value::from(v)
                } else if raw.starts_with("0b") || raw.starts_with("0B") {
                    let v = i64::from_str_radix(&raw[2..], 2).unwrap_or(0);
                    serde_json::Value::from(v)
                } else if raw.starts_with("0o") || raw.starts_with("0O") {
                    let v = i64::from_str_radix(&raw[2..], 8).unwrap_or(0);
                    serde_json::Value::from(v)
                } else {
                    let f: f64 = raw.parse().unwrap_or(0.0);
                    number_value(f)
                };
                Ok(Expr::Literal {
                    literal_kind: "number".to_string(),
                    value,
                    pos: tok.pos,
                })
            }
            TokenKind::String => {
                self.next();
                Ok(Expr::Literal {
                    literal_kind: "string".to_string(),
                    value: serde_json::Value::String(tok.value),
                    pos: tok.pos,
                })
            }
            TokenKind::InterpString => {
                self.next();
                self.interp_to_expr(&tok, "complex string interpolation",
                    "Interpolated expressions like {$obj->prop} or $arr[0] inside a string are not translated; build the string with concatenation instead.")
            }
            TokenKind::Heredoc => {
                self.next();
                if tok.is_nowdoc {
                    return Ok(Expr::Literal {
                        literal_kind: "string".to_string(),
                        value: serde_json::Value::String(tok.value),
                        pos: tok.pos,
                    });
                }
                let label = tok.heredoc_label.clone().unwrap_or_default();
                self.interp_to_expr(
                    &tok,
                    "complex heredoc interpolation",
                    "Heredoc strings with complex interpolation are not translated.",
                )
                .map(|e| match e {
                    // Rewrite the source of the unsupported node to <<<LABEL.
                    Expr::UnsupportedExpr {
                        label: l,
                        reason,
                        pos,
                        ..
                    } => Expr::UnsupportedExpr {
                        label: l,
                        reason,
                        source: format!("<<<{label}"),
                        pos,
                    },
                    other => other,
                })
            }
            TokenKind::Backtick => {
                self.next();
                Ok(Expr::UnsupportedExpr {
                    label: "backtick execution".to_string(),
                    reason: "Backtick shell execution is blocked by the conversion sandbox."
                        .to_string(),
                    source: format!("`{}`", tok.value),
                    pos: tok.pos,
                })
            }
            TokenKind::Var => {
                self.next();
                Ok(Expr::Var {
                    name: tok.value,
                    pos: tok.pos,
                })
            }
            TokenKind::Punct => match tok.value.as_str() {
                "$" => {
                    self.next();
                    while self.peek(0).kind == TokenKind::Var || self.is_punct("$", 0) {
                        self.next();
                    }
                    Ok(Expr::UnsupportedExpr {
                            label: "variable variable".to_string(),
                            reason: "Variable variables ($$x) are not translated; name the variable directly.".to_string(),
                            source: "$$".to_string(),
                            pos: tok.pos,
                        })
                }
                "(" => {
                    let after_open = self.peek(1).clone();
                    let after_type = self.peek(2).clone();
                    if after_open.kind == TokenKind::Ident
                        && CAST_TYPES.contains(&after_open.value.to_lowercase().as_str())
                        && after_type.kind == TokenKind::Punct
                        && after_type.value == ")"
                    {
                        self.next();
                        let cast_type = self.next().value.to_lowercase();
                        self.next();
                        let operand = self.parse_unary()?;
                        return Ok(Expr::Cast {
                            cast_type,
                            operand: Box::new(operand),
                            pos: tok.pos,
                        });
                    }
                    self.next();
                    let inner = self.parse_expression()?;
                    self.expect_punct(")")?;
                    Ok(inner)
                }
                "[" => self.parse_array_literal(),
                "\\" => {
                    let name = self.parse_qualified_name()?;
                    self.finish_name(name, tok.pos)
                }
                _ => Err(self.error(
                    format!("Unexpected token {}", describe_token(&tok)),
                    tok.pos,
                )),
            },
            TokenKind::Ident => {
                let kw = tok.value.to_lowercase();
                match kw.as_str() {
                    "true" | "false" => {
                        self.next();
                        Ok(Expr::Literal {
                            literal_kind: "bool".to_string(),
                            value: serde_json::Value::Bool(kw == "true"),
                            pos: tok.pos,
                        })
                    }
                    "null" => {
                        self.next();
                        Ok(Expr::Literal {
                            literal_kind: "null".to_string(),
                            value: serde_json::Value::Null,
                            pos: tok.pos,
                        })
                    }
                    "array" => {
                        self.next();
                        self.expect_punct("(")?;
                        let entries = self.parse_array_entries(")")?;
                        self.expect_punct(")")?;
                        Ok(Expr::Array {
                            entries,
                            pos: tok.pos,
                        })
                    }
                    "function" => {
                        self.next();
                        self.parse_closure(tok.pos)
                    }
                    "static" => {
                        if self.peek(1).kind == TokenKind::Ident
                            && self.peek(1).value.to_lowercase() == "function"
                        {
                            self.next();
                            self.next();
                            self.parse_closure(tok.pos)
                        } else {
                            let name = self.parse_qualified_name()?;
                            self.finish_name(name, tok.pos)
                        }
                    }
                    "new" => {
                        self.next();
                        let class_name = self.parse_qualified_name_or_var()?;
                        let mut args: Vec<Expr> = Vec::new();
                        if self.is_punct("(", 0) {
                            self.next();
                            args = self.parse_args()?;
                            self.expect_punct(")")?;
                        }
                        Ok(Expr::New {
                            class_name,
                            args,
                            pos: tok.pos,
                        })
                    }
                    "yield" => {
                        self.next();
                        if self.is_ident("from", 0) {
                            self.next();
                            self.parse_expression()?;
                        } else if !self.is_punct(";", 0)
                            && !self.is_punct(")", 0)
                            && !self.is_punct(",", 0)
                            && !self.is_punct("]", 0)
                        {
                            self.parse_expression()?;
                            if self.match_punct("=>") {
                                self.parse_expression()?;
                            }
                        }
                        Ok(Expr::UnsupportedExpr {
                            label: "yield".to_string(),
                            reason: "Generators (yield) are not translated; restructure the function to return an array.".to_string(),
                            source: "yield".to_string(),
                            pos: tok.pos,
                        })
                    }
                    "list" => {
                        self.next();
                        self.expect_punct("(")?;
                        let mut names: Vec<Expr> = Vec::new();
                        while !self.is_punct(")", 0) && !self.at_eof() {
                            let v = self.peek(0).clone();
                            if v.kind == TokenKind::Var {
                                self.next();
                                names.push(Expr::Var {
                                    name: v.value,
                                    pos: v.pos,
                                });
                            } else {
                                names.push(self.parse_expression()?);
                            }
                            if !self.match_punct(",") {
                                break;
                            }
                        }
                        self.expect_punct(")")?;
                        Ok(Expr::Call {
                            name: Some("__list_assign".to_string()),
                            callee: None,
                            args: names,
                            pos: tok.pos,
                        })
                    }
                    "clone" => {
                        self.next();
                        let _operand = self.parse_unary()?;
                        Ok(Expr::UnsupportedExpr {
                            label: "clone".to_string(),
                            reason: "Object cloning has no equivalent in the Wordless extension runtime.".to_string(),
                            source: "clone".to_string(),
                            pos: tok.pos,
                        })
                    }
                    _ => {
                        let name = self.parse_qualified_name()?;
                        self.finish_name(name, tok.pos)
                    }
                }
            }
            _ => Err(self.error(
                format!("Unexpected token {}", describe_token(&tok)),
                tok.pos,
            )),
        }
    }

    /// Shared logic for interp_string / heredoc tokens: complex parts make
    /// the whole string an unsupported expression; a single text part is a
    /// plain literal; anything else is an interpolated_string node.
    fn interp_to_expr(
        &mut self,
        tok: &Token,
        complex_label: &str,
        complex_reason: &str,
    ) -> Result<Expr, ParseError> {
        let parts_raw = tok.parts.clone().unwrap_or_default();
        let mut parts: Vec<InterpPart> = Vec::new();
        let mut has_complex = false;
        for p in &parts_raw {
            match p.kind {
                "var" => parts.push(InterpPart {
                    kind: "var".to_string(),
                    value: p.value.clone(),
                }),
                "complex" => {
                    has_complex = true;
                    parts.push(InterpPart {
                        kind: "text".to_string(),
                        value: String::new(),
                    });
                }
                _ => parts.push(InterpPart {
                    kind: "text".to_string(),
                    value: p.value.clone(),
                }),
            }
        }
        if has_complex {
            return Ok(Expr::UnsupportedExpr {
                label: complex_label.to_string(),
                reason: complex_reason.to_string(),
                source: tok.value.clone(),
                pos: tok.pos,
            });
        }
        if parts.len() == 1 && parts[0].kind == "text" {
            return Ok(Expr::Literal {
                literal_kind: "string".to_string(),
                value: serde_json::Value::String(parts[0].value.clone()),
                pos: tok.pos,
            });
        }
        Ok(Expr::InterpolatedString {
            parts,
            pos: tok.pos,
        })
    }

    fn parse_qualified_name(&mut self) -> Result<String, ParseError> {
        let mut name = String::new();
        if self.is_punct("\\", 0) {
            self.next();
            name.push('\\');
        }
        let first = self.expect_ident("name")?;
        name.push_str(&first.value);
        while self.is_punct("\\", 0) {
            self.next();
            name.push('\\');
            name.push_str(&self.expect_ident("namespace segment")?.value);
        }
        Ok(name)
    }

    fn parse_qualified_name_or_var(&mut self) -> Result<String, ParseError> {
        if self.peek(0).kind == TokenKind::Var {
            return Ok(format!("${}", self.next().value));
        }
        self.parse_qualified_name()
    }

    fn finish_name(&mut self, name: String, pos: Pos) -> Result<Expr, ParseError> {
        if self.is_punct("(", 0) {
            self.next();
            let args = self.parse_args()?;
            self.expect_punct(")")?;
            return Ok(Expr::Call {
                name: Some(name),
                callee: None,
                args,
                pos,
            });
        }
        Ok(Expr::ConstFetch { name, pos })
    }

    fn parse_array_literal(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect_punct("[")?.pos;
        let entries = self.parse_array_entries("]")?;
        self.expect_punct("]")?;
        Ok(Expr::Array {
            entries,
            pos: start,
        })
    }

    fn parse_array_entries(&mut self, closer: &str) -> Result<Vec<ArrayEntry>, ParseError> {
        let mut entries: Vec<ArrayEntry> = Vec::new();
        while !self.is_punct(closer, 0) && !self.at_eof() {
            if self.match_punct("...") {
                let value = self.parse_expression()?;
                entries.push(ArrayEntry { key: None, value });
                if !self.match_punct(",") {
                    break;
                }
                continue;
            }
            let first = self.parse_expression()?;
            if self.match_punct("=>") {
                let value = self.parse_expression()?;
                entries.push(ArrayEntry {
                    key: Some(first),
                    value,
                });
            } else {
                entries.push(ArrayEntry {
                    key: None,
                    value: first,
                });
            }
            if !self.match_punct(",") {
                break;
            }
        }
        Ok(entries)
    }
}

/// JSON number for an f64 literal: integral values that fit in i64 are
/// emitted as integers (matching TS `Number(raw)` printed by JSON.stringify);
/// non-finite values become null, as JSON.stringify does.
fn number_value(f: f64) -> serde_json::Value {
    if !f.is_finite() {
        return serde_json::Value::Null;
    }
    if f.fract() == 0.0 && f.abs() < 9007199254740992.0 {
        serde_json::Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    }
}
