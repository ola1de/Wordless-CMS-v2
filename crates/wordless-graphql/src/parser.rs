//! Hand-rolled GraphQL lexer + recursive-descent parser for the query
//! subset the v2 endpoint supports.
//!
//! Supported: `query` operations (and the `{ ... }` shorthand), named
//! operations, variable definitions with literal defaults, fragments
//! (`fragment X on T { ... }`, `...X`, `... on T { ... }`), aliases,
//! arguments (string / int / float / boolean / enum / null / list /
//! object literals, `$variables`), nested selection sets.
//!
//! NOT supported (rejected with a clear error, never silently ignored):
//! mutations, subscriptions, directives (`@skip`, `@include`, …),
//! schema definitions. Queries only — the endpoint is read-only, like
//! the REST reads it mirrors.

use serde_json::Value;

/// A literal argument value.
#[derive(Debug, Clone, PartialEq)]
pub enum GqlValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    /// A bare name used as a value (an enum literal, e.g. `status: publish`).
    Enum(String),
    List(Vec<GqlValue>),
    Object(Vec<(String, GqlValue)>),
    Variable(String),
}

impl GqlValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            GqlValue::String(s) | GqlValue::Enum(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            GqlValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Resolve variables against the supplied map; anything else passes
    /// through unchanged. An undefined variable is an error.
    pub fn resolve(&self, vars: &Value) -> Result<GqlValue, String> {
        match self {
            GqlValue::Variable(name) => {
                let v = vars.get(name).cloned().unwrap_or(Value::Null);
                Ok(json_to_gql(&v))
            }
            GqlValue::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for i in items {
                    out.push(i.resolve(vars)?);
                }
                Ok(GqlValue::List(out))
            }
            GqlValue::Object(fields) => {
                let mut out = Vec::with_capacity(fields.len());
                for (k, v) in fields {
                    out.push((k.clone(), v.resolve(vars)?));
                }
                Ok(GqlValue::Object(out))
            }
            other => Ok(other.clone()),
        }
    }

    /// Convert to a plain JSON value (for `option`-style passthrough).
    pub fn to_json(&self) -> Value {
        match self {
            GqlValue::String(s) | GqlValue::Enum(s) => Value::String(s.clone()),
            GqlValue::Int(n) => Value::from(*n),
            GqlValue::Float(f) => serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            GqlValue::Bool(b) => Value::Bool(*b),
            GqlValue::Null => Value::Null,
            GqlValue::List(items) => Value::Array(items.iter().map(|i| i.to_json()).collect()),
            GqlValue::Object(fields) => {
                let mut map = serde_json::Map::new();
                for (k, v) in fields {
                    map.insert(k.clone(), v.to_json());
                }
                Value::Object(map)
            }
            GqlValue::Variable(name) => Value::String(format!("${name}")),
        }
    }
}

fn json_to_gql(v: &Value) -> GqlValue {
    match v {
        Value::Null => GqlValue::Null,
        Value::Bool(b) => GqlValue::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                GqlValue::Int(i)
            } else {
                GqlValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => GqlValue::String(s.clone()),
        Value::Array(items) => GqlValue::List(items.iter().map(json_to_gql).collect()),
        Value::Object(map) => GqlValue::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_gql(v)))
                .collect(),
        ),
    }
}

/// One field in a selection set.
#[derive(Debug, Clone)]
pub struct Field {
    /// The response key: alias when present, else the field name.
    pub response_key: String,
    pub name: String,
    pub args: Vec<(String, GqlValue)>,
    pub selection: Vec<Selection>,
}

impl Field {
    pub fn arg(&self, name: &str) -> Option<&GqlValue> {
        self.args.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// A selection: a field, a named fragment spread, or an inline fragment.
#[derive(Debug, Clone)]
pub enum Selection {
    Field(Field),
    FragmentSpread(String),
    InlineFragment {
        type_condition: Option<String>,
        selection: Vec<Selection>,
    },
}

/// One operation (`query ... { ... }`).
#[derive(Debug, Clone)]
pub struct Operation {
    pub name: Option<String>,
    pub variables: Vec<VariableDef>,
    pub selection: Vec<Selection>,
}

#[derive(Debug, Clone)]
pub struct VariableDef {
    pub name: String,
    pub default: Option<GqlValue>,
}

/// A parsed document: operations + fragment definitions.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub operations: Vec<Operation>,
    pub fragments: Vec<(String, Option<String>, Vec<Selection>)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Name(String),
    Int(i64),
    Float(f64),
    Str(String),
    Punct(char),
    Spread, // `...`
    Variable(String),
    Eof,
}

fn lex(input: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() || c == ',' => {
                i += 1;
            }
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '{' | '}' | '(' | ')' | ':' | '=' | '!' | '[' | ']' | '@' => {
                out.push(Token::Punct(c));
                i += 1;
            }
            '.' => {
                if i + 2 < chars.len() && chars[i + 1] == '.' && chars[i + 2] == '.' {
                    out.push(Token::Spread);
                    i += 3;
                } else {
                    return Err(format!("Unexpected '.' at position {i}"));
                }
            }
            '$' => {
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if j == start {
                    return Err(format!(
                        "'$' must be followed by a variable name (position {i})"
                    ));
                }
                out.push(Token::Variable(chars[start..j].iter().collect()));
                i = j;
            }
            '"' => {
                // Double-quoted string with \" \\ \/ \n \t \r \b \f \uXXXX.
                let mut s = String::new();
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    let ch = chars[i];
                    if ch == '"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    if ch == '\\' && i + 1 < chars.len() {
                        let esc = chars[i + 1];
                        match esc {
                            '"' => s.push('"'),
                            '\\' => s.push('\\'),
                            '/' => s.push('/'),
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            'r' => s.push('\r'),
                            'b' => s.push('\u{0008}'),
                            'f' => s.push('\u{000C}'),
                            'u' if i + 5 < chars.len() => {
                                let hex: String = chars[i + 2..i + 6].iter().collect();
                                let code = u32::from_str_radix(&hex, 16)
                                    .map_err(|_| format!("Bad \\u escape '\\u{hex}'"))?;
                                let ch = char::from_u32(code)
                                    .ok_or_else(|| format!("Bad \\u escape '\\u{hex}'"))?;
                                s.push(ch);
                                i += 4;
                            }
                            other => {
                                return Err(format!("Bad escape '\\{other}' in string"));
                            }
                        }
                        i += 2;
                    } else {
                        s.push(ch);
                        i += 1;
                    }
                }
                if !closed {
                    return Err("Unterminated string literal".to_string());
                }
                out.push(Token::Str(s));
            }
            c if c.is_ascii_digit()
                || (c == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) =>
            {
                let start = i;
                if chars[i] == '-' {
                    i += 1;
                }
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let mut is_float = false;
                if i < chars.len() && chars[i] == '.' {
                    is_float = true;
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    is_float = true;
                    i += 1;
                    if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                        i += 1;
                    }
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                if is_float {
                    let f: f64 = text.parse().map_err(|_| format!("Bad number '{text}'"))?;
                    out.push(Token::Float(f));
                } else {
                    let n: i64 = text.parse().map_err(|_| format!("Bad number '{text}'"))?;
                    out.push(Token::Int(n));
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Token::Name(chars[start..i].iter().collect()));
            }
            other => return Err(format!("Unexpected character '{other}'")),
        }
    }
    out.push(Token::Eof);
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn next(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn expect_punct(&mut self, c: char) -> Result<(), String> {
        match self.next() {
            Token::Punct(p) if p == c => Ok(()),
            other => Err(format!("Expected '{c}', found {other:?}")),
        }
    }

    fn eat_punct(&mut self, c: char) -> bool {
        if *self.peek() == Token::Punct(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_name(&mut self) -> Result<String, String> {
        match self.next() {
            Token::Name(n) => Ok(n),
            other => Err(format!("Expected a name, found {other:?}")),
        }
    }

    fn peek_name(&self) -> Option<&str> {
        match self.peek() {
            Token::Name(n) => Some(n),
            _ => None,
        }
    }

    fn parse_document(&mut self) -> Result<Document, String> {
        let mut doc = Document::default();
        while *self.peek() != Token::Eof {
            match self.peek() {
                Token::Name(n) if n == "fragment" => {
                    self.next();
                    let name = self.expect_name()?;
                    let on = self.expect_name()?;
                    if on != "on" {
                        return Err(format!("Expected 'on' after fragment name, found '{on}'"));
                    }
                    let type_condition = self.expect_name()?;
                    let sel = self.parse_selection_set()?;
                    doc.fragments.push((name, Some(type_condition), sel));
                }
                Token::Name(n) if n == "query" => {
                    self.next();
                    doc.operations.push(self.parse_operation()?);
                }
                Token::Name(n) if n == "mutation" || n == "subscription" => {
                    return Err(format!(
                        "Mutations and subscriptions are not supported (found '{n}'); this endpoint is queries only"
                    ));
                }
                Token::Punct('{') => {
                    doc.operations.push(Operation {
                        name: None,
                        variables: Vec::new(),
                        selection: self.parse_selection_set()?,
                    });
                }
                other => {
                    return Err(format!("Unexpected {other:?} at document level"));
                }
            }
        }
        if doc.operations.is_empty() {
            return Err("No operation found in query".to_string());
        }
        Ok(doc)
    }

    /// After the `query` keyword: optional name, optional variable
    /// definitions, then the selection set.
    fn parse_operation(&mut self) -> Result<Operation, String> {
        let mut name = None;
        if let Token::Name(n) = self.peek().clone() {
            name = Some(n);
            self.next();
        }
        let mut variables = Vec::new();
        if self.eat_punct('(') {
            while !self.eat_punct(')') {
                let var = match self.next() {
                    Token::Variable(v) => v,
                    other => return Err(format!("Expected a variable, found {other:?}")),
                };
                self.expect_punct(':')?;
                // Type: a name, possibly wrapped in [ ] and suffixed with !.
                self.parse_type()?;
                let mut default = None;
                if self.eat_punct('=') {
                    default = Some(self.parse_value()?);
                }
                variables.push(VariableDef { name: var, default });
            }
        }
        let selection = self.parse_selection_set()?;
        Ok(Operation {
            name,
            variables,
            selection,
        })
    }

    fn parse_type(&mut self) -> Result<(), String> {
        if self.eat_punct('[') {
            self.parse_type()?;
            self.expect_punct(']')?;
        } else {
            self.expect_name()?;
        }
        self.eat_punct('!');
        Ok(())
    }

    fn parse_selection_set(&mut self) -> Result<Vec<Selection>, String> {
        self.expect_punct('{')?;
        let mut out = Vec::new();
        while !self.eat_punct('}') {
            if *self.peek() == Token::Eof {
                return Err("Unterminated selection set".to_string());
            }
            out.push(self.parse_selection()?);
        }
        Ok(out)
    }

    fn parse_selection(&mut self) -> Result<Selection, String> {
        if *self.peek() == Token::Spread {
            self.next();
            if self.peek_name() == Some("on") {
                self.next();
                let type_condition = Some(self.expect_name()?);
                let sel = self.parse_selection_set()?;
                return Ok(Selection::InlineFragment {
                    type_condition,
                    selection: sel,
                });
            }
            let name = self.expect_name()?;
            // A fragment spread may carry directives — rejected below.
            self.reject_directives()?;
            return Ok(Selection::FragmentSpread(name));
        }
        // Field: [alias ':'] name [args] [directives] [selection set]
        let first = self.expect_name()?;
        let (response_key, name) = if self.eat_punct(':') {
            (first, self.expect_name()?)
        } else {
            (first.clone(), first)
        };
        let mut args = Vec::new();
        if self.eat_punct('(') {
            while !self.eat_punct(')') {
                if *self.peek() == Token::Eof {
                    return Err("Unterminated argument list".to_string());
                }
                let arg_name = self.expect_name()?;
                self.expect_punct(':')?;
                let value = self.parse_value()?;
                args.push((arg_name, value));
            }
        }
        self.reject_directives()?;
        let selection = if *self.peek() == Token::Punct('{') {
            self.parse_selection_set()?
        } else {
            Vec::new()
        };
        Ok(Selection::Field(Field {
            response_key,
            name,
            args,
            selection,
        }))
    }

    fn reject_directives(&mut self) -> Result<(), String> {
        if *self.peek() == Token::Punct('@') {
            return Err(
                "Directives (@skip, @include, …) are not supported by this endpoint".to_string(),
            );
        }
        Ok(())
    }

    fn parse_value(&mut self) -> Result<GqlValue, String> {
        match self.next() {
            Token::Str(s) => Ok(GqlValue::String(s)),
            Token::Int(n) => Ok(GqlValue::Int(n)),
            Token::Float(f) => Ok(GqlValue::Float(f)),
            Token::Variable(v) => Ok(GqlValue::Variable(v)),
            Token::Name(n) => match n.as_str() {
                "true" => Ok(GqlValue::Bool(true)),
                "false" => Ok(GqlValue::Bool(false)),
                "null" => Ok(GqlValue::Null),
                _ => Ok(GqlValue::Enum(n)),
            },
            Token::Punct('[') => {
                let mut items = Vec::new();
                while !self.eat_punct(']') {
                    if *self.peek() == Token::Eof {
                        return Err("Unterminated list value".to_string());
                    }
                    items.push(self.parse_value()?);
                }
                Ok(GqlValue::List(items))
            }
            Token::Punct('{') => {
                let mut fields = Vec::new();
                while !self.eat_punct('}') {
                    if *self.peek() == Token::Eof {
                        return Err("Unterminated object value".to_string());
                    }
                    let k = self.expect_name()?;
                    self.expect_punct(':')?;
                    let v = self.parse_value()?;
                    fields.push((k, v));
                }
                Ok(GqlValue::Object(fields))
            }
            other => Err(format!("Unexpected value {other:?}")),
        }
    }
}

/// Parse a GraphQL query string into a `Document`.
pub fn parse_query(input: &str) -> Result<Document, String> {
    let tokens = lex(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    parser.parse_document()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shorthand_and_aliases() {
        let doc = parse_query("{ hero: post(id: 1) { id title } }").unwrap();
        assert_eq!(doc.operations.len(), 1);
        let sel = &doc.operations[0].selection;
        assert_eq!(sel.len(), 1);
        if let Selection::Field(f) = &sel[0] {
            assert_eq!(f.response_key, "hero");
            assert_eq!(f.name, "post");
            assert_eq!(f.arg("id"), Some(&GqlValue::Int(1)));
            assert_eq!(f.selection.len(), 2);
        } else {
            panic!("expected a field");
        }
    }

    #[test]
    fn parses_named_operation_variables_and_fragments() {
        let doc = parse_query(
            "query GetPost($id: Int = 2) { post(id: $id) { ...PostBits } } fragment PostBits on Post { id slug }",
        )
        .unwrap();
        assert_eq!(doc.operations[0].name.as_deref(), Some("GetPost"));
        assert_eq!(doc.operations[0].variables.len(), 1);
        assert_eq!(doc.fragments.len(), 1);
        assert_eq!(doc.fragments[0].0, "PostBits");
    }

    #[test]
    fn rejects_mutations_and_directives() {
        assert!(parse_query("mutation { x }").is_err());
        assert!(parse_query("{ post(id: 1) @skip(if: true) { id } }").is_err());
        assert!(parse_query("{ post(id: 1) { id }").is_err());
    }

    #[test]
    fn parses_string_escapes_and_lists() {
        let doc =
            parse_query(r#"{ posts(status: publish, page: 1) { posts { title } } }"#).unwrap();
        if let Selection::Field(f) = &doc.operations[0].selection[0] {
            assert_eq!(
                f.arg("status"),
                Some(&GqlValue::Enum("publish".to_string()))
            );
        } else {
            panic!("expected a field");
        }
    }
}
