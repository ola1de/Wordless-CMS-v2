//! Question banks and questions.
//!
//! Validation rules (enforced here, not just in the route layer):
//! - `mcq`: at least 2 options; the answer key is one option index
//!   inside the options.
//! - `true_false`: options are fixed to `["True", "False"]`; the key
//!   is a boolean.
//! - `multi_select`: at least 2 options; the key is a non-empty list
//!   of distinct indices inside the options.
//! - `short_answer`: no options, no key — the reference answer lives
//!   in `model_answer` and is shown to the human grader only.
//! - `marks` ≥ 1. `difficulty` is free text, default `medium`.
//!
//! The answer key is NEVER serialised for students: `to_student`
//! strips it (and the model answer). The instructor/admin read
//! carries the key — teachers need it to review their own bank.

use serde_json::json;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::courses::require_manage_course;
use crate::types::{Question, QuestionBank};

pub const QUESTION_TYPES: [&str; 4] = ["mcq", "true_false", "multi_select", "short_answer"];

/// Validate a question's options + answer key for its type, and
/// return the canonical (options_json, answer_json, model_answer)
/// triple to store. `true_false` options are forced to
/// `["True", "False"]` whatever the caller sent.
pub fn validate_question(
    qtype: &str,
    options: &[String],
    answer: &serde_json::Value,
    model_answer: &str,
) -> Result<(String, Option<String>, String), WordlessError> {
    if !QUESTION_TYPES.contains(&qtype) {
        return Err(bad_request("Invalid question type."));
    }
    match qtype {
        "mcq" => {
            if options.len() < 2 {
                return Err(bad_request(
                    "A multiple-choice question needs at least 2 options.",
                ));
            }
            if options.iter().any(|o| o.trim().is_empty()) {
                return Err(bad_request("Question options cannot be empty."));
            }
            let idx = answer.as_i64().ok_or_else(|| {
                bad_request("A multiple-choice answer must be the index of the correct option.")
            })?;
            if idx < 0 || idx as usize >= options.len() {
                return Err(bad_request("The correct option index is out of range."));
            }
            Ok((
                serde_json::to_string(&options).unwrap_or_else(|_| "[]".to_string()),
                Some(idx.to_string()),
                String::new(),
            ))
        }
        "true_false" => {
            let key = answer
                .as_bool()
                .ok_or_else(|| bad_request("A true/false answer must be true or false."))?;
            Ok((
                "[\"True\", \"False\"]".to_string(),
                Some(key.to_string()),
                String::new(),
            ))
        }
        "multi_select" => {
            if options.len() < 2 {
                return Err(bad_request(
                    "A multi-select question needs at least 2 options.",
                ));
            }
            if options.iter().any(|o| o.trim().is_empty()) {
                return Err(bad_request("Question options cannot be empty."));
            }
            let arr = answer.as_array().ok_or_else(|| {
                bad_request("A multi-select answer must be a list of option indices.")
            })?;
            let mut idxs: Vec<i64> = arr.iter().filter_map(|v| v.as_i64()).collect();
            if idxs.len() != arr.len() {
                return Err(bad_request(
                    "A multi-select answer must be a list of option indices.",
                ));
            }
            idxs.sort_unstable();
            idxs.dedup();
            if idxs.is_empty() {
                return Err(bad_request(
                    "A multi-select question needs at least one correct option.",
                ));
            }
            if idxs.iter().any(|i| *i < 0 || *i as usize >= options.len()) {
                return Err(bad_request("A multi-select answer index is out of range."));
            }
            Ok((
                serde_json::to_string(&options).unwrap_or_else(|_| "[]".to_string()),
                Some(serde_json::to_string(&idxs).unwrap_or_else(|_| "[]".to_string())),
                String::new(),
            ))
        }
        "short_answer" => Ok(("[]".to_string(), None, model_answer.trim().to_string())),
        _ => unreachable!(),
    }
}

// ------------------------------------------------------------------ banks

pub async fn create_bank(
    db: &dyn Db,
    creator: i64,
    course_id: i64,
    title: &str,
    description: &str,
    is_admin: bool,
) -> Result<QuestionBank, WordlessError> {
    let _course = crate::courses::get_course(db, course_id, Some(creator), is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    require_manage_course(db, creator, course_id, is_admin).await?;
    let title = title.trim();
    if title.is_empty() {
        return Err(bad_request("Question bank title is required."));
    }
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_question_banks (course_id, title, description, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?)",
            &[
                Param::Integer(course_id),
                Param::Text(title.to_string()),
                Param::Text(description.trim().to_string()),
                Param::Text(now.clone()),
                Param::Text(now),
            ],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    get_bank(db, id)
        .await?
        .ok_or_else(|| not_found("Question bank not found."))
}

pub async fn get_bank(db: &dyn Db, id: i64) -> Result<Option<QuestionBank>, WordlessError> {
    let row = db
        .query_one(
            "SELECT b.*, (SELECT COUNT(*) FROM wl_questions q WHERE q.bank_id = b.id) AS question_count \
             FROM wl_question_banks b WHERE b.id = ? LIMIT 1",
            &[Param::Integer(id)],
        )
        .await?;
    Ok(row.map(|r| row_to_bank(&r)))
}

pub async fn list_banks(
    db: &dyn Db,
    course_id: i64,
    requester: i64,
    is_admin: bool,
) -> Result<Vec<QuestionBank>, WordlessError> {
    let _course = crate::courses::get_course(db, course_id, Some(requester), is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    require_manage_course(db, requester, course_id, is_admin).await?;
    let rows = db
        .query_all(
            "SELECT b.*, (SELECT COUNT(*) FROM wl_questions q WHERE q.bank_id = b.id) AS question_count \
             FROM wl_question_banks b WHERE b.course_id = ? ORDER BY b.created_at ASC, b.id ASC",
            &[Param::Integer(course_id)],
        )
        .await?;
    Ok(rows.iter().map(row_to_bank).collect())
}

pub async fn delete_bank(
    db: &dyn Db,
    deleter: i64,
    bank_id: i64,
    is_admin: bool,
) -> Result<(), WordlessError> {
    let bank = get_bank(db, bank_id)
        .await?
        .ok_or_else(|| not_found("Question bank not found."))?;
    require_manage_course(db, deleter, bank.course_id, is_admin).await?;
    let in_use = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_exams WHERE bank_id = ?",
            &[Param::Integer(bank_id)],
        )
        .await?;
    if in_use.map(|r| row_int(&r, "c")).unwrap_or(0) > 0 {
        return Err(wordless_kernel::error::conflict(
            "This question bank is used by an exam and cannot be deleted.",
            "bank_in_use",
        ));
    }
    db.exec(
        "DELETE FROM wl_questions WHERE bank_id = ?",
        &[Param::Integer(bank_id)],
    )
    .await?;
    db.exec(
        "DELETE FROM wl_question_banks WHERE id = ?",
        &[Param::Integer(bank_id)],
    )
    .await?;
    Ok(())
}

// -------------------------------------------------------------- questions

pub struct CreateQuestionInput {
    pub qtype: String,
    pub question_text: String,
    pub options: Vec<String>,
    pub answer: serde_json::Value,
    pub model_answer: String,
    pub marks: i64,
    pub difficulty: String,
    pub topic: String,
}

pub async fn create_question(
    db: &dyn Db,
    creator: i64,
    bank_id: i64,
    input: &CreateQuestionInput,
    is_admin: bool,
) -> Result<Question, WordlessError> {
    let bank = get_bank(db, bank_id)
        .await?
        .ok_or_else(|| not_found("Question bank not found."))?;
    require_manage_course(db, creator, bank.course_id, is_admin).await?;
    let text = input.question_text.trim();
    if text.is_empty() {
        return Err(bad_request("Question text is required."));
    }
    if input.marks < 1 {
        return Err(bad_request("Question marks must be at least 1."));
    }
    let (options_json, answer_json, model_answer) = validate_question(
        &input.qtype,
        &input.options,
        &input.answer,
        &input.model_answer,
    )?;
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_questions (bank_id, type, question_text, options_json, answer_json, model_answer, marks, difficulty, topic, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[
                Param::Integer(bank_id),
                Param::Text(input.qtype.clone()),
                Param::Text(text.to_string()),
                Param::Text(options_json),
                match answer_json {
                    Some(a) => Param::Text(a),
                    None => Param::Null,
                },
                Param::Text(model_answer),
                Param::Integer(input.marks),
                Param::Text(if input.difficulty.trim().is_empty() { "medium".to_string() } else { input.difficulty.trim().to_string() }),
                Param::Text(input.topic.trim().to_string()),
                Param::Text(now.clone()),
                Param::Text(now),
            ],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    get_question(db, id)
        .await?
        .ok_or_else(|| not_found("Question not found."))
}

pub async fn get_question(db: &dyn Db, id: i64) -> Result<Option<Question>, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_questions WHERE id = ? LIMIT 1",
            &[Param::Integer(id)],
        )
        .await?;
    Ok(row.map(|r| row_to_question(&r)))
}

/// Questions of a bank, WITH answer keys — instructor/admin only.
/// The caller must have checked `require_manage_course` for the
/// bank's course first (the route handler does it).
pub async fn list_questions(db: &dyn Db, bank_id: i64) -> Result<Vec<Question>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT * FROM wl_questions WHERE bank_id = ? ORDER BY id ASC",
            &[Param::Integer(bank_id)],
        )
        .await?;
    Ok(rows.iter().map(row_to_question).collect())
}

pub struct UpdateQuestionInput {
    pub question_text: Option<String>,
    pub options: Option<Vec<String>>,
    pub answer: Option<serde_json::Value>,
    pub model_answer: Option<String>,
    pub marks: Option<i64>,
    pub difficulty: Option<String>,
    pub topic: Option<String>,
}

/// Update a question. Options/answer are validated together when
/// either changes (an answer without its options is meaningless);
/// the type itself never changes (delete + recreate instead).
pub async fn update_question(
    db: &dyn Db,
    updater: i64,
    question_id: i64,
    input: &UpdateQuestionInput,
    is_admin: bool,
) -> Result<Question, WordlessError> {
    let question = get_question(db, question_id)
        .await?
        .ok_or_else(|| not_found("Question not found."))?;
    let bank = get_bank(db, question.bank_id)
        .await?
        .ok_or_else(|| not_found("Question bank not found."))?;
    require_manage_course(db, updater, bank.course_id, is_admin).await?;
    // Refuse edits while an exam in this bank has an attempt in
    // progress — changing the key mid-exam would grade the same
    // attempt against two different keys. Recorded answers keep
    // their stored marks, so editing between exams is safe.
    let in_progress = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_exam_attempts a \
             JOIN wl_exams e ON e.id = a.exam_id \
             WHERE e.bank_id = ? AND a.status = 'in_progress'",
            &[Param::Integer(bank.id)],
        )
        .await?;
    if in_progress.map(|r| row_int(&r, "c")).unwrap_or(0) > 0 {
        return Err(wordless_kernel::error::conflict(
            "This question cannot be edited while an exam is in progress.",
            "exam_in_progress",
        ));
    }
    let mut text = question.question_text.clone();
    let mut options = question.options.clone();
    let mut answer = question.answer.clone().unwrap_or(serde_json::Value::Null);
    let mut model_answer = question.model_answer.clone();
    let mut marks = question.marks;
    let mut difficulty = question.difficulty.clone();
    let mut topic = question.topic.clone();
    if let Some(t) = &input.question_text {
        let t = t.trim();
        if t.is_empty() {
            return Err(bad_request("Question text is required."));
        }
        text = t.to_string();
    }
    if let Some(o) = &input.options {
        options = o.clone();
    }
    if let Some(a) = &input.answer {
        answer = a.clone();
    }
    if let Some(m) = &input.model_answer {
        model_answer = m.clone();
    }
    if let Some(m) = input.marks {
        if m < 1 {
            return Err(bad_request("Question marks must be at least 1."));
        }
        marks = m;
    }
    if let Some(d) = &input.difficulty {
        difficulty = if d.trim().is_empty() {
            "medium".to_string()
        } else {
            d.trim().to_string()
        };
    }
    if let Some(t) = &input.topic {
        topic = t.trim().to_string();
    }
    // Options/answer/model are validated together (an answer without
    // its options is meaningless); the type itself never changes.
    let (options_json, answer_json, model_answer) =
        validate_question(&question.qtype, &options, &answer, &model_answer)?;
    db.exec(
        "UPDATE wl_questions SET question_text = ?, options_json = ?, answer_json = ?, model_answer = ?, marks = ?, difficulty = ?, topic = ?, updated_at = ? WHERE id = ?",
        &[
            Param::Text(text),
            Param::Text(options_json),
            match answer_json {
                Some(a) => Param::Text(a),
                None => Param::Null,
            },
            Param::Text(model_answer),
            Param::Integer(marks),
            Param::Text(difficulty),
            Param::Text(topic),
            Param::Text(now_iso()),
            Param::Integer(question_id),
        ],
    )
    .await?;
    get_question(db, question_id)
        .await?
        .ok_or_else(|| not_found("Question not found."))
}

pub async fn delete_question(
    db: &dyn Db,
    deleter: i64,
    question_id: i64,
    is_admin: bool,
) -> Result<(), WordlessError> {
    let question = get_question(db, question_id)
        .await?
        .ok_or_else(|| not_found("Question not found."))?;
    let bank = get_bank(db, question.bank_id)
        .await?
        .ok_or_else(|| not_found("Question bank not found."))?;
    require_manage_course(db, deleter, bank.course_id, is_admin).await?;
    let used = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_exam_answers WHERE question_id = ?",
            &[Param::Integer(question_id)],
        )
        .await?;
    if used.map(|r| row_int(&r, "c")).unwrap_or(0) > 0 {
        return Err(wordless_kernel::error::conflict(
            "This question has answers and cannot be deleted.",
            "question_in_use",
        ));
    }
    db.exec(
        "DELETE FROM wl_questions WHERE id = ?",
        &[Param::Integer(question_id)],
    )
    .await?;
    Ok(())
}

/// The student-facing shape of a question: no answer key, no model
/// answer. Used for the exam start payload.
pub fn to_student(question: &Question) -> crate::types::AttemptQuestion {
    crate::types::AttemptQuestion {
        question_id: question.id,
        qtype: question.qtype.clone(),
        question_text: question.question_text.clone(),
        options: question.options.clone(),
        marks: question.marks,
    }
}

/// The instructor-facing JSON for one question, WITH the answer key
/// (serialised through the `Question` struct itself).
pub fn to_instructor_json(question: &Question) -> serde_json::Value {
    serde_json::to_value(question).unwrap_or_else(|_| json!({}))
}

fn row_to_bank(row: &wordless_db::Row) -> QuestionBank {
    QuestionBank {
        id: row_int(row, "id"),
        course_id: row_int(row, "course_id"),
        title: row_text(row, "title"),
        description: row_text(row, "description"),
        question_count: row_int(row, "question_count"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

pub fn row_to_question(row: &wordless_db::Row) -> Question {
    let options: Vec<String> =
        serde_json::from_str(&row_text(row, "options_json")).unwrap_or_default();
    let answer: Option<serde_json::Value> = match row.get("answer_json") {
        Some(v) if !v.is_null() => match v {
            serde_json::Value::String(s) => serde_json::from_str(s).ok(),
            other => Some(other.clone()),
        },
        _ => None,
    };
    Question {
        id: row_int(row, "id"),
        bank_id: row_int(row, "bank_id"),
        qtype: row_text(row, "type"),
        question_text: row_text(row, "question_text"),
        options,
        answer,
        model_answer: row_text(row, "model_answer"),
        marks: row_int(row, "marks"),
        difficulty: row_text(row, "difficulty"),
        topic: row_text(row, "topic"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}
