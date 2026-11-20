//! Exams, attempts, grading, results, leaderboard, certificates.
//!
//! State machine (doc §3 P9 outcome block):
//! - `in_progress` --student submits on time--> `submitted`
//!   (objective-only, fully auto-graded) or `awaiting_grading`
//!   (short answers still ungraded).
//! - `in_progress` --deadline passes--> `expired` (objective-only)
//!   or `awaiting_grading` (short answers pending). Expiry is
//!   detected by the cron sweep AND lazily on attempt read / answer
//!   save / submit — the lazy half is the locally verifiable one.
//! - `awaiting_grading` --last short answer graded--> `graded`.
//! - A result row exists ONLY once grading is complete (fully
//!   auto-graded, or manually finished); while `awaiting_grading`
//!   the result is 404 "still being graded" — a partial score is
//!   never presented as a final result.
//! - A certificate is auto-issued only on grading completion at or
//!   above the pass mark.
//!
//! Timing: `deadline_ms` is computed once at start and stored; the
//! browser clock is never trusted. A late submit is a 409
//! `attempt_expired` — but the attempt IS auto-submitted with the
//! answers saved so far, so the student still gets a result.
//!
//! Starting an exam when an `in_progress` attempt already exists
//! RESUMES it (same attempt, same question order, same deadline) —
//! a refresh mid-exam must not burn an attempt.

use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, conflict, forbidden, not_found, WordlessError};
use wordless_kernel::time::{iso_from_ms, ms_from_iso, now_iso};

use crate::courses::{get_course, get_enrolment, is_instructor, require_manage_course};
use crate::grading::{
    certificate_code, deadline_ms, grade_objective, percent, round2, shuffle_order,
    validate_answer_shape,
};
use crate::questions::{get_question, row_to_question, to_student};
use crate::types::{
    AttemptQuestion, Certificate, Exam, ExamAnswer, ExamAttempt, ExamResult, GradingQueueItem,
    LeaderboardRow, Question,
};

pub struct CreateExamInput {
    pub title: String,
    pub description: String,
    pub duration_minutes: i64,
    pub window_start: Option<String>,
    pub window_end: Option<String>,
    pub max_attempts: i64,
    pub pass_mark_percent: i64,
    pub shuffle_questions: bool,
    pub question_ids: Vec<i64>,
}

pub struct UpdateExamInput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub duration_minutes: Option<i64>,
    pub window_start: Option<Option<String>>,
    pub window_end: Option<Option<String>>,
    pub max_attempts: Option<i64>,
    pub pass_mark_percent: Option<i64>,
    pub shuffle_questions: Option<bool>,
    pub question_ids: Option<Vec<i64>>,
    pub status: Option<String>,
}

fn validate_window(start: &Option<String>, end: &Option<String>) -> Result<(), WordlessError> {
    let (Some(s), Some(e)) = (start, end) else {
        return Ok(());
    };
    let (Some(sm), Some(em)) = (ms_from_iso(s), ms_from_iso(e)) else {
        return Err(bad_request("Exam window times must be ISO-8601 UTC."));
    };
    if em <= sm {
        return Err(bad_request("The exam window must end after it starts."));
    }
    Ok(())
}

/// The questions an exam draws: the explicit `question_ids` (in
/// stored order, skipping ids that no longer exist), or every
/// question in the bank when the list is empty.
pub async fn exam_questions(db: &dyn Db, exam: &Exam) -> Result<Vec<Question>, WordlessError> {
    if !exam.question_ids.is_empty() {
        let mut out = Vec::new();
        for qid in &exam.question_ids {
            if let Some(q) = get_question(db, *qid).await? {
                out.push(q);
            }
        }
        return Ok(out);
    }
    let rows = db
        .query_all(
            "SELECT * FROM wl_questions WHERE bank_id = ? ORDER BY id ASC",
            &[Param::Integer(exam.bank_id)],
        )
        .await?;
    Ok(rows.iter().map(row_to_question).collect())
}

pub async fn create_exam(
    db: &dyn Db,
    creator: i64,
    course_id: i64,
    bank_id: i64,
    input: &CreateExamInput,
    is_admin: bool,
) -> Result<Exam, WordlessError> {
    let _course = get_course(db, course_id, Some(creator), is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    require_manage_course(db, creator, course_id, is_admin).await?;
    let bank = crate::questions::get_bank(db, bank_id)
        .await?
        .ok_or_else(|| not_found("Question bank not found."))?;
    if bank.course_id != course_id {
        return Err(bad_request(
            "The question bank belongs to a different course.",
        ));
    }
    let title = input.title.trim();
    if title.is_empty() {
        return Err(bad_request("Exam title is required."));
    }
    if input.duration_minutes < 1 {
        return Err(bad_request("Exam duration must be at least 1 minute."));
    }
    if input.max_attempts < 0 {
        return Err(bad_request("Maximum attempts cannot be negative."));
    }
    if !(0..=100).contains(&input.pass_mark_percent) {
        return Err(bad_request("Pass mark must be between 0 and 100."));
    }
    validate_window(&input.window_start, &input.window_end)?;
    // Every explicit question id must exist in this bank.
    let mut question_ids = input.question_ids.clone();
    question_ids.sort_unstable();
    question_ids.dedup();
    for qid in &question_ids {
        let q = get_question(db, *qid)
            .await?
            .ok_or_else(|| bad_request("An exam question does not exist."))?;
        if q.bank_id != bank_id {
            return Err(bad_request("An exam question belongs to a different bank."));
        }
    }
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_exams (course_id, bank_id, title, description, duration_minutes, window_start, window_end, max_attempts, pass_mark_percent, shuffle_questions, question_ids_json, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'draft', ?, ?)",
            &[
                Param::Integer(course_id),
                Param::Integer(bank_id),
                Param::Text(title.to_string()),
                Param::Text(input.description.trim().to_string()),
                Param::Integer(input.duration_minutes),
                match &input.window_start { Some(s) => Param::Text(s.clone()), None => Param::Null },
                match &input.window_end { Some(s) => Param::Text(s.clone()), None => Param::Null },
                Param::Integer(input.max_attempts),
                Param::Integer(input.pass_mark_percent),
                Param::Integer(if input.shuffle_questions { 1 } else { 0 }),
                Param::Text(serde_json::to_string(&question_ids).unwrap_or_else(|_| "[]".to_string())),
                Param::Text(now.clone()),
                Param::Text(now),
            ],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    get_exam(db, id, Some(creator), is_admin)
        .await?
        .ok_or_else(|| not_found("Exam not found."))
}

/// Read one exam. Drafts are invisible to non-instructors and
/// non-admins (404, not 403). `attempts_used` counts the viewer's
/// own attempts (0 for anonymous).
pub async fn get_exam(
    db: &dyn Db,
    id: i64,
    viewer: Option<i64>,
    is_admin: bool,
) -> Result<Option<Exam>, WordlessError> {
    let row = db
        .query_one(
            "SELECT e.*, \
             (SELECT COUNT(*) FROM wl_questions q WHERE q.bank_id = e.bank_id) AS bank_question_count \
             FROM wl_exams e WHERE e.id = ? LIMIT 1",
            &[Param::Integer(id)],
        )
        .await?;
    let Some(r) = row else { return Ok(None) };
    let status = row_text(&r, "status");
    let course_id = row_int(&r, "course_id");
    let viewer_is_instructor = match viewer {
        Some(uid) => is_instructor(db, uid, course_id).await?,
        None => false,
    };
    if status == "draft" && !is_admin && !viewer_is_instructor {
        return Ok(None);
    }
    let mut exam = row_to_exam(&r);
    exam.question_count = if exam.question_ids.is_empty() {
        row_int(&r, "bank_question_count")
    } else {
        exam.question_ids.len() as i64
    };
    exam.attempts_used = match viewer {
        Some(uid) => {
            let c = db
                .query_one(
                    "SELECT COUNT(*) AS c FROM wl_exam_attempts WHERE exam_id = ? AND user_id = ?",
                    &[Param::Integer(id), Param::Integer(uid)],
                )
                .await?;
            c.map(|r| row_int(&r, "c")).unwrap_or(0)
        }
        None => 0,
    };
    Ok(Some(exam))
}

pub async fn list_exams(
    db: &dyn Db,
    course_id: i64,
    viewer: Option<i64>,
    is_admin: bool,
) -> Result<Vec<Exam>, WordlessError> {
    let _course = get_course(db, course_id, viewer, is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    let is_instr = match viewer {
        Some(uid) => is_instructor(db, uid, course_id).await?,
        None => false,
    };
    let rows = db
        .query_all(
            "SELECT e.*, \
             (SELECT COUNT(*) FROM wl_questions q WHERE q.bank_id = e.bank_id) AS bank_question_count \
             FROM wl_exams e WHERE e.course_id = ? ORDER BY e.created_at ASC, e.id ASC",
            &[Param::Integer(course_id)],
        )
        .await?;
    let mut out = Vec::new();
    for r in &rows {
        if row_text(r, "status") == "draft" && !is_admin && !is_instr {
            continue;
        }
        let mut exam = row_to_exam(r);
        exam.question_count = if exam.question_ids.is_empty() {
            row_int(r, "bank_question_count")
        } else {
            exam.question_ids.len() as i64
        };
        exam.attempts_used = match viewer {
            Some(uid) => {
                let c = db
                    .query_one(
                        "SELECT COUNT(*) AS c FROM wl_exam_attempts WHERE exam_id = ? AND user_id = ?",
                        &[Param::Integer(exam.id), Param::Integer(uid)],
                    )
                    .await?;
                c.map(|r| row_int(&r, "c")).unwrap_or(0)
            }
            None => 0,
        };
        out.push(exam);
    }
    Ok(out)
}

pub async fn update_exam(
    db: &dyn Db,
    updater: i64,
    id: i64,
    input: &UpdateExamInput,
    is_admin: bool,
) -> Result<Exam, WordlessError> {
    let exam = get_exam(db, id, Some(updater), is_admin)
        .await?
        .ok_or_else(|| not_found("Exam not found."))?;
    require_manage_course(db, updater, exam.course_id, is_admin).await?;
    // Refuse edits while an attempt is in progress — the questions,
    // window and duration are already fixed for that attempt.
    let in_progress = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_exam_attempts WHERE exam_id = ? AND status = 'in_progress'",
            &[Param::Integer(id)],
        )
        .await?;
    if in_progress.map(|r| row_int(&r, "c")).unwrap_or(0) > 0 {
        return Err(conflict(
            "This exam cannot be edited while an attempt is in progress.",
            "exam_in_progress",
        ));
    }
    let mut title = exam.title.clone();
    let mut description = exam.description.clone();
    let mut duration = exam.duration_minutes;
    let mut window_start = exam.window_start.clone();
    let mut window_end = exam.window_end.clone();
    let mut max_attempts = exam.max_attempts;
    let mut pass_mark = exam.pass_mark_percent;
    let mut shuffle = exam.shuffle_questions;
    let mut question_ids = exam.question_ids.clone();
    let mut status = exam.status.clone();
    if let Some(t) = &input.title {
        let t = t.trim();
        if t.is_empty() {
            return Err(bad_request("Exam title is required."));
        }
        title = t.to_string();
    }
    if let Some(d) = &input.description {
        description = d.trim().to_string();
    }
    if let Some(d) = input.duration_minutes {
        if d < 1 {
            return Err(bad_request("Exam duration must be at least 1 minute."));
        }
        duration = d;
    }
    if let Some(w) = &input.window_start {
        window_start = w.clone();
    }
    if let Some(w) = &input.window_end {
        window_end = w.clone();
    }
    if let Some(m) = input.max_attempts {
        if m < 0 {
            return Err(bad_request("Maximum attempts cannot be negative."));
        }
        max_attempts = m;
    }
    if let Some(p) = input.pass_mark_percent {
        if !(0..=100).contains(&p) {
            return Err(bad_request("Pass mark must be between 0 and 100."));
        }
        pass_mark = p;
    }
    if let Some(s) = input.shuffle_questions {
        shuffle = s;
    }
    if let Some(qs) = &input.question_ids {
        let mut qs = qs.clone();
        qs.sort_unstable();
        qs.dedup();
        for qid in &qs {
            let q = get_question(db, *qid)
                .await?
                .ok_or_else(|| bad_request("An exam question does not exist."))?;
            if q.bank_id != exam.bank_id {
                return Err(bad_request("An exam question belongs to a different bank."));
            }
        }
        question_ids = qs;
    }
    if let Some(s) = &input.status {
        if !["draft", "published"].contains(&s.as_str()) {
            return Err(bad_request("Invalid exam status."));
        }
        status = s.clone();
    }
    validate_window(&window_start, &window_end)?;
    db.exec(
        "UPDATE wl_exams SET title = ?, description = ?, duration_minutes = ?, window_start = ?, window_end = ?, max_attempts = ?, pass_mark_percent = ?, shuffle_questions = ?, question_ids_json = ?, status = ?, updated_at = ? WHERE id = ?",
        &[
            Param::Text(title),
            Param::Text(description),
            Param::Integer(duration),
            match &window_start { Some(s) => Param::Text(s.clone()), None => Param::Null },
            match &window_end { Some(s) => Param::Text(s.clone()), None => Param::Null },
            Param::Integer(max_attempts),
            Param::Integer(pass_mark),
            Param::Integer(if shuffle { 1 } else { 0 }),
            Param::Text(serde_json::to_string(&question_ids).unwrap_or_else(|_| "[]".to_string())),
            Param::Text(status),
            Param::Text(now_iso()),
            Param::Integer(id),
        ],
    )
    .await?;
    get_exam(db, id, Some(updater), is_admin)
        .await?
        .ok_or_else(|| not_found("Exam not found."))
}

pub async fn delete_exam(
    db: &dyn Db,
    deleter: i64,
    id: i64,
    is_admin: bool,
) -> Result<(), WordlessError> {
    let exam = get_exam(db, id, Some(deleter), is_admin)
        .await?
        .ok_or_else(|| not_found("Exam not found."))?;
    require_manage_course(db, deleter, exam.course_id, is_admin).await?;
    let attempts = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_exam_attempts WHERE exam_id = ?",
            &[Param::Integer(id)],
        )
        .await?;
    if attempts.map(|r| row_int(&r, "c")).unwrap_or(0) > 0 {
        return Err(conflict(
            "This exam has attempts and cannot be deleted.",
            "exam_in_use",
        ));
    }
    db.exec("DELETE FROM wl_exams WHERE id = ?", &[Param::Integer(id)])
        .await?;
    Ok(())
}

// --------------------------------------------------------------- attempts

/// Start (or resume) an attempt. Checks, in order: the exam is
/// published; the student is enrolled and active; the exam window is
/// open; no in-progress attempt already exists (resume it); the
/// attempt limit is not reached. The deadline is computed once, from
/// the server clock, and stored.
pub async fn start_attempt(
    db: &dyn Db,
    user_id: i64,
    exam_id: i64,
    started_ip: &str,
    now: u64,
) -> Result<ExamAttempt, WordlessError> {
    let exam = get_exam(db, exam_id, Some(user_id), false)
        .await?
        .ok_or_else(|| not_found("Exam not found."))?;
    if exam.status != "published" {
        return Err(forbidden("This exam is not open."));
    }
    let enrolment = get_enrolment(db, user_id, exam.course_id)
        .await?
        .ok_or_else(|| forbidden("You must be enrolled in this course to sit the exam."))?;
    if enrolment.status == "dropped" {
        return Err(forbidden(
            "You must be enrolled in this course to sit the exam.",
        ));
    }
    check_window(&exam, now)?;
    // An in-progress attempt is resumed — a refresh mid-exam must
    // not burn an attempt. An expired one is closed first (it counts
    // toward the limit).
    if let Some(existing) = load_in_progress(db, exam_id, user_id).await? {
        if now as i64 > existing.deadline_ms {
            // Auto-submitted as of the deadline (the answers are as
            // of the deadline, not as of this discovery).
            finish_attempt(db, existing.id, existing.deadline_ms, true, now).await?;
        } else {
            return load_attempt_detail(db, existing.id, user_id, false, now)
                .await?
                .ok_or_else(|| not_found("Attempt not found."));
        }
    }
    let used = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_exam_attempts WHERE exam_id = ? AND user_id = ? AND status != 'in_progress'",
            &[Param::Integer(exam_id), Param::Integer(user_id)],
        )
        .await?;
    let used = used.map(|r| row_int(&r, "c")).unwrap_or(0);
    if exam.max_attempts > 0 && used >= exam.max_attempts {
        return Err(conflict(
            "You have used all your attempts for this exam.",
            "attempt_limit",
        ));
    }
    let questions = exam_questions(db, &exam).await?;
    if questions.is_empty() {
        return Err(bad_request("This exam has no questions."));
    }
    let max_score: i64 = questions.iter().map(|q| q.marks).sum();
    let deadline = deadline_ms(now, exam.duration_minutes);
    let now_s = iso_from_ms(now);
    let meta = db
        .exec(
            "INSERT INTO wl_exam_attempts (exam_id, user_id, status, started_at_ms, deadline_ms, question_order_json, score, max_score, started_ip, created_at, updated_at) \
             VALUES (?, ?, 'in_progress', ?, ?, '[]', 0, ?, ?, ?, ?)",
            &[
                Param::Integer(exam_id),
                Param::Integer(user_id),
                Param::Integer(now as i64),
                Param::Integer(deadline as i64),
                Param::Integer(max_score),
                Param::Text(started_ip.to_string()),
                Param::Text(now_s.clone()),
                Param::Text(now_s),
            ],
        )
        .await?;
    let attempt_id = meta.last_row_id.unwrap_or(0);
    let ids: Vec<i64> = questions.iter().map(|q| q.id).collect();
    let order = if exam.shuffle_questions {
        shuffle_order(&ids, attempt_id as u64)
    } else {
        ids
    };
    db.exec(
        "UPDATE wl_exam_attempts SET question_order_json = ? WHERE id = ?",
        &[
            Param::Text(serde_json::to_string(&order).unwrap_or_else(|_| "[]".to_string())),
            Param::Integer(attempt_id),
        ],
    )
    .await?;
    load_attempt_detail(db, attempt_id, user_id, false, now)
        .await?
        .ok_or_else(|| not_found("Attempt not found."))
}

fn check_window(exam: &Exam, now: u64) -> Result<(), WordlessError> {
    if let Some(s) = &exam.window_start {
        if let Some(sm) = ms_from_iso(s) {
            if now < sm {
                return Err(conflict("This exam has not opened yet.", "exam_not_open"));
            }
        }
    }
    if let Some(e) = &exam.window_end {
        if let Some(em) = ms_from_iso(e) {
            if now >= em {
                return Err(conflict(
                    "This exam's window has closed.",
                    "exam_window_closed",
                ));
            }
        }
    }
    Ok(())
}

async fn load_in_progress(
    db: &dyn Db,
    exam_id: i64,
    user_id: i64,
) -> Result<Option<AttemptRow>, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_exam_attempts WHERE exam_id = ? AND user_id = ? AND status = 'in_progress' LIMIT 1",
            &[Param::Integer(exam_id), Param::Integer(user_id)],
        )
        .await?;
    Ok(row.map(|r| row_to_attempt_row(&r)))
}

/// The attempt row without its questions/answers (for list views and
/// internal use).
#[derive(Debug, Clone)]
struct AttemptRow {
    id: i64,
    exam_id: i64,
    user_id: i64,
    status: String,
    started_at_ms: i64,
    deadline_ms: i64,
    submitted_at_ms: Option<i64>,
    question_order: Vec<i64>,
    score: f64,
    max_score: i64,
    created_at: String,
    updated_at: String,
}

fn row_to_attempt_row(row: &wordless_db::Row) -> AttemptRow {
    let order: Vec<i64> =
        serde_json::from_str(&row_text(row, "question_order_json")).unwrap_or_default();
    AttemptRow {
        id: row_int(row, "id"),
        exam_id: row_int(row, "exam_id"),
        user_id: row_int(row, "user_id"),
        status: row_text(row, "status"),
        started_at_ms: row_int(row, "started_at_ms"),
        deadline_ms: row_int(row, "deadline_ms"),
        submitted_at_ms: match row.get("submitted_at_ms") {
            Some(v) if !v.is_null() => Some(row_int(row, "submitted_at_ms")),
            _ => None,
        },
        question_order: order,
        score: row.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0),
        max_score: row_int(row, "max_score"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// Load one attempt with its questions (no answer keys) and saved
/// answers. The caller must be the owner, the course instructor, or
/// a site admin. Lazy expiry: an `in_progress` attempt past its
/// deadline is auto-submitted first.
pub async fn get_attempt(
    db: &dyn Db,
    attempt_id: i64,
    viewer_id: i64,
    is_admin: bool,
    now: u64,
) -> Result<Option<ExamAttempt>, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_exam_attempts WHERE id = ? LIMIT 1",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let Some(r) = row else { return Ok(None) };
    let attempt = row_to_attempt_row(&r);
    let exam = get_exam(db, attempt.exam_id, Some(viewer_id), is_admin)
        .await?
        .ok_or_else(|| not_found("Attempt not found."))?;
    if attempt.user_id != viewer_id
        && !is_admin
        && !is_instructor(db, viewer_id, exam.course_id).await?
    {
        return Err(forbidden("This attempt belongs to another student."));
    }
    if attempt.status == "in_progress" && now as i64 > attempt.deadline_ms {
        finish_attempt(db, attempt_id, now as i64, true, now).await?;
    }
    load_attempt_detail(db, attempt_id, viewer_id, is_admin, now).await
}

/// Attempts of one exam (instructor/admin). Questions and answers
/// are NOT loaded here — use `get_attempt` for the detail.
pub async fn list_attempts(
    db: &dyn Db,
    exam_id: i64,
    requester: i64,
    is_admin: bool,
) -> Result<Vec<ExamAttempt>, WordlessError> {
    let exam = get_exam(db, exam_id, Some(requester), is_admin)
        .await?
        .ok_or_else(|| not_found("Exam not found."))?;
    require_manage_course(db, requester, exam.course_id, is_admin).await?;
    let rows = db
        .query_all(
            "SELECT * FROM wl_exam_attempts WHERE exam_id = ? ORDER BY started_at_ms DESC, id DESC",
            &[Param::Integer(exam_id)],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let a = row_to_attempt_row(r);
            ExamAttempt {
                id: a.id,
                exam_id: a.exam_id,
                user_id: a.user_id,
                status: a.status,
                started_at_ms: a.started_at_ms,
                deadline_ms: a.deadline_ms,
                submitted_at_ms: a.submitted_at_ms,
                question_order: a.question_order,
                score: a.score,
                max_score: a.max_score,
                questions: Vec::new(),
                answers: Vec::new(),
                created_at: a.created_at,
                updated_at: a.updated_at,
            }
        })
        .collect())
}

async fn load_attempt_detail(
    db: &dyn Db,
    attempt_id: i64,
    _viewer_id: i64,
    _is_admin: bool,
    _now: u64,
) -> Result<Option<ExamAttempt>, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_exam_attempts WHERE id = ? LIMIT 1",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let Some(r) = row else { return Ok(None) };
    let attempt = row_to_attempt_row(&r);
    let exam = get_exam(db, attempt.exam_id, None, true)
        .await?
        .ok_or_else(|| not_found("Attempt not found."))?;
    let questions = exam_questions(db, &exam).await?;
    let by_id: std::collections::HashMap<i64, &Question> =
        questions.iter().map(|q| (q.id, q)).collect();
    let mut attempt_questions: Vec<AttemptQuestion> = Vec::new();
    for qid in &attempt.question_order {
        if let Some(q) = by_id.get(qid) {
            attempt_questions.push(to_student(q));
        }
    }
    // Saved answers, in question order; unanswered questions appear
    // with a null answer so the review shows every question.
    let saved = db
        .query_all(
            "SELECT * FROM wl_exam_answers WHERE attempt_id = ?",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let saved_by_q: std::collections::HashMap<i64, &wordless_db::Row> = saved
        .iter()
        .map(|r| (row_int(r, "question_id"), r))
        .collect();
    let mut answers: Vec<ExamAnswer> = Vec::new();
    for qid in &attempt.question_order {
        match saved_by_q.get(qid) {
            Some(r) => answers.push(row_to_answer(r)),
            None => answers.push(ExamAnswer {
                question_id: *qid,
                answer: serde_json::Value::Null,
                is_correct: None,
                marks_awarded: None,
                grader_feedback: String::new(),
                graded_at: None,
            }),
        }
    }
    Ok(Some(ExamAttempt {
        id: attempt.id,
        exam_id: attempt.exam_id,
        user_id: attempt.user_id,
        status: attempt.status,
        started_at_ms: attempt.started_at_ms,
        deadline_ms: attempt.deadline_ms,
        submitted_at_ms: attempt.submitted_at_ms,
        question_order: attempt.question_order,
        score: attempt.score,
        max_score: attempt.max_score,
        questions: attempt_questions,
        answers,
        created_at: attempt.created_at,
        updated_at: attempt.updated_at,
    }))
}

/// Save one answer during an attempt. The attempt must be the
/// caller's, in progress, and inside its server-side deadline — a
/// save past the deadline auto-submits the attempt and is rejected
/// with 409 `attempt_expired`.
pub async fn save_answer(
    db: &dyn Db,
    user_id: i64,
    attempt_id: i64,
    question_id: i64,
    answer: serde_json::Value,
    now: u64,
) -> Result<ExamAttempt, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_exam_attempts WHERE id = ? LIMIT 1",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let Some(r) = row else {
        return Err(not_found("Attempt not found."));
    };
    let attempt = row_to_attempt_row(&r);
    if attempt.user_id != user_id {
        return Err(forbidden("This attempt belongs to another student."));
    }
    if attempt.status != "in_progress" {
        return Err(conflict(
            "This attempt is already closed.",
            "attempt_closed",
        ));
    }
    if now as i64 > attempt.deadline_ms {
        finish_attempt(db, attempt_id, now as i64, true, now).await?;
        return Err(conflict(
            "The exam deadline has passed; your saved answers were submitted.",
            "attempt_expired",
        ));
    }
    if !attempt.question_order.contains(&question_id) {
        return Err(bad_request("This question is not part of the attempt."));
    }
    let question = get_question(db, question_id)
        .await?
        .ok_or_else(|| not_found("Question not found."))?;
    validate_answer_shape(&question.qtype, &question.options, &answer)
        .map_err(|m| bad_request(&m))?;
    db.exec(
        "INSERT INTO wl_exam_answers (attempt_id, question_id, answer_json) VALUES (?, ?, ?) \
         ON CONFLICT (attempt_id, question_id) DO UPDATE SET answer_json = excluded.answer_json",
        &[
            Param::Integer(attempt_id),
            Param::Integer(question_id),
            Param::Text(serde_json::to_string(&answer).unwrap_or_else(|_| "null".to_string())),
        ],
    )
    .await?;
    load_attempt_detail(db, attempt_id, user_id, false, now)
        .await?
        .ok_or_else(|| not_found("Attempt not found."))
}

/// Submit an attempt. A submit past the deadline is rejected with
/// 409 `attempt_expired` — but the attempt IS auto-submitted with
/// the answers saved so far, so the student still gets a result.
pub async fn submit_attempt(
    db: &dyn Db,
    user_id: i64,
    attempt_id: i64,
    now: u64,
) -> Result<ExamAttempt, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_exam_attempts WHERE id = ? LIMIT 1",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let Some(r) = row else {
        return Err(not_found("Attempt not found."));
    };
    let attempt = row_to_attempt_row(&r);
    if attempt.user_id != user_id {
        return Err(forbidden("This attempt belongs to another student."));
    }
    if attempt.status != "in_progress" {
        return Err(conflict(
            "This attempt is already closed.",
            "attempt_closed",
        ));
    }
    if now as i64 > attempt.deadline_ms {
        finish_attempt(db, attempt_id, attempt.deadline_ms, true, now).await?;
        return Err(conflict(
            "The exam deadline has passed; your saved answers were submitted.",
            "attempt_expired",
        ));
    }
    finish_attempt(db, attempt_id, now as i64, false, now).await?;
    load_attempt_detail(db, attempt_id, user_id, false, now)
        .await?
        .ok_or_else(|| not_found("Attempt not found."))
}

/// Close an attempt: grade every objective answer, total the score,
/// set the status, and — when grading is complete — write the result
/// and issue a certificate if the pass mark was reached. Used by
/// submit, by the expiry paths, and by the cron sweep.
async fn finish_attempt(
    db: &dyn Db,
    attempt_id: i64,
    submitted_at_ms: i64,
    expired: bool,
    now: u64,
) -> Result<(), WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_exam_attempts WHERE id = ? LIMIT 1",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let Some(r) = row else {
        return Err(not_found("Attempt not found."));
    };
    let attempt = row_to_attempt_row(&r);
    if attempt.status != "in_progress" {
        return Ok(());
    }
    let exam = get_exam(db, attempt.exam_id, None, true)
        .await?
        .ok_or_else(|| not_found("Exam not found."))?;
    let questions = exam_questions(db, &exam).await?;
    let by_id: std::collections::HashMap<i64, &Question> =
        questions.iter().map(|q| (q.id, q)).collect();
    let max_score: i64 = attempt
        .question_order
        .iter()
        .filter_map(|qid| by_id.get(qid))
        .map(|q| q.marks)
        .sum();
    let saved = db
        .query_all(
            "SELECT * FROM wl_exam_answers WHERE attempt_id = ?",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let now_s = iso_from_ms(now);
    let mut score = 0.0;
    let mut has_ungraded = false;
    for ans in &saved {
        let qid = row_int(ans, "question_id");
        let Some(q) = by_id.get(&qid) else { continue };
        if q.qtype == "short_answer" {
            has_ungraded = true;
            continue;
        }
        let answer: serde_json::Value =
            serde_json::from_str(&row_text(ans, "answer_json")).unwrap_or(serde_json::Value::Null);
        let key = q.answer.clone().unwrap_or(serde_json::Value::Null);
        let (is_correct, awarded) =
            grade_objective(&q.qtype, &key, &answer, q.marks).unwrap_or((false, 0.0));
        score += awarded;
        db.exec(
            "UPDATE wl_exam_answers SET is_correct = ?, marks_awarded = ?, graded_at = ? \
             WHERE attempt_id = ? AND question_id = ?",
            &[
                Param::Integer(if is_correct { 1 } else { 0 }),
                Param::Real(round2(awarded)),
                Param::Text(now_s.clone()),
                Param::Integer(attempt_id),
                Param::Integer(qid),
            ],
        )
        .await?;
    }
    // Unanswered objective questions score 0 (no row, nothing to do).
    score = round2(score);
    let status = if has_ungraded {
        "awaiting_grading"
    } else if expired {
        "expired"
    } else {
        "submitted"
    };
    db.exec(
        "UPDATE wl_exam_attempts SET status = ?, submitted_at_ms = ?, score = ?, max_score = ?, updated_at = ? WHERE id = ?",
        &[
            Param::Text(status.to_string()),
            Param::Integer(submitted_at_ms),
            Param::Real(score),
            Param::Integer(max_score),
            Param::Text(now_s.clone()),
            Param::Integer(attempt_id),
        ],
    )
    .await?;
    if !has_ungraded {
        let result = upsert_result(
            db,
            attempt_id,
            attempt.user_id,
            &exam,
            score,
            max_score,
            &now_s,
        )
        .await?;
        if result.passed {
            let _ = maybe_issue_certificate(db, &result, now).await?;
        }
    }
    Ok(())
}

/// The cron sweep (and the lazy-expiry helper): close every
/// `in_progress` attempt whose deadline has passed. Returns how many
/// attempts were closed. A failure on one attempt is logged by the
/// caller and does not stop the sweep.
pub async fn close_expired_attempts(db: &dyn Db, now: u64) -> Result<u64, WordlessError> {
    let rows = db
        .query_all(
            "SELECT id FROM wl_exam_attempts WHERE status = 'in_progress' AND deadline_ms < ? ORDER BY deadline_ms ASC",
            &[Param::Integer(now as i64)],
        )
        .await?;
    let mut closed = 0u64;
    for r in &rows {
        let id = row_int(r, "id");
        // submitted_at_ms for an auto-submit is the deadline itself
        // (the answers are as of the deadline, not as of the sweep).
        let deadline = db
            .query_one(
                "SELECT deadline_ms FROM wl_exam_attempts WHERE id = ? LIMIT 1",
                &[Param::Integer(id)],
            )
            .await?;
        let deadline_ms = deadline
            .map(|d| row_int(&d, "deadline_ms"))
            .unwrap_or(now as i64);
        match finish_attempt(db, id, deadline_ms, true, now).await {
            Ok(()) => closed += 1,
            Err(_) => continue,
        }
    }
    Ok(closed)
}

// ----------------------------------------------------------- manual grading

/// The grading queue: ungraded short answers. An instructor sees
/// their own courses' queue; an admin sees everything; a course
/// filter narrows it.
pub async fn grading_queue(
    db: &dyn Db,
    requester: i64,
    course_id: Option<i64>,
    is_admin: bool,
) -> Result<Vec<GradingQueueItem>, WordlessError> {
    let course_filter = match course_id {
        Some(cid) => {
            let _course = get_course(db, cid, Some(requester), is_admin)
                .await?
                .ok_or_else(|| not_found("Course not found."))?;
            require_manage_course(db, requester, cid, is_admin).await?;
            format!("AND e.course_id = {cid}")
        }
        None => {
            if is_admin {
                String::new()
            } else {
                // The instructor's own courses only.
                "AND e.course_id IN (SELECT id FROM wl_courses WHERE instructor_id = ".to_string()
                    + &requester.to_string()
                    + ")"
            }
        }
    };
    let sql = format!(
        "SELECT ans.id AS answer_id, ans.attempt_id, a.exam_id, e.title AS exam_title, \
         e.course_id, ans.question_id, q.question_text, q.model_answer, q.marks, \
         a.user_id AS student_id, COALESCE(u.user_login, '') AS student_login, \
         COALESCE(u.display_name, '') AS student_name, ans.answer_json AS answer, \
         a.submitted_at_ms \
         FROM wl_exam_answers ans \
         JOIN wl_exam_attempts a ON a.id = ans.attempt_id \
         JOIN wl_exams e ON e.id = a.exam_id \
         JOIN wl_questions q ON q.id = ans.question_id \
         LEFT JOIN wp_users u ON u.ID = a.user_id \
         WHERE ans.is_correct IS NULL AND q.type = 'short_answer' {course_filter} \
         ORDER BY a.submitted_at_ms ASC, ans.id ASC"
    );
    let rows = db.query_all(&sql, &[]).await?;
    Ok(rows
        .iter()
        .map(|r| GradingQueueItem {
            answer_id: row_int(r, "answer_id"),
            attempt_id: row_int(r, "attempt_id"),
            exam_id: row_int(r, "exam_id"),
            exam_title: row_text(r, "exam_title"),
            course_id: row_int(r, "course_id"),
            question_id: row_int(r, "question_id"),
            question_text: row_text(r, "question_text"),
            model_answer: row_text(r, "model_answer"),
            marks: row_int(r, "marks"),
            student_id: row_int(r, "student_id"),
            student_login: row_text(r, "student_login"),
            student_name: row_text(r, "student_name"),
            answer: serde_json::from_str(&row_text(r, "answer")).unwrap_or(serde_json::Value::Null),
            submitted_at_ms: row_int(r, "submitted_at_ms"),
        })
        .collect())
}

/// Grade one short answer (instructor of the course, or admin).
/// `marks_awarded` must be within 0..=question marks. Full marks
/// counts as correct; partial credit counts as incorrect for the
/// per-question review (the marks themselves are what count).
/// When the last ungraded answer is graded, the attempt becomes
/// `graded`, the result is written, and a certificate issues if the
/// pass mark was reached.
pub async fn grade_answer(
    db: &dyn Db,
    grader: i64,
    answer_id: i64,
    marks_awarded: f64,
    feedback: &str,
    is_admin: bool,
    now: u64,
) -> Result<ExamAnswer, WordlessError> {
    let row = db
        .query_one(
            "SELECT ans.*, q.marks AS q_marks, q.type AS q_type, a.exam_id, a.user_id AS student_id, a.status AS attempt_status \
             FROM wl_exam_answers ans \
             JOIN wl_questions q ON q.id = ans.question_id \
             JOIN wl_exam_attempts a ON a.id = ans.attempt_id \
             WHERE ans.id = ? LIMIT 1",
            &[Param::Integer(answer_id)],
        )
        .await?;
    let Some(r) = row else {
        return Err(not_found("Answer not found."));
    };
    if row_text(&r, "q_type") != "short_answer" {
        return Err(bad_request(
            "Only short-answer questions are manually graded.",
        ));
    }
    if !r.get("is_correct").map(|v| v.is_null()).unwrap_or(true) {
        return Err(conflict(
            "This answer has already been graded.",
            "already_graded",
        ));
    }
    let marks = row_int(&r, "q_marks");
    if !(0.0..=marks as f64).contains(&marks_awarded) {
        return Err(bad_request(
            "Marks awarded must be between 0 and the question's marks.",
        ));
    }
    let exam_id = row_int(&r, "exam_id");
    let exam = get_exam(db, exam_id, Some(grader), is_admin)
        .await?
        .ok_or_else(|| not_found("Exam not found."))?;
    require_manage_course(db, grader, exam.course_id, is_admin).await?;
    let awarded = round2(marks_awarded);
    let now_s = iso_from_ms(now);
    db.exec(
        "UPDATE wl_exam_answers SET is_correct = ?, marks_awarded = ?, grader_feedback = ?, graded_at = ? WHERE id = ?",
        &[
            Param::Integer(if awarded >= marks as f64 { 1 } else { 0 }),
            Param::Real(awarded),
            Param::Text(feedback.trim().to_string()),
            Param::Text(now_s.clone()),
            Param::Integer(answer_id),
        ],
    )
    .await?;
    let attempt_id = row_int(&r, "attempt_id");
    // Recompute the attempt score from all graded answers.
    let totals = db
        .query_one(
            "SELECT COALESCE(SUM(marks_awarded), 0) AS s FROM wl_exam_answers WHERE attempt_id = ? AND is_correct IS NOT NULL",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let score = totals
        .map(|t| t.get("s").and_then(|v| v.as_f64()).unwrap_or(0.0))
        .unwrap_or(0.0);
    let score = round2(score);
    let pending = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_exam_answers WHERE attempt_id = ? AND is_correct IS NULL",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let pending = pending.map(|r| row_int(&r, "c")).unwrap_or(0);
    let attempt_row = db
        .query_one(
            "SELECT * FROM wl_exam_attempts WHERE id = ? LIMIT 1",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let Some(ar) = attempt_row else {
        return Err(not_found("Attempt not found."));
    };
    let attempt = row_to_attempt_row(&ar);
    db.exec(
        "UPDATE wl_exam_attempts SET score = ?, updated_at = ? WHERE id = ?",
        &[
            Param::Real(score),
            Param::Text(now_s.clone()),
            Param::Integer(attempt_id),
        ],
    )
    .await?;
    if pending == 0 && attempt.status == "awaiting_grading" {
        db.exec(
            "UPDATE wl_exam_attempts SET status = 'graded', updated_at = ? WHERE id = ?",
            &[Param::Text(now_s.clone()), Param::Integer(attempt_id)],
        )
        .await?;
        let result = upsert_result(
            db,
            attempt_id,
            attempt.user_id,
            &exam,
            score,
            attempt.max_score,
            &now_s,
        )
        .await?;
        if result.passed {
            let _ = maybe_issue_certificate(db, &result, now).await?;
        }
    }
    let fresh = db
        .query_one(
            "SELECT * FROM wl_exam_answers WHERE id = ? LIMIT 1",
            &[Param::Integer(answer_id)],
        )
        .await?;
    fresh
        .map(|r| row_to_answer(&r))
        .ok_or_else(|| not_found("Answer not found."))
}

// ------------------------------------------------------------ results etc.

async fn upsert_result(
    db: &dyn Db,
    attempt_id: i64,
    user_id: i64,
    exam: &Exam,
    score: f64,
    max_score: i64,
    graded_at: &str,
) -> Result<ExamResult, WordlessError> {
    let pct = percent(score, max_score);
    let passed = max_score > 0 && pct >= exam.pass_mark_percent as f64;
    db.exec(
        "INSERT INTO wl_results (attempt_id, user_id, exam_id, course_id, score, max_score, percent, passed, graded_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (attempt_id) DO UPDATE SET score = excluded.score, max_score = excluded.max_score, \
         percent = excluded.percent, passed = excluded.passed, graded_at = excluded.graded_at",
        &[
            Param::Integer(attempt_id),
            Param::Integer(user_id),
            Param::Integer(exam.id),
            Param::Integer(exam.course_id),
            Param::Real(round2(score)),
            Param::Integer(max_score),
            Param::Real(pct),
            Param::Integer(if passed { 1 } else { 0 }),
            Param::Text(graded_at.to_string()),
        ],
    )
    .await?;
    let row = db
        .query_one(
            "SELECT * FROM wl_results WHERE attempt_id = ? LIMIT 1",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    row.map(|r| row_to_result(&r, None))
        .ok_or_else(|| not_found("Result not found."))
}

/// Results for one user (their own), or — for an instructor/admin —
/// every result of one exam. A result exists only once grading is
/// complete; an `awaiting_grading` attempt has no result yet.
pub async fn list_results(
    db: &dyn Db,
    viewer: i64,
    is_admin: bool,
    exam_id: Option<i64>,
) -> Result<Vec<ExamResult>, WordlessError> {
    let (sql, params) = match exam_id {
        Some(eid) => {
            let exam = get_exam(db, eid, Some(viewer), is_admin)
                .await?
                .ok_or_else(|| not_found("Exam not found."))?;
            require_manage_course(db, viewer, exam.course_id, is_admin).await?;
            (
                "SELECT r.* FROM wl_results r WHERE r.exam_id = ? ORDER BY r.graded_at DESC, r.id DESC"
                    .to_string(),
                vec![Param::Integer(eid)],
            )
        }
        None => (
            "SELECT r.* FROM wl_results r WHERE r.user_id = ? ORDER BY r.graded_at DESC, r.id DESC"
                .to_string(),
            vec![Param::Integer(viewer)],
        ),
    };
    let rows = db.query_all(&sql, &params).await?;
    let mut out = Vec::new();
    for r in &rows {
        out.push(row_to_result(r, None));
    }
    // Attach certificate codes (one query per result is fine here —
    // result lists are short; the leaderboard path does not need them).
    for res in &mut out {
        if let Some(cert) = certificate_for_attempt(db, res.attempt_id).await? {
            res.certificate_code = Some(cert.code);
        }
    }
    Ok(out)
}

/// One result. The owner, the course instructor, or an admin may
/// read it. Returns `None` when the attempt is still awaiting
/// grading (no result row exists yet) — the route turns that into a
/// 404 "still being graded", not a 200 with a partial score.
pub async fn get_result(
    db: &dyn Db,
    result_id: i64,
    viewer: i64,
    is_admin: bool,
) -> Result<Option<ExamResult>, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_results WHERE id = ? LIMIT 1",
            &[Param::Integer(result_id)],
        )
        .await?;
    let Some(r) = row else { return Ok(None) };
    let mut result = row_to_result(&r, None);
    if result.user_id != viewer && !is_admin {
        let exam = get_exam(db, result.exam_id, Some(viewer), false)
            .await?
            .ok_or_else(|| not_found("Result not found."))?;
        if !is_instructor(db, viewer, exam.course_id).await? {
            return Err(forbidden("This result belongs to another student."));
        }
    }
    if let Some(cert) = certificate_for_attempt(db, result.attempt_id).await? {
        result.certificate_code = Some(cert.code);
    }
    Ok(Some(result))
}

/// The result of one attempt, if grading is complete.
pub async fn result_for_attempt(
    db: &dyn Db,
    attempt_id: i64,
    viewer: i64,
    is_admin: bool,
) -> Result<Option<ExamResult>, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_results WHERE attempt_id = ? LIMIT 1",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    let Some(r) = row else { return Ok(None) };
    let mut result = row_to_result(&r, None);
    if result.user_id != viewer && !is_admin {
        let exam = get_exam(db, result.exam_id, Some(viewer), false)
            .await?
            .ok_or_else(|| not_found("Result not found."))?;
        if !is_instructor(db, viewer, exam.course_id).await? {
            return Err(forbidden("This result belongs to another student."));
        }
    }
    if let Some(cert) = certificate_for_attempt(db, result.attempt_id).await? {
        result.certificate_code = Some(cert.code);
    }
    Ok(Some(result))
}

/// Leaderboard: public, top 50, score descending, earliest
/// submission first on ties.
pub async fn leaderboard(db: &dyn Db, exam_id: i64) -> Result<Vec<LeaderboardRow>, WordlessError> {
    let _exam = get_exam(db, exam_id, None, true)
        .await?
        .ok_or_else(|| not_found("Exam not found."))?;
    let rows = db
        .query_all(
            "SELECT r.*, COALESCE(u.user_login, '') AS login, COALESCE(u.display_name, '') AS display_name, \
             a.submitted_at_ms AS submitted_at_ms \
             FROM wl_results r \
             JOIN wl_exam_attempts a ON a.id = r.attempt_id \
             LEFT JOIN wp_users u ON u.ID = r.user_id \
             WHERE r.exam_id = ? ORDER BY r.score DESC, a.submitted_at_ms ASC LIMIT 50",
            &[Param::Integer(exam_id)],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| LeaderboardRow {
            user_id: row_int(r, "user_id"),
            login: row_text(r, "login"),
            display_name: row_text(r, "display_name"),
            score: r.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0),
            max_score: row_int(r, "max_score"),
            percent: r.get("percent").and_then(|v| v.as_f64()).unwrap_or(0.0),
            submitted_at_ms: row_int(r, "submitted_at_ms"),
            graded_at: row_text(r, "graded_at"),
        })
        .collect())
}

// ------------------------------------------------------------ certificates

/// Issue a certificate for a passing result, if none exists for the
/// attempt yet. The code is deterministic per attempt, so a second
/// call returns the existing certificate.
pub async fn maybe_issue_certificate(
    db: &dyn Db,
    result: &ExamResult,
    now: u64,
) -> Result<Option<Certificate>, WordlessError> {
    if !result.passed {
        return Ok(None);
    }
    if let Some(existing) = certificate_for_attempt(db, result.attempt_id).await? {
        return Ok(Some(existing));
    }
    let code = certificate_code(result.attempt_id, result.user_id, result.exam_id, now);
    let issued_at = iso_from_ms(now);
    db.exec(
        "INSERT INTO wl_certificates (code, user_id, course_id, exam_id, attempt_id, result_id, issued_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        &[
            Param::Text(code),
            Param::Integer(result.user_id),
            Param::Integer(result.course_id),
            Param::Integer(result.exam_id),
            Param::Integer(result.attempt_id),
            Param::Integer(result.id),
            Param::Text(issued_at),
        ],
    )
    .await?;
    certificate_for_attempt(db, result.attempt_id).await
}

pub async fn certificate_for_attempt(
    db: &dyn Db,
    attempt_id: i64,
) -> Result<Option<Certificate>, WordlessError> {
    let row = db
        .query_one(
            "SELECT c.*, COALESCE(u.user_login, '') AS student_login, COALESCE(u.display_name, '') AS student_name, \
             co.title AS course_title, e.title AS exam_title, r.score AS score, r.max_score AS max_score, r.percent AS percent \
             FROM wl_certificates c \
             LEFT JOIN wp_users u ON u.ID = c.user_id \
             LEFT JOIN wl_courses co ON co.id = c.course_id \
             LEFT JOIN wl_exams e ON e.id = c.exam_id \
             LEFT JOIN wl_results r ON r.id = c.result_id \
             WHERE c.attempt_id = ? LIMIT 1",
            &[Param::Integer(attempt_id)],
        )
        .await?;
    Ok(row.map(|r| row_to_certificate(&r)))
}

/// Public verification lookup by code.
pub async fn get_certificate_by_code(
    db: &dyn Db,
    code: &str,
) -> Result<Option<Certificate>, WordlessError> {
    let row = db
        .query_one(
            "SELECT c.*, COALESCE(u.user_login, '') AS student_login, COALESCE(u.display_name, '') AS student_name, \
             co.title AS course_title, e.title AS exam_title, r.score AS score, r.max_score AS max_score, r.percent AS percent \
             FROM wl_certificates c \
             LEFT JOIN wp_users u ON u.ID = c.user_id \
             LEFT JOIN wl_courses co ON co.id = c.course_id \
             LEFT JOIN wl_exams e ON e.id = c.exam_id \
             LEFT JOIN wl_results r ON r.id = c.result_id \
             WHERE c.code = ? LIMIT 1",
            &[Param::Text(code.trim().to_string())],
        )
        .await?;
    Ok(row.map(|r| row_to_certificate(&r)))
}

/// Certificates for one user (their own), or — for an admin — all.
/// An instructor can filter by their course.
pub async fn list_certificates(
    db: &dyn Db,
    viewer: i64,
    is_admin: bool,
    course_id: Option<i64>,
) -> Result<Vec<Certificate>, WordlessError> {
    let (sql, params) = match course_id {
        Some(cid) => {
            let _course = get_course(db, cid, Some(viewer), is_admin)
                .await?
                .ok_or_else(|| not_found("Course not found."))?;
            require_manage_course(db, viewer, cid, is_admin).await?;
            (
                "SELECT c.*, COALESCE(u.user_login, '') AS student_login, COALESCE(u.display_name, '') AS student_name, \
                 co.title AS course_title, e.title AS exam_title, r.score AS score, r.max_score AS max_score, r.percent AS percent \
                 FROM wl_certificates c \
                 LEFT JOIN wp_users u ON u.ID = c.user_id \
                 LEFT JOIN wl_courses co ON co.id = c.course_id \
                 LEFT JOIN wl_exams e ON e.id = c.exam_id \
                 LEFT JOIN wl_results r ON r.id = c.result_id \
                 WHERE c.course_id = ? ORDER BY c.issued_at DESC, c.id DESC"
                    .to_string(),
                vec![Param::Integer(cid)],
            )
        }
        None => {
            if is_admin {
                (
                    "SELECT c.*, COALESCE(u.user_login, '') AS student_login, COALESCE(u.display_name, '') AS student_name, \
                     co.title AS course_title, e.title AS exam_title, r.score AS score, r.max_score AS max_score, r.percent AS percent \
                     FROM wl_certificates c \
                     LEFT JOIN wp_users u ON u.ID = c.user_id \
                     LEFT JOIN wl_courses co ON co.id = c.course_id \
                     LEFT JOIN wl_exams e ON e.id = c.exam_id \
                     LEFT JOIN wl_results r ON r.id = c.result_id \
                     ORDER BY c.issued_at DESC, c.id DESC"
                        .to_string(),
                    vec![],
                )
            } else {
                (
                    "SELECT c.*, COALESCE(u.user_login, '') AS student_login, COALESCE(u.display_name, '') AS student_name, \
                     co.title AS course_title, e.title AS exam_title, r.score AS score, r.max_score AS max_score, r.percent AS percent \
                     FROM wl_certificates c \
                     LEFT JOIN wp_users u ON u.ID = c.user_id \
                     LEFT JOIN wl_courses co ON co.id = c.course_id \
                     LEFT JOIN wl_exams e ON e.id = c.exam_id \
                     LEFT JOIN wl_results r ON r.id = c.result_id \
                     WHERE c.user_id = ? ORDER BY c.issued_at DESC, c.id DESC"
                        .to_string(),
                    vec![Param::Integer(viewer)],
                )
            }
        }
    };
    let rows = db.query_all(&sql, &params).await?;
    Ok(rows.iter().map(row_to_certificate).collect())
}

// ------------------------------------------------------------------ rows

fn row_to_exam(row: &wordless_db::Row) -> Exam {
    let question_ids: Vec<i64> =
        serde_json::from_str(&row_text(row, "question_ids_json")).unwrap_or_default();
    Exam {
        id: row_int(row, "id"),
        course_id: row_int(row, "course_id"),
        bank_id: row_int(row, "bank_id"),
        title: row_text(row, "title"),
        description: row_text(row, "description"),
        duration_minutes: row_int(row, "duration_minutes"),
        window_start: row_opt_text(row, "window_start"),
        window_end: row_opt_text(row, "window_end"),
        max_attempts: row_int(row, "max_attempts"),
        pass_mark_percent: row_int(row, "pass_mark_percent"),
        shuffle_questions: row_int(row, "shuffle_questions") == 1,
        question_ids,
        status: row_text(row, "status"),
        question_count: 0,
        attempts_used: 0,
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

fn row_to_answer(row: &wordless_db::Row) -> ExamAnswer {
    let answer: serde_json::Value =
        serde_json::from_str(&row_text(row, "answer_json")).unwrap_or(serde_json::Value::Null);
    let is_correct = match row.get("is_correct") {
        Some(v) if !v.is_null() => Some(row_int(row, "is_correct") == 1),
        _ => None,
    };
    let marks_awarded = match row.get("marks_awarded") {
        Some(v) if !v.is_null() => Some(v.as_f64().unwrap_or(0.0)),
        _ => None,
    };
    // An ungraded saved answer has marks_awarded 0.0 in the row —
    // report it as ungraded (None), not as a zero.
    let marks_awarded = if is_correct.is_none() {
        None
    } else {
        marks_awarded
    };
    ExamAnswer {
        question_id: row_int(row, "question_id"),
        answer,
        is_correct,
        marks_awarded,
        grader_feedback: row_text(row, "grader_feedback"),
        graded_at: row_opt_text(row, "graded_at"),
    }
}

fn row_to_result(row: &wordless_db::Row, certificate_code: Option<String>) -> ExamResult {
    ExamResult {
        id: row_int(row, "id"),
        attempt_id: row_int(row, "attempt_id"),
        user_id: row_int(row, "user_id"),
        exam_id: row_int(row, "exam_id"),
        course_id: row_int(row, "course_id"),
        score: row.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0),
        max_score: row_int(row, "max_score"),
        percent: row.get("percent").and_then(|v| v.as_f64()).unwrap_or(0.0),
        passed: row_int(row, "passed") == 1,
        certificate_code,
        graded_at: row_text(row, "graded_at"),
    }
}

fn row_to_certificate(row: &wordless_db::Row) -> Certificate {
    Certificate {
        id: row_int(row, "id"),
        code: row_text(row, "code"),
        user_id: row_int(row, "user_id"),
        student_login: row_text(row, "student_login"),
        student_name: row_text(row, "student_name"),
        course_id: row_int(row, "course_id"),
        course_title: row_text(row, "course_title"),
        exam_id: row_int(row, "exam_id"),
        exam_title: row_text(row, "exam_title"),
        attempt_id: row_int(row, "attempt_id"),
        result_id: row_int(row, "result_id"),
        score: row.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0),
        max_score: row_int(row, "max_score"),
        percent: row.get("percent").and_then(|v| v.as_f64()).unwrap_or(0.0),
        issued_at: row_text(row, "issued_at"),
    }
}
