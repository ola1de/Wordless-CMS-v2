//! Shared types for the school module — the JSON shapes the API
//! returns (camelCase, the v1 `{ok, data, error}` envelope wraps
//! them) and the small input structs the services take.

use serde::{Deserialize, Serialize};

/// A course (a class a student enrols in).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Course {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub status: String,
    #[serde(rename = "instructorId")]
    pub instructor_id: i64,
    #[serde(rename = "instructorLogin")]
    pub instructor_login: String,
    #[serde(rename = "instructorName")]
    pub instructor_name: String,
    #[serde(rename = "lessonCount")]
    pub lesson_count: i64,
    #[serde(rename = "enrolmentCount")]
    pub enrolment_count: i64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// A lesson inside a course, in `position` order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub id: i64,
    #[serde(rename = "courseId")]
    pub course_id: i64,
    pub title: String,
    pub content: String,
    pub position: i64,
    #[serde(rename = "durationMinutes")]
    pub duration_minutes: i64,
    #[serde(rename = "completed")]
    pub completed: bool,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// One enrolment of one user in one course. `login` / `displayName`
/// are populated on the instructor's enrolment list; they are empty
/// on a single-enrolment read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Enrolment {
    pub id: i64,
    #[serde(rename = "courseId")]
    pub course_id: i64,
    #[serde(rename = "userId")]
    pub user_id: i64,
    pub login: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub status: String,
    #[serde(rename = "enrolledAt")]
    pub enrolled_at: String,
    #[serde(rename = "completedAt")]
    pub completed_at: Option<String>,
    #[serde(rename = "lessonsCompleted")]
    pub lessons_completed: i64,
    #[serde(rename = "lessonsTotal")]
    pub lessons_total: i64,
}

/// A question bank — course-scoped; an exam draws from one bank of
/// its own course.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionBank {
    pub id: i64,
    #[serde(rename = "courseId")]
    pub course_id: i64,
    pub title: String,
    pub description: String,
    #[serde(rename = "questionCount")]
    pub question_count: i64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// A question. `answer` is the answer KEY — it is serialised only
/// for instructors/admins (see `to_student_json` in questions.rs);
/// the student-facing exam payload never carries it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub id: i64,
    #[serde(rename = "bankId")]
    pub bank_id: i64,
    #[serde(rename = "type")]
    pub qtype: String,
    #[serde(rename = "questionText")]
    pub question_text: String,
    pub options: Vec<String>,
    /// Answer key: mcq -> number (option index); true_false ->
    /// boolean; multi_select -> array of numbers; short_answer -> null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<serde_json::Value>,
    #[serde(rename = "modelAnswer")]
    pub model_answer: String,
    pub marks: i64,
    pub difficulty: String,
    pub topic: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// An exam: a timed, windowed, attempt-limited assessment of one
/// course's bank.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exam {
    pub id: i64,
    #[serde(rename = "courseId")]
    pub course_id: i64,
    #[serde(rename = "bankId")]
    pub bank_id: i64,
    pub title: String,
    pub description: String,
    #[serde(rename = "durationMinutes")]
    pub duration_minutes: i64,
    #[serde(rename = "windowStart")]
    pub window_start: Option<String>,
    #[serde(rename = "windowEnd")]
    pub window_end: Option<String>,
    #[serde(rename = "maxAttempts")]
    pub max_attempts: i64,
    #[serde(rename = "passMarkPercent")]
    pub pass_mark_percent: i64,
    #[serde(rename = "shuffleQuestions")]
    pub shuffle_questions: bool,
    #[serde(rename = "questionIds")]
    pub question_ids: Vec<i64>,
    pub status: String,
    #[serde(rename = "questionCount")]
    pub question_count: i64,
    #[serde(rename = "attemptsUsed")]
    pub attempts_used: i64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// One question inside a started attempt — WITHOUT the answer key.
/// This is the only question shape a student ever sees.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptQuestion {
    #[serde(rename = "questionId")]
    pub question_id: i64,
    #[serde(rename = "type")]
    pub qtype: String,
    #[serde(rename = "questionText")]
    pub question_text: String,
    pub options: Vec<String>,
    pub marks: i64,
}

/// An exam attempt. `deadlineMs` is the server-side deadline
/// (epoch ms); the client must never compute it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExamAttempt {
    pub id: i64,
    #[serde(rename = "examId")]
    pub exam_id: i64,
    #[serde(rename = "userId")]
    pub user_id: i64,
    pub status: String,
    #[serde(rename = "startedAtMs")]
    pub started_at_ms: i64,
    #[serde(rename = "deadlineMs")]
    pub deadline_ms: i64,
    #[serde(rename = "submittedAtMs")]
    pub submitted_at_ms: Option<i64>,
    #[serde(rename = "questionOrder")]
    pub question_order: Vec<i64>,
    pub score: f64,
    #[serde(rename = "maxScore")]
    pub max_score: i64,
    #[serde(rename = "questions")]
    pub questions: Vec<AttemptQuestion>,
    #[serde(rename = "answers")]
    pub answers: Vec<ExamAnswer>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// One saved answer inside an attempt. `isCorrect` / `marksAwarded`
/// are null while the answer is ungraded (short-answer before manual
/// grading, or any answer while the attempt is still in progress).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExamAnswer {
    #[serde(rename = "questionId")]
    pub question_id: i64,
    pub answer: serde_json::Value,
    #[serde(rename = "isCorrect")]
    pub is_correct: Option<bool>,
    #[serde(rename = "marksAwarded")]
    pub marks_awarded: Option<f64>,
    #[serde(rename = "graderFeedback")]
    pub grader_feedback: String,
    #[serde(rename = "gradedAt")]
    pub graded_at: Option<String>,
}

/// A graded result — one snapshot per finished attempt. `passed` is
/// `score / maxScore >= pass mark`. `certificateCode` is present
/// only when a certificate was issued for this result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExamResult {
    pub id: i64,
    #[serde(rename = "attemptId")]
    pub attempt_id: i64,
    #[serde(rename = "userId")]
    pub user_id: i64,
    #[serde(rename = "examId")]
    pub exam_id: i64,
    #[serde(rename = "courseId")]
    pub course_id: i64,
    pub score: f64,
    #[serde(rename = "maxScore")]
    pub max_score: i64,
    pub percent: f64,
    pub passed: bool,
    #[serde(rename = "certificateCode")]
    pub certificate_code: Option<String>,
    #[serde(rename = "gradedAt")]
    pub graded_at: String,
}

/// One leaderboard row: score first, then earliest submission wins
/// ties (doc §3 P9 decision).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaderboardRow {
    #[serde(rename = "userId")]
    pub user_id: i64,
    pub login: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub score: f64,
    #[serde(rename = "maxScore")]
    pub max_score: i64,
    pub percent: f64,
    #[serde(rename = "submittedAtMs")]
    pub submitted_at_ms: i64,
    #[serde(rename = "gradedAt")]
    pub graded_at: String,
}

/// One pending short-answer item in the manual grading queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradingQueueItem {
    #[serde(rename = "answerId")]
    pub answer_id: i64,
    #[serde(rename = "attemptId")]
    pub attempt_id: i64,
    #[serde(rename = "examId")]
    pub exam_id: i64,
    #[serde(rename = "examTitle")]
    pub exam_title: String,
    #[serde(rename = "courseId")]
    pub course_id: i64,
    #[serde(rename = "questionId")]
    pub question_id: i64,
    #[serde(rename = "questionText")]
    pub question_text: String,
    #[serde(rename = "modelAnswer")]
    pub model_answer: String,
    pub marks: i64,
    #[serde(rename = "studentId")]
    pub student_id: i64,
    #[serde(rename = "studentLogin")]
    pub student_login: String,
    #[serde(rename = "studentName")]
    pub student_name: String,
    /// The student's written answer (a string; null only if the
    /// student left it blank).
    pub answer: serde_json::Value,
    #[serde(rename = "submittedAtMs")]
    pub submitted_at_ms: i64,
}

/// A certificate. `code` is the public verification key
/// (`WL-` + 16 hex chars). The HTML page is rendered by the worker
/// from this record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Certificate {
    pub id: i64,
    pub code: String,
    #[serde(rename = "userId")]
    pub user_id: i64,
    #[serde(rename = "studentLogin")]
    pub student_login: String,
    #[serde(rename = "studentName")]
    pub student_name: String,
    #[serde(rename = "courseId")]
    pub course_id: i64,
    #[serde(rename = "courseTitle")]
    pub course_title: String,
    #[serde(rename = "examId")]
    pub exam_id: i64,
    #[serde(rename = "examTitle")]
    pub exam_title: String,
    #[serde(rename = "attemptId")]
    pub attempt_id: i64,
    #[serde(rename = "resultId")]
    pub result_id: i64,
    pub score: f64,
    #[serde(rename = "maxScore")]
    pub max_score: i64,
    pub percent: f64,
    #[serde(rename = "issuedAt")]
    pub issued_at: String,
}
