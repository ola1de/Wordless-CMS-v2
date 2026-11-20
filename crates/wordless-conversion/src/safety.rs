//! Sandbox safety scan — a port of v1 `src/conversion/safety.ts`.
//!
//! The converter NEVER executes PHP and NEVER executes converted code at
//! conversion time, but the scan is the first line of defence for what
//! the converter will even translate: code that can run arbitrary
//! commands, load code dynamically, or unserialize untrusted data is
//! rejected outright — the job fails with a clear reason instead of
//! producing a partial, silently weakened translation.
//!
//! The scan runs on lexer tokens (not raw text), so a blocked word
//! inside a string literal or a comment does not trip it, and a blocked
//! call cannot hide behind odd spacing.

use serde::{Deserialize, Serialize};

use crate::lexer::{lex_php, TokenKind};

/// Blocked PHP function / construct names (case-insensitive).
const BLOCKED_CALLS: &[&str] = &[
    "eval",
    "exec",
    "shell_exec",
    "system",
    "passthru",
    "popen",
    "proc_open",
    "create_function",
    "unserialize",
    "mb_parse_str",
    "parse_str",
    "assert",
];

const DYNAMIC_LOADERS: &[&str] = &["require", "include", "require_once", "include_once"];

/// One safety violation found by the scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyViolation {
    pub construct: String,
    pub reason: String,
    pub line: usize,
    pub column: usize,
}

/// The result of [`scan_safety`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyResult {
    pub safe: bool,
    pub violations: Vec<SafetyViolation>,
}

/// Scan PHP source for blocked constructs. Returns every violation found
/// (not just the first) so the report can list them all.
///
/// A lex error is a parse failure, not a safety violation — the scan
/// reports "safe" and parsing will fail (same rule as v1).
pub fn scan_safety(source: &str) -> SafetyResult {
    let tokens = match lex_php(source) {
        Ok(r) => r.tokens,
        Err(_) => {
            return SafetyResult {
                safe: true,
                violations: Vec::new(),
            }
        }
    };
    let mut violations: Vec<SafetyViolation> = Vec::new();

    for (i, tok) in tokens.iter().enumerate() {
        // Backtick execution: `cmd`.
        if tok.kind == TokenKind::Backtick {
            violations.push(SafetyViolation {
                construct: "backtick execution".to_string(),
                reason: "Backtick shell execution (`` `cmd` ``) is blocked: the converter never translates code that can run shell commands.".to_string(),
                line: tok.pos.line,
                column: tok.pos.column,
            });
            continue;
        }

        if tok.kind != TokenKind::Ident {
            continue;
        }
        let name = tok.value.to_lowercase();

        // Blocked function calls: name followed by `(`.
        if BLOCKED_CALLS.contains(&name.as_str()) {
            let next = tokens.get(i + 1);
            if let Some(next) = next {
                if next.kind == TokenKind::Punct && next.value == "(" {
                    let reason = if name == "unserialize" {
                        "unserialize() is blocked: PHP object injection. Import data as JSON instead (the WordPress importer unserializes safely at import time).".to_string()
                    } else if name == "eval" || name == "create_function" {
                        format!(
                            "{}() is blocked: it executes dynamically generated code, which the Wordless sandbox never allows.",
                            tok.value
                        )
                    } else if name == "parse_str" || name == "mb_parse_str" {
                        format!(
                            "{}() is blocked: it writes variables into scope from untrusted input. Parse query strings explicitly instead.",
                            tok.value
                        )
                    } else if name == "assert" {
                        "assert() is blocked: it can execute a string as PHP code.".to_string()
                    } else {
                        format!(
                            "{}() is blocked: it runs shell commands, which the Wordless sandbox never allows.",
                            tok.value
                        )
                    };
                    violations.push(SafetyViolation {
                        construct: format!("{}()", tok.value),
                        reason,
                        line: tok.pos.line,
                        column: tok.pos.column,
                    });
                    continue;
                }
            }
        }

        // Dynamic include/require: the argument must be a string literal.
        if DYNAMIC_LOADERS.contains(&name.as_str()) {
            let mut j = i + 1;
            if let Some(t) = tokens.get(j) {
                if t.kind == TokenKind::Punct && t.value == "(" {
                    j += 1;
                }
            }
            let arg = tokens.get(j);
            let is_literal = match arg {
                Some(t) => {
                    (t.kind == TokenKind::String && !t.value.is_empty())
                        || (t.kind == TokenKind::Ident && t.value == "__FILE__")
                }
                None => false,
            };
            if !is_literal {
                violations.push(SafetyViolation {
                    construct: format!("{} (dynamic path)", tok.value),
                    reason: format!(
                        "{} with a non-literal path is blocked: converted extensions may only load files whose path is known at conversion time.",
                        tok.value
                    ),
                    line: tok.pos.line,
                    column: tok.pos.column,
                });
            }
        }
    }

    SafetyResult {
        safe: violations.is_empty(),
        violations,
    }
}

/// The contract-level safety check: the blocking reasons for a source
/// file, empty when safe. Each reason names the line and the construct,
/// in the same wording as v1's `assertSafe` error.
pub fn check_safety(source: &str) -> Vec<String> {
    scan_safety(source)
        .violations
        .iter()
        .map(|v| format!("line {}: {} — {}", v.line, v.construct, v.reason))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_blocked_constructs() {
        let cases: &[&str] = &[
            "<?php eval($code);",
            "<?php exec('ls');",
            "<?php $x = shell_exec('id');",
            "<?php system('whoami');",
            "<?php passthru('cat /etc/passwd');",
            "<?php $f = create_function('$a', 'return $a;');",
            "<?php $x = unserialize($_POST['data']);",
            "<?php $x = `whoami`;",
            "<?php include $file;",
            "<?php require($path . '.php');",
        ];
        for src in cases {
            let result = scan_safety(src);
            assert!(!result.safe, "{src}");
            assert!(!result.violations.is_empty(), "{src}");
        }
        // check_safety (the contract fn) agrees.
        for src in cases {
            assert!(!check_safety(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn allows_literal_includes_and_normal_plugin_code() {
        let src = r#"<?php
require_once 'inc/helpers.php';
function hello_dolly() {
  $lyrics = explode("\n", "a\nb");
  return $lyrics[0];
}
add_action('admin_notices', 'hello_dolly');
"#;
        assert!(scan_safety(src).safe);
        assert!(check_safety(src).is_empty());
    }

    #[test]
    fn does_not_trip_on_blocked_words_inside_strings_or_comments() {
        let src = r#"<?php
// This comment mentions eval and system() for documentation.
// $x = "exec('ls') is just a string";
$a = 'unserialize is a word';
"#;
        assert!(scan_safety(src).safe);
    }

    #[test]
    fn reports_the_line_of_each_violation() {
        let result = scan_safety("<?php\n$a = 1;\n$x = eval($code);\n$y = system('id');\n");
        assert_eq!(result.violations.len(), 2);
        assert_eq!(result.violations[0].line, 3);
        assert_eq!(result.violations[1].line, 4);
    }

    #[test]
    fn is_case_insensitive_for_blocked_names() {
        assert!(!scan_safety("<?php EVAL($x);").safe);
        assert!(!scan_safety("<?php Shell_Exec('id');").safe);
    }

    #[test]
    fn lex_error_is_not_a_safety_violation() {
        // Unterminated string: the lexer fails, so the scan reports
        // safe and lets the parser report the failure.
        assert!(scan_safety("<?php $x = 'unterminated;").safe);
    }
}
