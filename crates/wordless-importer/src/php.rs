//! Safe PHP unserializer (port of v1 `src/modules/importer/php-serialize.ts`).
//!
//! WordPress stores PHP-serialized values in `wp_options` /
//! `wp_postmeta` / `wp_usermeta`. Wordless stores JSON instead, so the
//! importer converts them at import time. This parser handles
//! scalars, arrays, and nested structures — and NOTHING else:
//!
//! * `O:` (objects) and `C:` (custom serializable) are REJECTED —
//!   unserializing an object would be executing imported code.
//! * `r:` / `R:` (references) are rejected — no object graph to rebuild.
//! * `N;` (null), `b:` (bool), `i:` (int), `d:` (double), `s:`
//!   (string), `a:` (array) are supported, with strict length checks
//!   so a corrupt or hostile payload fails loudly instead of being
//!   half-parsed.
//!
//! A plain string that is not serialized at all is returned unchanged
//! (WordPress options are often plain strings). Values parse directly
//! into `serde_json::Value` (the JSON form Wordless stores); PHP
//! string lengths are byte lengths, measured in bytes and decoded as
//! UTF-8, as in v1. A nesting-depth cap (200) stops hostile
//! deeply-nested payloads from overflowing the stack — v1 has no cap,
//! this is a deliberate hardening difference, documented here.

use serde_json::Value;
use thiserror::Error;

/// Maximum nesting depth for arrays. v1 has no cap; this crate caps
/// at 200 so a hostile payload cannot overflow the stack.
pub const MAX_DEPTH: usize = 200;
/// Maximum array element count (same as v1).
pub const MAX_ARRAY_COUNT: i64 = 100_000;

#[derive(Debug, Error)]
#[error("{0}")]
pub struct PhpUnserializeError(pub String);

impl PhpUnserializeError {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
            depth: 0,
        }
    }

    fn parse(&mut self) -> Result<Value, PhpUnserializeError> {
        let value = self.parse_value()?;
        self.skip_ws();
        if self.pos != self.bytes.len() {
            return Err(PhpUnserializeError::new(format!(
                "Trailing data after value at offset {}.",
                self.pos
            )));
        }
        Ok(value)
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && (self.bytes[self.pos] as char).is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn expect(&mut self, ch: u8) -> Result<(), PhpUnserializeError> {
        if self.bytes.get(self.pos) != Some(&ch) {
            return Err(PhpUnserializeError::new(format!(
                "Expected \"{}\" at offset {}, found \"{}\".",
                ch as char,
                self.pos,
                self.bytes
                    .get(self.pos)
                    .map(|b| (*b as char).to_string())
                    .unwrap_or_default()
            )));
        }
        self.pos += 1;
        Ok(())
    }

    fn read_until(&mut self, delim: u8) -> Result<String, PhpUnserializeError> {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos] != delim {
            self.pos += 1;
        }
        if self.pos >= self.bytes.len() {
            return Err(PhpUnserializeError::new(format!(
                "Missing \"{}\" after offset {}.",
                delim as char, start
            )));
        }
        let out = String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned();
        self.pos += 1; // consume the delimiter
        Ok(out)
    }

    fn parse_value(&mut self) -> Result<Value, PhpUnserializeError> {
        let ty = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| PhpUnserializeError::new("Unexpected end of input."))?;
        self.pos += 1;
        match ty {
            b'N' => {
                self.expect(b';')?;
                Ok(Value::Null)
            }
            b'b' => {
                self.expect(b':')?;
                let raw = self.read_until(b';')?;
                match raw.as_str() {
                    "0" => Ok(Value::Bool(false)),
                    "1" => Ok(Value::Bool(true)),
                    _ => Err(PhpUnserializeError::new(format!(
                        "Invalid boolean \"{raw}\"."
                    ))),
                }
            }
            b'i' => {
                self.expect(b':')?;
                let raw = self.read_until(b';')?;
                let valid = {
                    let digits = raw.strip_prefix('-').unwrap_or(&raw);
                    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                };
                let n: i64 = raw.parse().unwrap_or(i64::MAX);
                if !valid || n == i64::MAX && raw != "9223372036854775807" {
                    return Err(PhpUnserializeError::new(format!(
                        "Invalid integer \"{raw}\"."
                    )));
                }
                Ok(Value::from(n))
            }
            b'd' => {
                self.expect(b':')?;
                let raw = self.read_until(b';')?;
                let n: f64 = raw.parse().unwrap_or(f64::NAN);
                if !n.is_finite() {
                    return Err(PhpUnserializeError::new(format!(
                        "Invalid double \"{raw}\"."
                    )));
                }
                Ok(serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or(Value::Null))
            }
            b's' => {
                self.expect(b':')?;
                let len_raw = self.read_until(b':')?;
                let valid = !len_raw.is_empty() && len_raw.bytes().all(|b| b.is_ascii_digit());
                let len: usize = len_raw.parse().unwrap_or(usize::MAX);
                if !valid || len == usize::MAX {
                    return Err(PhpUnserializeError::new(format!(
                        "Invalid string length \"{len_raw}\"."
                    )));
                }
                self.expect(b'"')?;
                // PHP string lengths are byte lengths; slice by byte
                // length, then decode as UTF-8 (fatal on invalid).
                if self.pos + len > self.bytes.len() {
                    return Err(PhpUnserializeError::new(format!(
                        "String at offset {} is longer than the input.",
                        self.pos
                    )));
                }
                let slice = &self.bytes[self.pos..self.pos + len];
                let s = std::str::from_utf8(slice).map_err(|_| {
                    PhpUnserializeError::new(format!(
                        "String at offset {} is not valid UTF-8.",
                        self.pos
                    ))
                })?;
                let s = s.to_string();
                self.pos += len;
                self.expect(b'"')?;
                self.expect(b';')?;
                Ok(Value::String(s))
            }
            b'a' => {
                self.expect(b':')?;
                let count_raw = self.read_until(b':')?;
                let valid =
                    !count_raw.is_empty() && count_raw.bytes().all(|b| b.is_ascii_digit());
                let count: i64 = count_raw.parse().unwrap_or(-1);
                if !valid || count < 0 || count > MAX_ARRAY_COUNT {
                    return Err(PhpUnserializeError::new(format!(
                        "Invalid array count \"{count_raw}\"."
                    )));
                }
                self.expect(b'{')?;
                if self.depth >= MAX_DEPTH {
                    return Err(PhpUnserializeError::new(
                        "Serialized value is nested too deeply.",
                    ));
                }
                self.depth += 1;
                let mut entries: Vec<(Value, Value)> = Vec::new();
                for _ in 0..count {
                    let key = self.parse_value()?;
                    let value = self.parse_value()?;
                    entries.push((key, value));
                }
                self.depth -= 1;
                self.expect(b'}')?;
                let is_list = entries
                    .iter()
                    .enumerate()
                    .all(|(i, (k, _))| k.as_i64() == Some(i as i64));
                if is_list {
                    Ok(Value::Array(entries.into_iter().map(|(_, v)| v).collect()))
                } else {
                    let mut obj = serde_json::Map::new();
                    for (k, v) in entries {
                        obj.insert(php_key_to_string(&k), v);
                    }
                    Ok(Value::Object(obj))
                }
            }
            b'O' | b'C' => Err(PhpUnserializeError::new(format!(
                "Serialized PHP objects ({}:) are refused — unserializing them would execute imported code.",
                ty as char
            ))),
            b'r' | b'R' => Err(PhpUnserializeError::new(
                "Serialized PHP references (r:/R:) are refused.",
            )),
            _ => Err(PhpUnserializeError::new(format!(
                "Unknown serialized type \"{}\" at offset {}.",
                ty as char,
                self.pos - 1
            ))),
        }
    }
}

/// PHP array-key → JSON object-key string, mirroring JS `String(k)`.
fn php_key_to_string(k: &Value) -> String {
    match k {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        _ => serde_json::to_string(k).unwrap_or_default(),
    }
}

/// Is this string a serialized PHP value (rather than a plain string)?
pub fn looks_serialized(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed == "N;"
        || (trimmed.len() >= 2
            && trimmed.as_bytes()[1] == b':'
            && trimmed.as_bytes()[0].is_ascii_alphabetic())
}

/// Unserialize a PHP value, or return the input unchanged when it is
/// not a serialized value at all. Throws [`PhpUnserializeError`] on
/// malformed or refused payloads.
pub fn safe_unserialize(input: &str) -> Result<Value, PhpUnserializeError> {
    if !looks_serialized(input) {
        // Not a serialized value — a plain string stays a plain string.
        return Ok(Value::String(input.to_string()));
    }
    Parser::new(input.trim()).parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_scalars_arrays_and_nested_structures() {
        assert_eq!(
            safe_unserialize("a:2:{s:5:\"color\";s:4:\"blue\";s:4:\"size\";i:3;}").unwrap(),
            json!({"color": "blue", "size": 3})
        );
        assert_eq!(safe_unserialize("i:42;").unwrap(), json!(42));
        assert_eq!(safe_unserialize("i:-7;").unwrap(), json!(-7));
        assert_eq!(safe_unserialize("d:3.14;").unwrap(), json!(3.14));
        assert_eq!(safe_unserialize("b:1;").unwrap(), json!(true));
        assert_eq!(safe_unserialize("b:0;").unwrap(), json!(false));
        assert_eq!(safe_unserialize("N;").unwrap(), Value::Null);
        assert_eq!(safe_unserialize("s:5:\"hello\";").unwrap(), json!("hello"));
        assert_eq!(
            safe_unserialize("a:2:{i:0;s:1:\"a\";i:1;s:1:\"b\";}").unwrap(),
            json!(["a", "b"])
        );
        assert_eq!(
            safe_unserialize("a:1:{s:3:\"key\";a:1:{s:1:\"x\";i:1;}}").unwrap(),
            json!({"key": {"x": 1}})
        );
    }

    #[test]
    fn returns_plain_strings_unchanged() {
        assert_eq!(
            safe_unserialize("just a string").unwrap(),
            json!("just a string")
        );
        assert_eq!(safe_unserialize("").unwrap(), json!(""));
        assert_eq!(safe_unserialize("blogname").unwrap(), json!("blogname"));
    }

    #[test]
    fn refuses_serialized_objects_and_references() {
        for bad in [
            "O:8:\"stdClass\":0:{}",
            "C:11:\"ArrayObject\":0:{}",
            "r:1;",
            "R:2;",
        ] {
            assert!(
                safe_unserialize(bad).is_err(),
                "{bad} must be refused, got {:?}",
                safe_unserialize(bad)
            );
        }
    }

    #[test]
    fn rejects_malformed_payloads_instead_of_half_parsing() {
        for bad in ["i:notanumber;", "s:10:\"short\";", "a:5:{i:0;i:1;}", "x:1;"] {
            assert!(
                safe_unserialize(bad).is_err(),
                "{bad} must be rejected, got {:?}",
                safe_unserialize(bad)
            );
        }
    }

    #[test]
    fn byte_length_strings_with_multibyte_utf8() {
        // "héllo" is 6 bytes in UTF-8, 5 chars — PHP counts bytes.
        assert_eq!(safe_unserialize("s:6:\"héllo\";").unwrap(), json!("héllo"));
        assert!(
            safe_unserialize("s:5:\"héllo\";").is_err(),
            "char count must not pass"
        );
    }
}
