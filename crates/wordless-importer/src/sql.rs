//! mysqldump parser (port of v1 `src/modules/importer/sql-parser.ts`).
//!
//! Parses `INSERT INTO <table> [(cols)] VALUES (...), (...);`
//! statements from a mysqldump-style file. Only the 12 single-site
//! `wp_*` tables are returned; other tables are reported as ignored.
//! CREATE TABLE, SET, LOCK/UNLOCK and comment lines are skipped.
//!
//! SQL string literals are decoded with MySQL backslash escapes
//! (`\'`, `\"`, `\\`, `\n`, `\r`, `\t`, `\0`, `\b`, `\Z`, `\%`, `\_`)
//! and `''` → `'`. NULL becomes null; bare numbers become numbers.
//! Anything else that does not parse is a hard error — a half-parsed
//! dump is never imported.

use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("{0}")]
pub struct SqlParseError(pub String);

/// One parsed row: its table plus column → value.
#[derive(Debug, Clone)]
pub struct SqlRow {
    pub table: String,
    pub values: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct SqlParseResult {
    pub rows: Vec<SqlRow>,
    pub ignored_tables: Vec<String>,
    pub statements: usize,
}

/// Canonical column order per table (wp-admin/includes/schema.php,
/// WordPress 7.1.5) — same as v1 `WP_TABLE_COLUMNS`.
pub fn wp_table_columns(table: &str) -> Option<&'static [&'static str]> {
    Some(match table {
        "wp_posts" => &[
            "ID",
            "post_author",
            "post_date",
            "post_date_gmt",
            "post_content",
            "post_title",
            "post_excerpt",
            "post_status",
            "comment_status",
            "ping_status",
            "post_password",
            "post_name",
            "post_modified",
            "post_modified_gmt",
            "post_content_filtered",
            "post_parent",
            "guid",
            "menu_order",
            "post_type",
            "post_mime_type",
            "comment_count",
        ],
        "wp_postmeta" => &["meta_id", "post_id", "meta_key", "meta_value"],
        "wp_users" => &[
            "ID",
            "user_login",
            "user_pass",
            "user_nicename",
            "user_email",
            "user_url",
            "user_registered",
            "user_activation_key",
            "user_status",
            "display_name",
        ],
        "wp_usermeta" => &["umeta_id", "user_id", "meta_key", "meta_value"],
        "wp_terms" => &["term_id", "name", "slug", "term_group"],
        "wp_term_taxonomy" => &[
            "term_taxonomy_id",
            "term_id",
            "taxonomy",
            "description",
            "parent",
            "count",
        ],
        "wp_term_relationships" => &["object_id", "term_taxonomy_id", "term_order"],
        "wp_comments" => &[
            "comment_ID",
            "comment_post_ID",
            "comment_author",
            "comment_author_email",
            "comment_author_url",
            "comment_author_IP",
            "comment_date",
            "comment_date_gmt",
            "comment_content",
            "comment_karma",
            "comment_approved",
            "comment_agent",
            "comment_type",
            "comment_parent",
            "user_id",
        ],
        "wp_commentmeta" => &["meta_id", "comment_id", "meta_key", "meta_value"],
        "wp_links" => &[
            "link_id",
            "link_url",
            "link_name",
            "link_image",
            "link_target",
            "link_description",
            "link_visible",
            "link_owner",
            "link_rating",
            "link_updated",
            "link_rel",
            "link_notes",
            "link_rss",
        ],
        "wp_options" => &["option_id", "option_name", "option_value", "autoload"],
        "wp_termmeta" => &["meta_id", "term_id", "meta_key", "meta_value"],
        _ => return None,
    })
}

pub fn is_wp_table(table: &str) -> bool {
    wp_table_columns(table).is_some()
}

fn unescape_char(c: char) -> char {
    match c {
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        '0' => '\0',
        'b' => '\u{0008}',
        'Z' => '\u{001a}',
        _ => c, // ' " \ % _ and anything else: the char itself.
    }
}

/// Split the dump into top-level statements (respecting quotes and comments).
pub fn split_statements(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if ch == '*' && next == Some('/') {
                in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if !in_single && !in_double && !in_backtick {
            if ch == '-' && next == Some('-') && matches!(chars.get(i + 2), Some(' ') | Some('\t'))
            {
                in_line_comment = true;
                i += 1;
                continue;
            }
            if ch == '#' {
                in_line_comment = true;
                i += 1;
                continue;
            }
            if ch == '/' && next == Some('*') {
                in_block_comment = true;
                i += 2;
                continue;
            }
        }
        if ch == '\\' && (in_single || in_double) {
            current.push(ch);
            if let Some(n) = next {
                current.push(n);
            }
            i += 2;
            continue;
        }
        if ch == '\'' && !in_double && !in_backtick {
            in_single = !in_single;
        } else if ch == '"' && !in_single && !in_backtick {
            in_double = !in_double;
        } else if ch == '`' && !in_single && !in_double {
            in_backtick = !in_backtick;
        }
        current.push(ch);
        if ch == ';' && !in_single && !in_double && !in_backtick {
            statements.push(std::mem::take(&mut current));
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        statements.push(current);
    }
    statements
}

#[derive(Debug, Clone)]
enum Token {
    Str(String),
    Number(String),
    Null,
    Word(String),
}

/// Tokenize the parenthesised value list of one row, starting at the `(`.
fn tokenize_values(text: &[char], start: usize) -> Result<(Vec<Token>, usize), SqlParseError> {
    let mut values = Vec::new();
    let mut i = start;
    let err = |msg: String| SqlParseError(msg);
    let skip_ws = |i: &mut usize| {
        while *i < text.len() && text[*i].is_whitespace() {
            *i += 1;
        }
    };
    skip_ws(&mut i);
    if text.get(i) != Some(&'(') {
        return Err(err(format!("Expected \"(\" at offset {i} in VALUES list.")));
    }
    i += 1;
    loop {
        skip_ws(&mut i);
        let ch = text.get(i).copied();
        let Some(ch) = ch else {
            return Err(err("Unexpected end of VALUES list.".to_string()));
        };
        if ch == '\'' {
            i += 1;
            let mut out = String::new();
            loop {
                let c = text.get(i).copied();
                let Some(c) = c else {
                    return Err(err(
                        "Unterminated string literal in VALUES list.".to_string()
                    ));
                };
                if c == '\'' {
                    if text.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                if c == '\\' {
                    let esc = text.get(i + 1).copied();
                    let Some(esc) = esc else {
                        return Err(err("Trailing backslash in string literal.".to_string()));
                    };
                    out.push(unescape_char(esc));
                    i += 2;
                    continue;
                }
                out.push(c);
                i += 1;
            }
            values.push(Token::Str(out));
        } else if ch == 'N' && text[i..].iter().take(4).collect::<String>() == "NULL" {
            values.push(Token::Null);
            i += 4;
        } else {
            // Bare number (or TRUE/FALSE, treated as a word).
            let mut j = i;
            while j < text.len() && !(text[j] == ',' || text[j].is_whitespace() || text[j] == ')') {
                j += 1;
            }
            let word: String = text[i..j].iter().collect();
            if word.is_empty() {
                return Err(err(format!(
                    "Unexpected character \"{ch}\" in VALUES list."
                )));
            }
            let is_num = {
                let digits = word.strip_prefix('-').unwrap_or(&word);
                let (int_part, frac) = match digits.split_once('.') {
                    Some((a, b)) => (a, Some(b)),
                    None => (digits, None),
                };
                !int_part.is_empty()
                    && int_part.bytes().all(|b| b.is_ascii_digit())
                    && frac.map_or(true, |f| {
                        !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit())
                    })
            };
            values.push(if is_num {
                Token::Number(word)
            } else {
                Token::Word(word)
            });
            i = j;
        }
        skip_ws(&mut i);
        match text.get(i).copied() {
            Some(',') => {
                i += 1;
                continue;
            }
            Some(')') => {
                i += 1;
                return Ok((values, i));
            }
            other => {
                return Err(err(format!(
                    "Expected \",\" or \")\" at offset {i} in VALUES list, found \"{}\".",
                    other.map(|c| c.to_string()).unwrap_or_default()
                )));
            }
        }
    }
}

/// Parse one INSERT statement. Returns `None` for non-INSERT
/// statements; for an unknown table returns empty rows (caller
/// records it as ignored — its VALUES are never parsed).
fn parse_insert(
    statement: &str,
) -> Result<Option<(String, Vec<Map<String, Value>>)>, SqlParseError> {
    let upper = statement.trim_start();
    // Case-insensitive `INSERT INTO <table> [(cols)] VALUES` head,
    // matched by hand (no regex crate in this workspace).
    let rest = match strip_keyword_ci(upper, "INSERT") {
        Some(r) => r,
        None => return Ok(None),
    };
    let rest = match strip_keyword_ci(rest.trim_start(), "INTO") {
        Some(r) => r,
        None => return Ok(None),
    };
    let rest = rest.trim_start();
    let (table, idx) = read_table_name(rest);
    if table.is_empty() {
        return Ok(None);
    }
    let table = table.to_lowercase();
    if !is_wp_table(&table) {
        return Ok(Some((table, Vec::new())));
    }
    let chars: Vec<char> = statement.chars().collect();
    // Re-locate the head end inside the original statement: the
    // table name we read sits at `statement.len() - rest.len() + idx`
    // only when the head had no leading whitespace; recompute by
    // scanning for VALUES instead — simpler and robust.
    let stmt_upper = statement.to_uppercase();
    let values_at = stmt_upper
        .find("VALUES")
        .ok_or_else(|| SqlParseError(format!("INSERT into {table} has no VALUES.")))?;
    // Columns: text between the table name and VALUES, if parenthesised.
    let head_region = &statement[..values_at];
    let columns: Vec<String> = match (head_region.find('('), head_region.rfind(')')) {
        (Some(open), Some(close)) if close > open => head_region[open + 1..close]
            .split(',')
            .map(|c| c.trim().trim_matches('`').to_string())
            .collect(),
        _ => wp_table_columns(&table)
            .unwrap_or(&[])
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    let _ = idx;
    let mut rows: Vec<Map<String, Value>> = Vec::new();
    let mut i = values_at + "VALUES".len();
    loop {
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        if i >= chars.len() || chars[i] == ';' {
            break;
        }
        if chars[i] != '(' {
            return Err(SqlParseError(format!(
                "Expected \"(\" at offset {i} in INSERT into {table}."
            )));
        }
        let (tokens, end) = tokenize_values(&chars, i)?;
        i = end;
        if tokens.len() != columns.len() {
            return Err(SqlParseError(format!(
                "INSERT into {table} has {} values but {} columns (statement: {}…).",
                tokens.len(),
                columns.len(),
                statement.chars().take(80).collect::<String>()
            )));
        }
        let mut record = Map::new();
        for (col, token) in columns.iter().zip(tokens.iter()) {
            let value = match token {
                Token::Null => Value::Null,
                Token::Number(n) => {
                    if let Ok(i) = n.parse::<i64>() {
                        Value::from(i)
                    } else if let Ok(f) = n.parse::<f64>() {
                        serde_json::Number::from_f64(f)
                            .map(Value::Number)
                            .unwrap_or(Value::Null)
                    } else {
                        Value::String(n.clone())
                    }
                }
                Token::Str(s) | Token::Word(s) => Value::String(s.clone()),
            };
            record.insert(col.clone(), value);
        }
        rows.push(record);
    }
    Ok(Some((table, rows)))
}

/// Match a keyword at the start of `s` (case-insensitive), followed by
/// whitespace; returns the remainder, or `None`.
fn strip_keyword_ci<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    if s.len() < keyword.len() {
        return None;
    }
    if !s[..keyword.len()].eq_ignore_ascii_case(keyword) {
        return None;
    }
    let after = &s[keyword.len()..];
    if after.is_empty() || after.chars().next().is_some_and(|c| c.is_whitespace()) {
        Some(after)
    } else {
        None
    }
}

/// Read a (possibly backticked) table name from the start of `s`;
/// returns the name and the byte offset just past it.
fn read_table_name(s: &str) -> (String, usize) {
    let s = s.trim_start();
    let leading_ws = s.len() - s.trim_start().len();
    let _ = leading_ws;
    if let Some(rest) = s.strip_prefix('`') {
        match rest.find('`') {
            Some(end) => (rest[..end].to_string(), end + 2),
            None => (rest.to_string(), rest.len()),
        }
    } else {
        let end = s
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(s.len());
        (s[..end].to_string(), end)
    }
}

pub fn parse_sql_dump(sql: &str) -> Result<SqlParseResult, SqlParseError> {
    let statements = split_statements(sql);
    let mut rows: Vec<SqlRow> = Vec::new();
    let mut ignored: Vec<String> = Vec::new();
    for statement in &statements {
        let Some((table, records)) = parse_insert(statement)? else {
            continue;
        };
        if !is_wp_table(&table) {
            if !ignored.contains(&table) {
                ignored.push(table);
            }
            continue;
        }
        for record in records {
            rows.push(SqlRow {
                table: table.clone(),
                values: record,
            });
        }
    }
    ignored.sort();
    Ok(SqlParseResult {
        rows,
        ignored_tables: ignored,
        statements: statements.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str =
        include_str!("../test-fixtures/wordpress-dump.sql");

    #[test]
    fn parses_the_fixture_dump() {
        let parsed = parse_sql_dump(FIXTURE).unwrap();
        assert!(parsed.statements > 0);
        assert!(parsed
            .ignored_tables
            .contains(&"wp_custom_plugin".to_string()));
        let users: Vec<&SqlRow> = parsed
            .rows
            .iter()
            .filter(|r| r.table == "wp_users")
            .collect();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].values["user_login"], "fixture-admin");
        let comments: Vec<&SqlRow> = parsed
            .rows
            .iter()
            .filter(|r| r.table == "wp_comments")
            .collect();
        assert_eq!(comments.len(), 2);
        assert_eq!(
            comments[0].values["comment_content"],
            "It's a fixture, and it works."
        );
        assert_eq!(comments[0].values["comment_approved"], "1");
        assert!(
            parsed
                .rows
                .iter()
                .any(|r| r.table == "wp_options"
                    && r.values["option_name"] == "fixture_object_option")
        );
    }

    #[test]
    fn handles_explicit_column_list_and_null_values() {
        let parsed = parse_sql_dump(
            "INSERT INTO `wp_options` (`option_name`, `option_value`, `autoload`) VALUES ('a', NULL, 'yes'), ('b', 'x', 'no');",
        )
        .unwrap();
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].values["option_name"], "a");
        assert_eq!(parsed.rows[0].values["option_value"], Value::Null);
        assert_eq!(parsed.rows[0].values["autoload"], "yes");
        assert_eq!(parsed.rows[1].values["option_value"], "x");
    }

    #[test]
    fn malformed_insert_is_a_hard_error() {
        assert!(parse_sql_dump("INSERT INTO wp_options VALUES ('a', 'b');").is_err());
    }
}
