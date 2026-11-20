//! PHP tokenizer — a mirror of the TypeScript converter's `lexer.ts`
//! (`wordless-cms/src/conversion/lexer.ts`).
//!
//! Operates on raw bytes: all delimiters are ASCII, so slicing at ASCII
//! boundaries never splits a UTF-8 sequence. Positions are tracked in
//! bytes/columns; for the ASCII sources WordPress code is made of, these
//! match the TypeScript positions exactly.

use crate::ast::Pos;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    OpenTag,
    CloseTag,
    InlineHtml,
    Ident,
    Var,
    Number,
    String,
    InterpString,
    Heredoc,
    Backtick,
    Punct,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringPart {
    /// "text" | "var" | "complex"
    pub kind: &'static str,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    /// Decoded value (strings) or raw text (everything else).
    pub value: String,
    pub pos: Pos,
    /// For open_tag: true when the tag was `<?=` (short echo).
    pub short_echo: bool,
    /// For string / interp_string / heredoc: decoded parts.
    pub parts: Option<Vec<StringPart>>,
    /// For heredoc: the label; for nowdoc this is set and is_nowdoc is true.
    pub heredoc_label: Option<String>,
    pub is_nowdoc: bool,
}

impl Token {
    fn new(kind: TokenKind, value: String, pos: Pos) -> Self {
        Token {
            kind,
            value,
            pos,
            short_echo: false,
            parts: None,
            heredoc_label: None,
            is_nowdoc: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LexComment {
    pub text: String,
    pub pos: Pos,
}

#[derive(Debug, Clone)]
pub struct LexError {
    pub message: String,
    pub pos: Pos,
}

impl LexError {
    fn new(message: impl Into<String>, pos: Pos) -> Self {
        LexError {
            message: message.into(),
            pos,
        }
    }
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (line {}, column {})",
            self.message, self.pos.line, self.pos.column
        )
    }
}

const MULTI_PUNCT: &[&str] = &[
    "...", "**=", "??=", "===", "!==", "<=>", "**", "<<=", ">>=", "<<", ">>", "<=", ">=", "==",
    "!=", "&&", "||", "++", "--", "->", "=>", "::", "?->", "??", ".=", "+=", "-=", "*=", "/=",
    "%=", "&=", "|=", "^=",
];

fn is_single_punct(b: u8) -> bool {
    matches!(
        b,
        b'=' | b'<'
            | b'>'
            | b'+'
            | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'.'
            | b','
            | b';'
            | b':'
            | b'?'
            | b'('
            | b')'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'!'
            | b'~'
            | b'&'
            | b'|'
            | b'^'
            | b'@'
            | b'$'
            | b'\\'
    )
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

/// Decode PHP double-quoted escape sequences. Unknown escapes keep the backslash.
fn decode_double_quoted(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch == b'\\' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            match next {
                b'n' => {
                    out.push('\n');
                    i += 2;
                }
                b'r' => {
                    out.push('\r');
                    i += 2;
                }
                b't' => {
                    out.push('\t');
                    i += 2;
                }
                b'v' => {
                    out.push('\x0b');
                    i += 2;
                }
                b'e' => {
                    out.push('\x1b');
                    i += 2;
                }
                b'f' => {
                    out.push('\x0c');
                    i += 2;
                }
                b'\\' => {
                    out.push('\\');
                    i += 2;
                }
                b'"' => {
                    out.push('"');
                    i += 2;
                }
                b'$' => {
                    out.push('$');
                    i += 2;
                }
                _ => {
                    out.push('\\');
                    i += 1;
                }
            }
        } else {
            // Copy one UTF-8 char (or byte) verbatim.
            let s = &raw[i..];
            let c = s.chars().next().unwrap_or('\0');
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// Split a double-quoted / heredoc body into text / $var / complex parts.
fn split_interpolation(raw: &str) -> Vec<StringPart> {
    let bytes = raw.as_bytes();
    let mut parts: Vec<StringPart> = Vec::new();
    let mut text = String::new();
    let mut i = 0usize;
    let flush = |text: &mut String, parts: &mut Vec<StringPart>| {
        if !text.is_empty() {
            parts.push(StringPart {
                kind: "text",
                value: decode_double_quoted(text),
            });
            text.clear();
        }
    };
    while i < bytes.len() {
        let ch = bytes[i];
        if ch == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'$' {
            text.push_str("\\$");
            i += 2;
            continue;
        }
        if ch == b'$' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next == b'{' {
                flush(&mut text, &mut parts);
                let close = raw[i + 2..].find('}').map(|p| i + 2 + p);
                let end = close.map(|c| c + 1).unwrap_or(raw.len());
                parts.push(StringPart {
                    kind: "complex",
                    value: raw[i..end].to_string(),
                });
                i = end;
                continue;
            }
            if is_ident_start(next) {
                let mut j = i + 1;
                while j < bytes.len() && is_ident_char(bytes[j]) {
                    j += 1;
                }
                let after = bytes.get(j).copied().unwrap_or(0);
                if after == b'-' && bytes.get(j + 1).copied() == Some(b'>') {
                    flush(&mut text, &mut parts);
                    parts.push(StringPart {
                        kind: "complex",
                        value: format!("{}->", &raw[i..j]),
                    });
                    i = j + 2;
                    continue;
                }
                if after == b'[' {
                    flush(&mut text, &mut parts);
                    let close = raw[j..].find(']').map(|p| j + p);
                    let end = close.map(|c| c + 1).unwrap_or(j + 1);
                    parts.push(StringPart {
                        kind: "complex",
                        value: raw[i..end].to_string(),
                    });
                    i = end;
                    continue;
                }
                flush(&mut text, &mut parts);
                parts.push(StringPart {
                    kind: "var",
                    value: raw[i + 1..j].to_string(),
                });
                i = j;
                continue;
            }
        }
        // Append one char.
        let s = &raw[i..];
        let c = s.chars().next().unwrap_or('\0');
        text.push(c);
        i += c.len_utf8();
    }
    flush(&mut text, &mut parts);
    parts
}

pub struct Lexer<'a> {
    src: &'a [u8],
    i: usize,
    line: usize,
    column: usize,
    tokens: Vec<Token>,
    comments: Vec<LexComment>,
}

pub struct LexResult {
    pub tokens: Vec<Token>,
    pub comments: Vec<LexComment>,
}

pub fn lex_php(source: &str) -> Result<LexResult, LexError> {
    let mut lx = Lexer {
        src: source.as_bytes(),
        i: 0,
        line: 1,
        column: 1,
        tokens: Vec::new(),
        comments: Vec::new(),
    };
    lx.run()?;
    Ok(LexResult {
        tokens: lx.tokens,
        comments: lx.comments,
    })
}

impl<'a> Lexer<'a> {
    fn pos(&self) -> Pos {
        Pos::new(self.line, self.column, self.i)
    }

    fn advance(&mut self, n: usize) {
        let mut k = 0;
        while k < n && self.i < self.src.len() {
            if self.src[self.i] == b'\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
            self.i += 1;
            k += 1;
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.i..].starts_with(s.as_bytes())
    }

    fn slice(&self, from: usize, to: usize) -> String {
        String::from_utf8_lossy(&self.src[from..to]).into_owned()
    }

    fn run(&mut self) -> Result<(), LexError> {
        while self.i < self.src.len() {
            if self.starts_with("<?php") || self.starts_with("<?=") {
                let start = self.pos();
                let short_echo = self.starts_with("<?=");
                let tag_len = if short_echo { 3 } else { 5 };
                let mut tok = Token::new(
                    TokenKind::OpenTag,
                    self.slice(self.i, self.i + tag_len),
                    start,
                );
                tok.short_echo = short_echo;
                self.tokens.push(tok);
                self.advance(tag_len);
                self.lex_code()?;
                continue;
            }
            if self.starts_with("<?") {
                let start = self.pos();
                let tok = Token::new(TokenKind::OpenTag, "<?".to_string(), start);
                self.tokens.push(tok);
                self.advance(2);
                self.lex_code()?;
                continue;
            }
            // Inline HTML until the next open tag.
            let start = self.pos();
            let rest = &self.src[self.i..];
            let next_php = find_subslice(rest, b"<?").map(|p| p).unwrap_or(rest.len());
            let end = self.i + next_php;
            let text = self.slice(self.i, end);
            if !text.is_empty() {
                self.tokens
                    .push(Token::new(TokenKind::InlineHtml, text, start));
            }
            self.advance(end - self.i);
        }
        self.tokens
            .push(Token::new(TokenKind::Eof, String::new(), self.pos()));
        Ok(())
    }

    fn lex_code(&mut self) -> Result<(), LexError> {
        while self.i < self.src.len() {
            if self.starts_with("?>") {
                let start = self.pos();
                self.tokens
                    .push(Token::new(TokenKind::CloseTag, "?>".to_string(), start));
                self.advance(2);
                if self.src.get(self.i) == Some(&b'\r') && self.src.get(self.i + 1) == Some(&b'\n')
                {
                    self.advance(2);
                } else if self.src.get(self.i) == Some(&b'\n') {
                    self.advance(1);
                }
                return Ok(());
            }
            let ch = self.src[self.i];

            if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
                self.advance(1);
                continue;
            }

            // Comments (single-line ends at newline OR at `?>`).
            if self.starts_with("//") || self.starts_with("#") {
                let start = self.pos();
                let rest = &self.src[self.i..];
                let nl = find_subslice(rest, b"\n").unwrap_or(rest.len());
                let close_tag = find_subslice(rest, b"?>").unwrap_or(rest.len());
                let end = nl.min(close_tag);
                let text = self.slice(self.i, self.i + end);
                self.comments.push(LexComment { text, pos: start });
                self.advance(end);
                continue;
            }
            if self.starts_with("/*") {
                let start = self.pos();
                let rest = &self.src[self.i + 2..];
                match find_subslice(rest, b"*/") {
                    None => return Err(LexError::new("Unterminated block comment", start)),
                    Some(off) => {
                        let end = self.i + 2 + off + 2;
                        let text = self.slice(self.i, end);
                        self.comments.push(LexComment { text, pos: start });
                        self.advance(end - self.i);
                        continue;
                    }
                }
            }

            // Heredoc / nowdoc.
            if self.starts_with("<<<") {
                self.lex_heredoc()?;
                continue;
            }

            // Backtick execution.
            if ch == b'`' {
                let start = self.pos();
                self.advance(1);
                let mut raw = String::new();
                while self.i < self.src.len() && self.src[self.i] != b'`' {
                    let (c, len) = decode_char(self.src, self.i);
                    raw.push(c);
                    self.advance(len.max(1));
                }
                if self.i >= self.src.len() {
                    return Err(LexError::new("Unterminated backtick string", start));
                }
                self.advance(1);
                self.tokens
                    .push(Token::new(TokenKind::Backtick, raw, start));
                continue;
            }

            // Strings.
            if ch == b'\'' || ch == b'"' {
                self.lex_string()?;
                continue;
            }

            // Variables: $name.
            if ch == b'$' && self.i + 1 < self.src.len() && is_ident_start(self.src[self.i + 1]) {
                let start = self.pos();
                self.advance(1);
                let name_start = self.i;
                while self.i < self.src.len() && is_ident_char(self.src[self.i]) {
                    self.advance(1);
                }
                let name = self.slice(name_start, self.i);
                self.tokens.push(Token::new(TokenKind::Var, name, start));
                continue;
            }

            // Numbers.
            if is_digit(ch)
                || (ch == b'.' && self.i + 1 < self.src.len() && is_digit(self.src[self.i + 1]))
            {
                let start = self.pos();
                let text = self.scan_number();
                self.tokens.push(Token::new(TokenKind::Number, text, start));
                continue;
            }

            // Identifiers / keywords.
            if is_ident_start(ch) {
                let start = self.pos();
                let name_start = self.i;
                while self.i < self.src.len() && is_ident_char(self.src[self.i]) {
                    self.advance(1);
                }
                let name = self.slice(name_start, self.i);
                self.tokens.push(Token::new(TokenKind::Ident, name, start));
                continue;
            }

            // Multi-char punctuators (longest first).
            let mut matched = false;
            for p in MULTI_PUNCT {
                if self.starts_with(p) {
                    let start = self.pos();
                    self.tokens
                        .push(Token::new(TokenKind::Punct, p.to_string(), start));
                    self.advance(p.len());
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }

            // Single-char punctuators.
            if is_single_punct(ch) {
                let start = self.pos();
                self.tokens.push(Token::new(
                    TokenKind::Punct,
                    (ch as char).to_string(),
                    start,
                ));
                self.advance(1);
                continue;
            }

            return Err(LexError::new(
                format!(
                    "Unexpected character {}",
                    serde_json::to_string(&(ch as char).to_string()).unwrap_or_default()
                ),
                self.pos(),
            ));
        }
        Ok(())
    }

    /// Number regex equivalent:
    /// 0[xX][0-9a-fA-F]+ | 0[bB][01]+ | 0[oO][0-7]+ |
    /// (\d+\.?\d*|\.\d+)([eE][+-]?\d+)?
    fn scan_number(&mut self) -> String {
        let start = self.i;
        let rest = &self.src[self.i..];
        let mut n = 0usize;
        if rest.len() >= 2 && rest[0] == b'0' && (rest[1] == b'x' || rest[1] == b'X') {
            n = 2;
            while n < rest.len() && rest[n].is_ascii_hexdigit() {
                n += 1;
            }
        } else if rest.len() >= 2 && rest[0] == b'0' && (rest[1] == b'b' || rest[1] == b'B') {
            n = 2;
            while n < rest.len() && (rest[n] == b'0' || rest[n] == b'1') {
                n += 1;
            }
        } else if rest.len() >= 2 && rest[0] == b'0' && (rest[1] == b'o' || rest[1] == b'O') {
            n = 2;
            while n < rest.len() && (b'0'..=b'7').contains(&rest[n]) {
                n += 1;
            }
        } else {
            // (\d+\.?\d*|\.\d+)([eE][+-]?\d+)?
            if rest[0] == b'.' {
                n = 1;
                while n < rest.len() && rest[n].is_ascii_digit() {
                    n += 1;
                }
            } else {
                while n < rest.len() && rest[n].is_ascii_digit() {
                    n += 1;
                }
                if n < rest.len() && rest[n] == b'.' {
                    n += 1;
                    while n < rest.len() && rest[n].is_ascii_digit() {
                        n += 1;
                    }
                }
            }
            if n < rest.len() && (rest[n] == b'e' || rest[n] == b'E') {
                let mut m = n + 1;
                if m < rest.len() && (rest[m] == b'+' || rest[m] == b'-') {
                    m += 1;
                }
                if m < rest.len() && rest[m].is_ascii_digit() {
                    while m < rest.len() && rest[m].is_ascii_digit() {
                        m += 1;
                    }
                    n = m;
                }
            }
        }
        if n == 0 {
            n = 1; // single digit fallback (cannot happen: caller checked is_digit)
        }
        let text = self.slice(start, start + n);
        self.advance(n);
        text
    }

    fn lex_heredoc(&mut self) -> Result<(), LexError> {
        let start = self.pos();
        // Parse <<<LABEL / <<<"LABEL" / <<<'LABEL' then a newline.
        let rest = &self.src[self.i..];
        let mut j = 3usize;
        while j < rest.len() && (rest[j] == b' ' || rest[j] == b'\t') {
            j += 1;
        }
        let mut quote: Option<u8> = None;
        if j < rest.len() && (rest[j] == b'\'' || rest[j] == b'"') {
            quote = Some(rest[j]);
            j += 1;
        }
        if j >= rest.len() || !is_ident_start(rest[j]) {
            return Err(LexError::new("Invalid heredoc start", start));
        }
        let label_start = j;
        while j < rest.len() && is_ident_char(rest[j]) {
            j += 1;
        }
        let label = self.slice(self.i + label_start, self.i + j);
        if let Some(q) = quote {
            if j >= rest.len() || rest[j] != q {
                return Err(LexError::new("Invalid heredoc start", start));
            }
            j += 1;
        }
        // Must be followed by \r?\n.
        if j < rest.len() && rest[j] == b'\r' {
            j += 1;
        }
        if j >= rest.len() || rest[j] != b'\n' {
            return Err(LexError::new("Invalid heredoc start", start));
        }
        j += 1; // consume the newline
        let is_nowdoc = quote == Some(b'\'');
        self.advance(j);
        // Body runs until a line whose trimmed text is exactly the label
        // (PHP 7.3+ allows the closing marker to be indented).
        let body_start = self.i;
        let mut body_end: Option<usize> = None;
        let mut cursor = self.i;
        while cursor < self.src.len() {
            let nl = find_subslice(&self.src[cursor..], b"\n").map(|p| cursor + p);
            let line_end = nl.unwrap_or(self.src.len());
            let line_text = String::from_utf8_lossy(&self.src[cursor..line_end]);
            let trimmed = line_text.trim();
            if trimmed == label || trimmed == format!("{label};") {
                body_end = Some(cursor);
                break;
            }
            cursor = line_end + 1;
        }
        let body_end = match body_end {
            Some(e) => e,
            None => {
                return Err(LexError::new(
                    format!("Unterminated heredoc ({label})"),
                    start,
                ))
            }
        };
        let mut raw = self.slice(body_start, body_end);
        if raw.ends_with("\r\n") {
            raw.truncate(raw.len() - 2);
        } else if raw.ends_with('\n') {
            raw.pop();
        }
        // Consume the label line (but not its newline).
        self.advance(body_end - self.i + label.len());
        if is_nowdoc {
            let mut tok = Token::new(TokenKind::Heredoc, raw, start);
            tok.heredoc_label = Some(label);
            tok.is_nowdoc = true;
            self.tokens.push(tok);
        } else {
            let parts = split_interpolation(&raw);
            let has_complex = parts.iter().any(|p| p.kind == "complex");
            let value = if has_complex {
                raw.clone()
            } else {
                parts
                    .iter()
                    .map(|p| {
                        if p.kind == "text" {
                            p.value.clone()
                        } else {
                            format!("${{{}}}", p.value)
                        }
                    })
                    .collect::<String>()
            };
            let mut tok = Token::new(TokenKind::Heredoc, value, start);
            tok.parts = Some(parts);
            tok.heredoc_label = Some(label);
            self.tokens.push(tok);
        }
        Ok(())
    }

    fn lex_string(&mut self) -> Result<(), LexError> {
        let start = self.pos();
        let quote = self.src[self.i];
        self.advance(1);
        let mut raw = String::new();
        let mut closed = false;
        while self.i < self.src.len() {
            let c = self.src[self.i];
            if c == b'\\' && self.i + 1 < self.src.len() {
                raw.push('\\');
                let n = self.src[self.i + 1];
                raw.push(n as char);
                self.advance(2);
                continue;
            }
            if c == quote {
                closed = true;
                self.advance(1);
                break;
            }
            // Copy one char.
            let (ch, len) = decode_char(self.src, self.i);
            raw.push(ch);
            self.advance(len.max(1));
        }
        if !closed {
            return Err(LexError::new("Unterminated string literal", start));
        }
        if quote == b'\'' {
            let value = raw.replace("\\'", "'").replace("\\\\", "\\");
            self.tokens
                .push(Token::new(TokenKind::String, value, start));
        } else {
            let parts = split_interpolation(&raw);
            if parts.len() == 1 && parts[0].kind == "text" {
                self.tokens
                    .push(Token::new(TokenKind::String, parts[0].value.clone(), start));
            } else {
                let mut tok = Token::new(TokenKind::InterpString, raw, start);
                tok.parts = Some(parts);
                self.tokens.push(tok);
            }
        }
        Ok(())
    }
}

/// Decode one UTF-8 char at byte offset `i` (fast: inspects at most 4 bytes).
fn decode_char(src: &[u8], i: usize) -> (char, usize) {
    let b = src[i];
    if b < 0x80 {
        return (b as char, 1);
    }
    let len = if b >= 0xF0 {
        4
    } else if b >= 0xE0 {
        3
    } else {
        2
    };
    let end = (i + len).min(src.len());
    match std::str::from_utf8(&src[i..end]) {
        Ok(s) => match s.chars().next() {
            Some(c) => (c, c.len_utf8()),
            None => ('\u{FFFD}', 1),
        },
        Err(_) => ('\u{FFFD}', 1),
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}
