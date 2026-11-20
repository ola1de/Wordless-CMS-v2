//! Pure grading + timing logic for CBT — no database, no I/O, so
//! every rule carries a unit test here (doc §3 P9: "grading is pure
//! logic under `cargo test` with a case per question type").
//!
//! Rules (decided 2026-11-20, doc §3 P9 outcome block):
//! - mcq / true-false: full marks or 0.
//! - multi-select: partial credit `(correct selected − incorrect
//!   selected) / total correct`, clamped at ≥ 0, × marks. No negative
//!   marking. Duplicated selections count once.
//! - short-answer: NEVER auto-graded — `grade_objective` returns
//!   `None` and the answer waits in the manual grading queue.
//! - Scores are rounded to 2 decimal places on write.

use sha2::{Digest, Sha256};

/// Round to 2 decimal places (half away from zero, the same rule the
/// Woo quote math uses).
pub fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Grade one objective answer. Returns `None` when the question type
/// is not auto-gradable (`short_answer`) or when the stored answer
/// KEY is malformed for the type (corrupt data — grading cannot
/// proceed). A malformed SUBMITTED answer scores 0 — it is a wrong
/// answer, not a crash.
/// Returns `(is_correct, marks_awarded)`.
pub fn grade_objective(
    qtype: &str,
    answer_key: &serde_json::Value,
    answer: &serde_json::Value,
    marks: i64,
) -> Option<(bool, f64)> {
    let marks = marks as f64;
    match qtype {
        "mcq" => {
            let key = answer_key.as_i64()?;
            let given = match answer.as_i64() {
                Some(i) => i,
                None => return Some((false, 0.0)),
            };
            if given == key {
                Some((true, marks))
            } else {
                Some((false, 0.0))
            }
        }
        "true_false" => {
            let key = answer_key.as_bool()?;
            let given = match answer.as_bool() {
                Some(b) => b,
                None => return Some((false, 0.0)),
            };
            if given == key {
                Some((true, marks))
            } else {
                Some((false, 0.0))
            }
        }
        "multi_select" => {
            let key = answer_key.as_array()?;
            let given = match answer.as_array() {
                Some(a) => a,
                None => return Some((false, 0.0)),
            };
            let mut correct_total: Vec<i64> = key.iter().filter_map(|v| v.as_i64()).collect();
            correct_total.sort_unstable();
            correct_total.dedup();
            if correct_total.is_empty() {
                return Some((false, 0.0));
            }
            // A non-integer element makes the whole answer wrong.
            let mut given_set: Vec<i64> = Vec::new();
            for v in given {
                match v.as_i64() {
                    Some(i) => given_set.push(i),
                    None => return Some((false, 0.0)),
                }
            }
            given_set.sort_unstable();
            given_set.dedup();
            let correct_selected = given_set
                .iter()
                .filter(|g| correct_total.contains(g))
                .count() as f64;
            let incorrect_selected = given_set
                .iter()
                .filter(|g| !correct_total.contains(g))
                .count() as f64;
            let fraction =
                ((correct_selected - incorrect_selected) / correct_total.len() as f64).max(0.0);
            let awarded = round2(fraction * marks);
            Some((awarded >= marks, awarded))
        }
        // Short-answer is manual-graded only — never auto-graded.
        "short_answer" => None,
        _ => None,
    }
}

/// Percentage score, 2 dp. `max_score` of 0 is 0% (an exam with no
/// questions cannot be passed).
pub fn percent(score: f64, max_score: i64) -> f64 {
    if max_score <= 0 {
        return 0.0;
    }
    round2(score / max_score as f64 * 100.0)
}

/// Deterministic shuffle (Fisher–Yates over a xorshift64* PRNG seeded
/// from the attempt id). The same attempt id always yields the same
/// order, so a re-fetch of the attempt is stable — and the order is
/// stored on the attempt row anyway, so the shuffle only runs once,
/// at start.
pub fn shuffle_order(ids: &[i64], seed: u64) -> Vec<i64> {
    let mut out: Vec<i64> = ids.to_vec();
    let mut state = if seed == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        seed
    };
    // xorshift64*: one step per swap, good enough for ordering.
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let n = out.len();
    if n > 1 {
        for i in (1..n).rev() {
            let j = (next() % ((i + 1) as u64)) as usize;
            out.swap(i, j);
        }
    }
    out
}

/// Certificate verification code: `WL-` + the first 16 hex chars of
/// SHA-256 over (attempt, user, exam, issued-at-ms). Deterministic,
/// so re-issuing the same attempt yields the same code — and unique
/// per attempt, because the attempt id is in the input.
pub fn certificate_code(attempt_id: i64, user_id: i64, exam_id: i64, issued_at_ms: u64) -> String {
    let input = format!("wordless-cert|{attempt_id}|{user_id}|{exam_id}|{issued_at_ms}");
    let hash = Sha256::digest(input.as_bytes());
    let hex: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("WL-{hex}")
}

/// Server-side deadline: start + duration. The deadline is computed
/// once at start and stored — never recomputed from a client clock.
pub fn deadline_ms(started_at_ms: u64, duration_minutes: i64) -> u64 {
    started_at_ms.saturating_add((duration_minutes.max(0) as u64).saturating_mul(60_000))
}

/// Validate a submitted answer's SHAPE for its question type (pure
/// logic, no grading): mcq = an integer index inside the options;
/// true_false = a boolean; multi_select = a list of indices inside
/// the options; short_answer = a string (empty allowed — a blank
/// answer is a wrong answer, not an error). Returns an error message
/// for the caller to surface as a 400.
pub fn validate_answer_shape(
    qtype: &str,
    options: &[String],
    answer: &serde_json::Value,
) -> Result<(), String> {
    match qtype {
        "mcq" => match answer.as_i64() {
            Some(i) if i >= 0 && (i as usize) < options.len() => Ok(()),
            _ => Err("The answer must be the index of one of the options.".to_string()),
        },
        "true_false" => match answer.as_bool() {
            Some(_) => Ok(()),
            None => Err("The answer must be true or false.".to_string()),
        },
        "multi_select" => match answer.as_array() {
            Some(arr)
                if arr.iter().all(
                    |v| matches!(v.as_i64(), Some(i) if i >= 0 && (i as usize) < options.len()),
                ) =>
            {
                Ok(())
            }
            _ => Err("The answer must be a list of option indices.".to_string()),
        },
        "short_answer" => match answer.as_str() {
            Some(_) => Ok(()),
            None => Err("The answer must be text.".to_string()),
        },
        _ => Err("Unknown question type.".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mcq_full_marks_or_zero() {
        assert_eq!(
            grade_objective("mcq", &json!(2), &json!(2), 5),
            Some((true, 5.0))
        );
        assert_eq!(
            grade_objective("mcq", &json!(2), &json!(0), 5),
            Some((false, 0.0))
        );
        // A non-integer answer is a wrong answer, not a crash.
        assert_eq!(
            grade_objective("mcq", &json!(2), &json!("2"), 5),
            Some((false, 0.0))
        );
    }

    #[test]
    fn true_false_full_marks_or_zero() {
        assert_eq!(
            grade_objective("true_false", &json!(true), &json!(true), 2),
            Some((true, 2.0))
        );
        assert_eq!(
            grade_objective("true_false", &json!(true), &json!(false), 2),
            Some((false, 0.0))
        );
    }

    #[test]
    fn multi_select_full_credit() {
        // All correct, none wrong: full marks.
        assert_eq!(
            grade_objective("multi_select", &json!([0, 2]), &json!([0, 2]), 4),
            Some((true, 4.0))
        );
        // Order does not matter.
        assert_eq!(
            grade_objective("multi_select", &json!([0, 2]), &json!([2, 0]), 4),
            Some((true, 4.0))
        );
        // Duplicates count once.
        assert_eq!(
            grade_objective("multi_select", &json!([0, 2]), &json!([0, 0, 2, 2]), 4),
            Some((true, 4.0))
        );
    }

    #[test]
    fn multi_select_partial_credit() {
        // 1 of 2 correct, none wrong: half the marks.
        assert_eq!(
            grade_objective("multi_select", &json!([0, 2]), &json!([0]), 4),
            Some((false, 2.0))
        );
        // 1 correct + 1 wrong of 2: (1-1)/2 = 0.
        assert_eq!(
            grade_objective("multi_select", &json!([0, 2]), &json!([0, 1]), 4),
            Some((false, 0.0))
        );
        // 2 correct + 1 wrong of 3: (2-1)/3 = 1/3 of 6 = 2.0.
        assert_eq!(
            grade_objective("multi_select", &json!([0, 1, 2]), &json!([0, 1, 3]), 6),
            Some((false, 2.0))
        );
        // All wrong: clamped at 0, never negative.
        assert_eq!(
            grade_objective("multi_select", &json!([0]), &json!([1, 2, 3]), 4),
            Some((false, 0.0))
        );
    }

    #[test]
    fn short_answer_is_never_auto_graded() {
        assert_eq!(
            grade_objective("short_answer", &json!(null), &json!("anything"), 5),
            None
        );
        assert_eq!(
            grade_objective("short_answer", &json!(null), &json!(""), 5),
            None
        );
    }

    #[test]
    fn unknown_type_is_not_graded() {
        assert_eq!(grade_objective("essay", &json!(null), &json!("x"), 5), None);
    }

    #[test]
    fn percent_rules() {
        assert_eq!(percent(8.0, 10), 80.0);
        assert_eq!(percent(1.0, 3), 33.33);
        assert_eq!(percent(2.0, 3), 66.67);
        assert_eq!(percent(0.0, 0), 0.0);
        assert_eq!(percent(5.0, 0), 0.0);
    }

    #[test]
    fn shuffle_is_deterministic_and_a_permutation() {
        let ids = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let a = shuffle_order(&ids, 42);
        let b = shuffle_order(&ids, 42);
        assert_eq!(a, b, "same seed must give the same order");
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, ids, "shuffle must be a permutation");
        // A different seed (usually) gives a different order — and a
        // zero seed is still valid.
        let _ = shuffle_order(&ids, 0);
        assert_eq!(shuffle_order(&[], 7), Vec::<i64>::new());
        assert_eq!(shuffle_order(&[9], 7), vec![9]);
    }

    #[test]
    fn certificate_code_shape_and_determinism() {
        let code = certificate_code(12, 3, 5, 1_795_000_000_000);
        assert!(code.starts_with("WL-"), "code: {code}");
        assert_eq!(code.len(), 19, "WL- + 16 hex chars, got {code}");
        assert!(code[3..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(certificate_code(12, 3, 5, 1_795_000_000_000), code);
        assert_ne!(certificate_code(13, 3, 5, 1_795_000_000_000), code);
        assert_ne!(certificate_code(12, 4, 5, 1_795_000_000_000), code);
    }

    #[test]
    fn deadline_is_start_plus_duration() {
        assert_eq!(deadline_ms(1_000_000, 30), 1_000_000 + 30 * 60_000);
        assert_eq!(deadline_ms(1_000_000, 0), 1_000_000);
    }

    #[test]
    fn answer_shape_validation() {
        let opts = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(validate_answer_shape("mcq", &opts, &json!(1)).is_ok());
        assert!(validate_answer_shape("mcq", &opts, &json!(3)).is_err());
        assert!(validate_answer_shape("mcq", &opts, &json!("1")).is_err());
        assert!(validate_answer_shape("true_false", &opts, &json!(true)).is_ok());
        assert!(validate_answer_shape("true_false", &opts, &json!(1)).is_err());
        assert!(validate_answer_shape("multi_select", &opts, &json!([0, 2])).is_ok());
        assert!(validate_answer_shape("multi_select", &opts, &json!([0, 3])).is_err());
        assert!(validate_answer_shape("multi_select", &opts, &json!([0, 0])).is_ok());
        assert!(validate_answer_shape("short_answer", &opts, &json!("Lagos")).is_ok());
        assert!(validate_answer_shape("short_answer", &opts, &json!("")).is_ok());
        assert!(validate_answer_shape("short_answer", &opts, &json!(4)).is_err());
    }
}
