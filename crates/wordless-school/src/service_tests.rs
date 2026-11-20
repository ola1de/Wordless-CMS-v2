//! Service tests: the full student journey plus the targeted
//! rules (attempt limits, windows, deadlines, manual grading,
//! validation). They run the real SQL from migration 0015 against
//! in-memory SQLite through `TestDb`, with a fixed clock so timing
//! assertions are deterministic.

use serde_json::json;

use crate::courses;
use crate::exams;
use crate::questions;
use crate::test_support::{block_on, TestDb};

const TEACHER: i64 = 1;
const STUDENT: i64 = 2;
const OTHER: i64 = 3;
/// Fixed "now": 2026-11-20T00:00:00Z-ish (epoch ms).
const NOW: u64 = 1_794_643_200_000;

fn setup() -> TestDb {
    let db = TestDb::new();
    db.seed_user(TEACHER, "teacher", "Teacher");
    db.seed_user(STUDENT, "student", "Student");
    db.seed_user(OTHER, "other", "Other");
    db
}

fn published_course(db: &TestDb) -> i64 {
    block_on(async {
        let course = courses::create_course(db, TEACHER, "Rust 101", "Learn Rust.")
            .await
            .unwrap();
        courses::update_course(
            db,
            TEACHER,
            course.id,
            &courses::UpdateCourseInput {
                title: None,
                description: None,
                status: Some("published".to_string()),
            },
            false,
        )
        .await
        .unwrap()
        .id
    })
}

fn bank_with_questions(db: &TestDb, course_id: i64) -> (i64, Vec<i64>) {
    block_on(async {
        let bank = questions::create_bank(db, TEACHER, course_id, "Week 1", "", false)
            .await
            .unwrap();
        let mut ids = Vec::new();
        // mcq, 5 marks, correct option index 1.
        let q1 = questions::create_question(
            db,
            TEACHER,
            bank.id,
            &questions::CreateQuestionInput {
                qtype: "mcq".to_string(),
                question_text: "What is 2 + 2?".to_string(),
                options: vec!["3".to_string(), "4".to_string(), "5".to_string()],
                answer: json!(1),
                model_answer: String::new(),
                marks: 5,
                difficulty: "easy".to_string(),
                topic: "arithmetic".to_string(),
            },
            false,
        )
        .await
        .unwrap();
        ids.push(q1.id);
        // true/false, 2 marks, true.
        let q2 = questions::create_question(
            db,
            TEACHER,
            bank.id,
            &questions::CreateQuestionInput {
                qtype: "true_false".to_string(),
                question_text: "Rust is memory-safe.".to_string(),
                options: vec![],
                answer: json!(true),
                model_answer: String::new(),
                marks: 2,
                difficulty: "easy".to_string(),
                topic: "rust".to_string(),
            },
            false,
        )
        .await
        .unwrap();
        ids.push(q2.id);
        // multi-select, 4 marks, correct [0, 2].
        let q3 = questions::create_question(
            db,
            TEACHER,
            bank.id,
            &questions::CreateQuestionInput {
                qtype: "multi_select".to_string(),
                question_text: "Which are prime?".to_string(),
                options: vec!["2".to_string(), "4".to_string(), "5".to_string()],
                answer: json!([0, 2]),
                model_answer: String::new(),
                marks: 4,
                difficulty: "medium".to_string(),
                topic: "arithmetic".to_string(),
            },
            false,
        )
        .await
        .unwrap();
        ids.push(q3.id);
        (bank.id, ids)
    })
}

fn published_exam(db: &TestDb, course_id: i64, bank_id: i64, qids: &[i64]) -> i64 {
    block_on(async {
        let exam = exams::create_exam(
            db,
            TEACHER,
            course_id,
            bank_id,
            &exams::CreateExamInput {
                title: "Week 1 Quiz".to_string(),
                description: String::new(),
                duration_minutes: 30,
                window_start: None,
                window_end: None,
                max_attempts: 1,
                pass_mark_percent: 50,
                shuffle_questions: true,
                question_ids: qids.to_vec(),
            },
            false,
        )
        .await
        .unwrap();
        exams::update_exam(
            db,
            TEACHER,
            exam.id,
            &exams::UpdateExamInput {
                title: None,
                description: None,
                duration_minutes: None,
                window_start: None,
                window_end: None,
                max_attempts: None,
                pass_mark_percent: None,
                shuffle_questions: None,
                question_ids: None,
                status: Some("published".to_string()),
            },
            false,
        )
        .await
        .unwrap()
        .id
    })
}

#[test]
fn full_student_journey_to_certificate() {
    let db = setup();
    let course_id = published_course(&db);
    // Two lessons; completing both completes the course.
    block_on(async {
        let l1 =
            courses::create_lesson(&db, TEACHER, course_id, "Intro", "Welcome.", None, 5, false)
                .await
                .unwrap();
        let l2 = courses::create_lesson(
            &db, TEACHER, course_id, "Basics", "Content.", None, 10, false,
        )
        .await
        .unwrap();
        let enrol = courses::enrol(&db, STUDENT, course_id).await.unwrap();
        assert_eq!(enrol.status, "active");
        assert_eq!(enrol.lessons_total, 2);
        let after1 = courses::complete_lesson(&db, STUDENT, l1.id).await.unwrap();
        assert_eq!(after1.status, "active");
        assert_eq!(after1.lessons_completed, 1);
        let after2 = courses::complete_lesson(&db, STUDENT, l2.id).await.unwrap();
        assert_eq!(after2.status, "completed");
        assert!(after2.completed_at.is_some());
    });
    let (bank_id, qids) = bank_with_questions(&db, course_id);
    let exam_id = published_exam(&db, course_id, bank_id, &qids);
    block_on(async {
        // Enrolment is required to start; the student is enrolled.
        let attempt = exams::start_attempt(&db, STUDENT, exam_id, "", NOW)
            .await
            .unwrap();
        assert_eq!(attempt.status, "in_progress");
        assert_eq!(attempt.deadline_ms, (NOW + 30 * 60_000) as i64);
        assert_eq!(attempt.max_score, 11);
        assert_eq!(attempt.questions.len(), 3);
        // The start payload carries NO answer keys: each question
        // object must have no "answer" or "modelAnswer" key. (The
        // attempt also has an "answers" array — the student's own
        // saved answers — which is a different field and is fine.)
        let payload: serde_json::Value = serde_json::to_value(&attempt).unwrap();
        for q in payload["questions"].as_array().unwrap() {
            assert!(q.get("answer").is_none(), "answer key leaked: {q}");
            assert!(q.get("modelAnswer").is_none(), "model answer leaked: {q}");
        }
        // Resume: starting again returns the SAME attempt.
        let resumed = exams::start_attempt(&db, STUDENT, exam_id, "", NOW + 1_000)
            .await
            .unwrap();
        assert_eq!(resumed.id, attempt.id);
        // Answer everything correctly (multi-select full marks).
        let q = &attempt.questions;
        let mcq = q.iter().find(|x| x.qtype == "mcq").unwrap();
        let tf = q.iter().find(|x| x.qtype == "true_false").unwrap();
        let ms = q.iter().find(|x| x.qtype == "multi_select").unwrap();
        exams::save_answer(
            &db,
            STUDENT,
            attempt.id,
            mcq.question_id,
            json!(1),
            NOW + 2_000,
        )
        .await
        .unwrap();
        exams::save_answer(
            &db,
            STUDENT,
            attempt.id,
            tf.question_id,
            json!(true),
            NOW + 3_000,
        )
        .await
        .unwrap();
        exams::save_answer(
            &db,
            STUDENT,
            attempt.id,
            ms.question_id,
            json!([0, 2]),
            NOW + 4_000,
        )
        .await
        .unwrap();
        let submitted = exams::submit_attempt(&db, STUDENT, attempt.id, NOW + 5_000)
            .await
            .unwrap();
        assert_eq!(submitted.status, "submitted");
        assert_eq!(submitted.score, 11.0);
        // Result: 100%, passed, certificate issued.
        let result = exams::result_for_attempt(&db, attempt.id, STUDENT, false)
            .await
            .unwrap()
            .expect("result must exist after grading");
        assert_eq!(result.percent, 100.0);
        assert!(result.passed);
        let code = result
            .certificate_code
            .clone()
            .expect("certificate must issue on pass");
        assert!(code.starts_with("WL-"));
        // Per-question review on the attempt detail.
        let detail = exams::get_attempt(&db, attempt.id, STUDENT, false, NOW + 6_000)
            .await
            .unwrap()
            .unwrap();
        assert!(detail.answers.iter().all(|a| a.is_correct == Some(true)));
        assert!(detail.answers.iter().all(|a| a.marks_awarded.is_some()));
        // Leaderboard: the student is ranked first.
        let board = exams::leaderboard(&db, exam_id).await.unwrap();
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].user_id, STUDENT);
        assert_eq!(board[0].score, 11.0);
        // Certificate verifies by code.
        let cert = exams::get_certificate_by_code(&db, &code)
            .await
            .unwrap()
            .expect("cert");
        assert_eq!(cert.student_login, "student");
        assert_eq!(cert.course_title, "Rust 101");
        assert_eq!(cert.percent, 100.0);
        // Second attempt is blocked (max_attempts = 1).
        let again = exams::start_attempt(&db, STUDENT, exam_id, "", NOW + 7_000).await;
        assert!(again.is_err());
        assert_eq!(again.unwrap_err().code, "attempt_limit");
    });
}

#[test]
fn non_enrolled_student_cannot_start() {
    let db = setup();
    let course_id = published_course(&db);
    let (bank_id, qids) = bank_with_questions(&db, course_id);
    let exam_id = published_exam(&db, course_id, bank_id, &qids);
    block_on(async {
        let err = exams::start_attempt(&db, OTHER, exam_id, "", NOW)
            .await
            .unwrap_err();
        assert_eq!(err.status, 403);
    });
}

#[test]
fn exam_window_is_enforced() {
    let db = setup();
    let course_id = published_course(&db);
    block_on(async {
        courses::enrol(&db, STUDENT, course_id).await.unwrap();
    });
    let (bank_id, qids) = bank_with_questions(&db, course_id);
    // Window opens in the future.
    let future_exam = block_on(async {
        let exam = exams::create_exam(
            &db,
            TEACHER,
            course_id,
            bank_id,
            &exams::CreateExamInput {
                title: "Future Quiz".to_string(),
                description: String::new(),
                duration_minutes: 30,
                window_start: Some("2026-12-01T00:00:00.000Z".to_string()),
                window_end: Some("2026-12-02T00:00:00.000Z".to_string()),
                max_attempts: 1,
                pass_mark_percent: 50,
                shuffle_questions: false,
                question_ids: qids.clone(),
            },
            false,
        )
        .await
        .unwrap();
        exams::update_exam(
            &db,
            TEACHER,
            exam.id,
            &exams::UpdateExamInput {
                title: None,
                description: None,
                duration_minutes: None,
                window_start: None,
                window_end: None,
                max_attempts: None,
                pass_mark_percent: None,
                shuffle_questions: None,
                question_ids: None,
                status: Some("published".to_string()),
            },
            false,
        )
        .await
        .unwrap()
        .id
    });
    block_on(async {
        let err = exams::start_attempt(&db, STUDENT, future_exam, "", NOW)
            .await
            .unwrap_err();
        assert_eq!(err.code, "exam_not_open");
        // A window that already ended is closed too.
        let past = exams::create_exam(
            &db,
            TEACHER,
            course_id,
            bank_id,
            &exams::CreateExamInput {
                title: "Past Quiz".to_string(),
                description: String::new(),
                duration_minutes: 30,
                window_start: Some("2026-01-01T00:00:00.000Z".to_string()),
                window_end: Some("2026-01-02T00:00:00.000Z".to_string()),
                max_attempts: 1,
                pass_mark_percent: 50,
                shuffle_questions: false,
                question_ids: qids.clone(),
            },
            false,
        )
        .await
        .unwrap();
        exams::update_exam(
            &db,
            TEACHER,
            past.id,
            &exams::UpdateExamInput {
                title: None,
                description: None,
                duration_minutes: None,
                window_start: None,
                window_end: None,
                max_attempts: None,
                pass_mark_percent: None,
                shuffle_questions: None,
                question_ids: None,
                status: Some("published".to_string()),
            },
            false,
        )
        .await
        .unwrap();
        let err = exams::start_attempt(&db, STUDENT, past.id, "", NOW)
            .await
            .unwrap_err();
        assert_eq!(err.code, "exam_window_closed");
    });
}

#[test]
fn late_submit_is_rejected_but_auto_submits() {
    let db = setup();
    let course_id = published_course(&db);
    block_on(async {
        courses::enrol(&db, STUDENT, course_id).await.unwrap();
    });
    let (bank_id, qids) = bank_with_questions(&db, course_id);
    // 1-minute exam so the deadline is easy to pass.
    let exam_id = block_on(async {
        let exam = exams::create_exam(
            &db,
            TEACHER,
            course_id,
            bank_id,
            &exams::CreateExamInput {
                title: "Sprint".to_string(),
                description: String::new(),
                duration_minutes: 1,
                window_start: None,
                window_end: None,
                max_attempts: 1,
                pass_mark_percent: 50,
                shuffle_questions: false,
                question_ids: qids.clone(),
            },
            false,
        )
        .await
        .unwrap();
        exams::update_exam(
            &db,
            TEACHER,
            exam.id,
            &exams::UpdateExamInput {
                title: None,
                description: None,
                duration_minutes: None,
                window_start: None,
                window_end: None,
                max_attempts: None,
                pass_mark_percent: None,
                shuffle_questions: None,
                question_ids: None,
                status: Some("published".to_string()),
            },
            false,
        )
        .await
        .unwrap()
        .id
    });
    block_on(async {
        let attempt = exams::start_attempt(&db, STUDENT, exam_id, "", NOW)
            .await
            .unwrap();
        // Answer the mcq correctly, then submit 2 minutes later —
        // past the 1-minute deadline.
        let mcq = attempt.questions.iter().find(|x| x.qtype == "mcq").unwrap();
        exams::save_answer(
            &db,
            STUDENT,
            attempt.id,
            mcq.question_id,
            json!(1),
            NOW + 10_000,
        )
        .await
        .unwrap();
        let err = exams::submit_attempt(&db, STUDENT, attempt.id, NOW + 120_000)
            .await
            .unwrap_err();
        assert_eq!(err.code, "attempt_expired");
        // The attempt WAS auto-submitted: status expired, result
        // exists with the saved answer scored (5 of 11 = 45.45%).
        let detail = exams::get_attempt(&db, attempt.id, STUDENT, false, NOW + 121_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.status, "expired");
        assert_eq!(detail.score, 5.0);
        let result = exams::result_for_attempt(&db, attempt.id, STUDENT, false)
            .await
            .unwrap()
            .expect("expired attempt still gets a result");
        assert_eq!(result.percent, 45.45);
        assert!(!result.passed);
        assert!(result.certificate_code.is_none());
        // A late ANSWER save is rejected the same way.
        let attempt2 = exams::start_attempt(&db, STUDENT, exam_id, "", NOW + 200_000).await;
        assert!(
            attempt2.is_err(),
            "attempt limit is 1 — the expired attempt counts"
        );
    });
}

#[test]
fn sweep_closes_expired_attempts() {
    let db = setup();
    let course_id = published_course(&db);
    block_on(async {
        courses::enrol(&db, STUDENT, course_id).await.unwrap();
    });
    let (bank_id, qids) = bank_with_questions(&db, course_id);
    let exam_id = block_on(async {
        let exam = exams::create_exam(
            &db,
            TEACHER,
            course_id,
            bank_id,
            &exams::CreateExamInput {
                title: "Sweep Quiz".to_string(),
                description: String::new(),
                duration_minutes: 1,
                window_start: None,
                window_end: None,
                max_attempts: 3,
                pass_mark_percent: 50,
                shuffle_questions: false,
                question_ids: qids.clone(),
            },
            false,
        )
        .await
        .unwrap();
        exams::update_exam(
            &db,
            TEACHER,
            exam.id,
            &exams::UpdateExamInput {
                title: None,
                description: None,
                duration_minutes: None,
                window_start: None,
                window_end: None,
                max_attempts: None,
                pass_mark_percent: None,
                shuffle_questions: None,
                question_ids: None,
                status: Some("published".to_string()),
            },
            false,
        )
        .await
        .unwrap()
        .id
    });
    block_on(async {
        let attempt = exams::start_attempt(&db, STUDENT, exam_id, "", NOW)
            .await
            .unwrap();
        // Nothing expired yet.
        assert_eq!(
            exams::close_expired_attempts(&db, NOW + 30_000)
                .await
                .unwrap(),
            0
        );
        // Past the deadline the sweep closes it (submitted_at_ms is
        // the deadline itself).
        assert_eq!(
            exams::close_expired_attempts(&db, NOW + 90_000)
                .await
                .unwrap(),
            1
        );
        let detail = exams::get_attempt(&db, attempt.id, STUDENT, false, NOW + 91_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.status, "expired");
        assert_eq!(detail.submitted_at_ms, Some((NOW + 60_000) as i64));
        // The sweep is idempotent.
        assert_eq!(
            exams::close_expired_attempts(&db, NOW + 120_000)
                .await
                .unwrap(),
            0
        );
    });
}

#[test]
fn short_answer_queues_for_manual_grading() {
    let db = setup();
    let course_id = published_course(&db);
    block_on(async {
        courses::enrol(&db, STUDENT, course_id).await.unwrap();
    });
    let (bank_id, mut qids) = bank_with_questions(&db, course_id);
    // Add a short-answer question, 5 marks.
    let short_id = block_on(async {
        questions::create_question(
            &db,
            TEACHER,
            bank_id,
            &questions::CreateQuestionInput {
                qtype: "short_answer".to_string(),
                question_text: "What is the capital of Nigeria?".to_string(),
                options: vec![],
                answer: json!(null),
                model_answer: "Abuja".to_string(),
                marks: 5,
                difficulty: "easy".to_string(),
                topic: "geography".to_string(),
            },
            false,
        )
        .await
        .unwrap()
        .id
    });
    qids.push(short_id);
    let exam_id = published_exam(&db, course_id, bank_id, &qids);
    block_on(async {
        let attempt = exams::start_attempt(&db, STUDENT, exam_id, "", NOW)
            .await
            .unwrap();
        assert_eq!(attempt.max_score, 16);
        let short = attempt
            .questions
            .iter()
            .find(|x| x.qtype == "short_answer")
            .unwrap();
        let mcq = attempt.questions.iter().find(|x| x.qtype == "mcq").unwrap();
        exams::save_answer(
            &db,
            STUDENT,
            attempt.id,
            mcq.question_id,
            json!(1),
            NOW + 1_000,
        )
        .await
        .unwrap();
        exams::save_answer(
            &db,
            STUDENT,
            attempt.id,
            short.question_id,
            json!("Abuja"),
            NOW + 2_000,
        )
        .await
        .unwrap();
        let submitted = exams::submit_attempt(&db, STUDENT, attempt.id, NOW + 3_000)
            .await
            .unwrap();
        assert_eq!(submitted.status, "awaiting_grading");
        assert_eq!(submitted.score, 5.0);
        // No result yet — a partial score is never a final result.
        assert!(exams::result_for_attempt(&db, attempt.id, STUDENT, false)
            .await
            .unwrap()
            .is_none());
        // The grading queue has the one short answer, with the model
        // answer for the grader.
        let queue = exams::grading_queue(&db, TEACHER, None, false)
            .await
            .unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].question_id, short_id);
        assert_eq!(queue[0].model_answer, "Abuja");
        assert_eq!(queue[0].answer, json!("Abuja"));
        // A student instructs no courses, so their queue is empty
        // (the route guard, not the service, is what refuses them
        // with 403 — subscribers lack `edit_posts`).
        let student_queue = exams::grading_queue(&db, STUDENT, None, false)
            .await
            .unwrap();
        assert!(student_queue.is_empty());
        // Grade it full marks: attempt becomes graded, result
        // appears (10 of 16 = 62.5%, passed), certificate issues.
        let graded = exams::grade_answer(
            &db,
            TEACHER,
            queue[0].answer_id,
            5.0,
            "Correct.",
            false,
            NOW + 4_000,
        )
        .await
        .unwrap();
        assert_eq!(graded.is_correct, Some(true));
        assert_eq!(graded.marks_awarded, Some(5.0));
        let detail = exams::get_attempt(&db, attempt.id, STUDENT, false, NOW + 5_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.status, "graded");
        assert_eq!(detail.score, 10.0);
        let result = exams::result_for_attempt(&db, attempt.id, STUDENT, false)
            .await
            .unwrap()
            .expect("result after manual grading");
        assert_eq!(result.percent, 62.5);
        assert!(result.passed);
        assert!(result.certificate_code.is_some());
        // Grading the same answer twice is refused.
        let again = exams::grade_answer(
            &db,
            TEACHER,
            queue[0].answer_id,
            5.0,
            "",
            false,
            NOW + 6_000,
        )
        .await;
        assert!(again.is_err());
        assert_eq!(again.unwrap_err().code, "already_graded");
    });
}

#[test]
fn multi_select_partial_credit_in_a_real_exam() {
    let db = setup();
    let course_id = published_course(&db);
    block_on(async {
        courses::enrol(&db, STUDENT, course_id).await.unwrap();
    });
    let (bank_id, qids) = bank_with_questions(&db, course_id);
    // Exam with ONLY the multi-select question (4 marks).
    let ms_only: Vec<i64> = vec![qids[2]];
    let exam_id = published_exam(&db, course_id, bank_id, &ms_only);
    block_on(async {
        let attempt = exams::start_attempt(&db, STUDENT, exam_id, "", NOW)
            .await
            .unwrap();
        let ms = attempt
            .questions
            .iter()
            .find(|x| x.qtype == "multi_select")
            .unwrap();
        // 1 of 2 correct, none wrong: half marks.
        exams::save_answer(
            &db,
            STUDENT,
            attempt.id,
            ms.question_id,
            json!([0]),
            NOW + 1_000,
        )
        .await
        .unwrap();
        let submitted = exams::submit_attempt(&db, STUDENT, attempt.id, NOW + 2_000)
            .await
            .unwrap();
        assert_eq!(submitted.score, 2.0);
        let result = exams::result_for_attempt(&db, attempt.id, STUDENT, false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.percent, 50.0);
        assert!(result.passed);
    });
}

#[test]
fn question_validation_rules() {
    let db = setup();
    let course_id = published_course(&db);
    block_on(async {
        let bank = questions::create_bank(&db, TEACHER, course_id, "B", "", false)
            .await
            .unwrap();
        // mcq needs at least 2 options.
        let bad = questions::create_question(
            &db,
            TEACHER,
            bank.id,
            &questions::CreateQuestionInput {
                qtype: "mcq".to_string(),
                question_text: "One option?".to_string(),
                options: vec!["only".to_string()],
                answer: json!(0),
                model_answer: String::new(),
                marks: 1,
                difficulty: "easy".to_string(),
                topic: String::new(),
            },
            false,
        )
        .await;
        assert!(bad.is_err());
        // mcq answer index out of range.
        let bad2 = questions::create_question(
            &db,
            TEACHER,
            bank.id,
            &questions::CreateQuestionInput {
                qtype: "mcq".to_string(),
                question_text: "Bad index?".to_string(),
                options: vec!["a".to_string(), "b".to_string()],
                answer: json!(5),
                model_answer: String::new(),
                marks: 1,
                difficulty: "easy".to_string(),
                topic: String::new(),
            },
            false,
        )
        .await;
        assert!(bad2.is_err());
        // multi-select needs a non-empty key.
        let bad3 = questions::create_question(
            &db,
            TEACHER,
            bank.id,
            &questions::CreateQuestionInput {
                qtype: "multi_select".to_string(),
                question_text: "No key?".to_string(),
                options: vec!["a".to_string(), "b".to_string()],
                answer: json!([]),
                model_answer: String::new(),
                marks: 1,
                difficulty: "easy".to_string(),
                topic: String::new(),
            },
            false,
        )
        .await;
        assert!(bad3.is_err());
        // true/false forces its options; the caller's options are ignored.
        let tf = questions::create_question(
            &db,
            TEACHER,
            bank.id,
            &questions::CreateQuestionInput {
                qtype: "true_false".to_string(),
                question_text: "Sky is blue.".to_string(),
                options: vec!["Yes".to_string(), "No".to_string(), "Maybe".to_string()],
                answer: json!(true),
                model_answer: String::new(),
                marks: 1,
                difficulty: "easy".to_string(),
                topic: String::new(),
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(tf.options, vec!["True".to_string(), "False".to_string()]);
        // Unknown type is refused.
        let bad4 = questions::create_question(
            &db,
            TEACHER,
            bank.id,
            &questions::CreateQuestionInput {
                qtype: "essay".to_string(),
                question_text: "Essay?".to_string(),
                options: vec![],
                answer: json!(null),
                model_answer: String::new(),
                marks: 1,
                difficulty: "easy".to_string(),
                topic: String::new(),
            },
            false,
        )
        .await;
        assert!(bad4.is_err());
    });
}

#[test]
fn draft_course_is_invisible_to_students() {
    let db = setup();
    block_on(async {
        let course = courses::create_course(&db, TEACHER, "Secret", "Draft.")
            .await
            .unwrap();
        assert!(courses::get_course(&db, course.id, Some(STUDENT), false)
            .await
            .unwrap()
            .is_none());
        assert!(courses::get_course(&db, course.id, Some(TEACHER), false)
            .await
            .unwrap()
            .is_some());
        let (list, _) = courses::list_courses(&db, Some(STUDENT), false, "", 1, 10)
            .await
            .unwrap();
        assert!(list.is_empty());
    });
}

#[test]
fn question_edit_refused_while_exam_in_progress() {
    let db = setup();
    let course_id = published_course(&db);
    block_on(async {
        courses::enrol(&db, STUDENT, course_id).await.unwrap();
    });
    let (bank_id, qids) = bank_with_questions(&db, course_id);
    let exam_id = published_exam(&db, course_id, bank_id, &qids);
    block_on(async {
        let _attempt = exams::start_attempt(&db, STUDENT, exam_id, "", NOW)
            .await
            .unwrap();
        let err = questions::update_question(
            &db,
            TEACHER,
            qids[0],
            &questions::UpdateQuestionInput {
                question_text: Some("Changed?".to_string()),
                options: None,
                answer: None,
                model_answer: None,
                marks: None,
                difficulty: None,
                topic: None,
            },
            false,
        )
        .await;
        assert!(err.is_err());
        assert_eq!(err.unwrap_err().code, "exam_in_progress");
    });
}

#[test]
fn course_delete_refused_when_in_use() {
    let db = setup();
    let course_id = published_course(&db);
    block_on(async {
        courses::enrol(&db, STUDENT, course_id).await.unwrap();
        let err = courses::delete_course(&db, TEACHER, course_id, false).await;
        assert!(err.is_err());
        assert_eq!(err.unwrap_err().code, "course_in_use");
    });
    // An unused course deletes cleanly.
    let db2 = setup();
    let empty = published_course(&db2);
    block_on(async {
        courses::delete_course(&db2, TEACHER, empty, false)
            .await
            .unwrap();
        assert!(courses::get_course(&db2, empty, Some(TEACHER), false)
            .await
            .unwrap()
            .is_none());
    });
}
