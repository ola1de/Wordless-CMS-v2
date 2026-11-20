//! School route table — the school portal + CBT (v2-only routes, no v1
//! counterpart): courses, lessons, enrolment, question banks,
//! exams, attempts, results, grading queue, certificates, and the
//! public school pages (`/school`, `/school/courses/:slug`,
//! `/school/certificates/:code`).
//!
//! Moved here verbatim from the main Worker's `routes_p9.rs` in the
//! 2026-11-20 size remedy (only the `Ctx` / helper imports changed).
//! All routes use the v1 `{ok, data, error}` envelope and the same
//! auth/capability guards as the rest of the API.
//!
//! Guards follow the doc §3 P9 decisions: creating/managing school
//! content needs `edit_posts` (author+ — the teacher role); the
//! instructor check for "my own course" lives in the service
//! (`require_manage_course`), because the router cannot know who
//! teaches a course. Enrolling, sitting an exam and reading your own
//! results need only a logged-in user (`read`). Answer keys are
//! never sent to students — the start payload carries questions
//! without answers (enforced in `wordless-school`, tested there).
//!
//! The cron sweep (`sweep_expired_attempts`) and the admin re-issue
//! lookup (`certificate_of_attempt`) live at the bottom of this file:
//! the sweep is called by this Worker's `#[event(scheduled)]`
//! handler (its own 5-minute trigger — the main Worker no longer
//! links `wordless-school`).

use serde_json::{json, Value};
use wordless_cms::settings::get_settings;
use wordless_kernel::capabilities::user_can;
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::html::escape_html;
use wordless_kernel::router::{read_json_body, RequestContext, RouteOptions, Router};
use wordless_kernel::time::now_ms;
use wordless_render::pages::{layout, NavItem};
use wordless_school::courses::{
    complete_lesson, create_course, create_lesson, delete_course, delete_lesson, drop_enrolment,
    enrol, get_course, get_course_by_slug, get_enrolment, list_courses, list_enrolments,
    list_lessons, update_course, update_lesson, UpdateCourseInput, UpdateLessonInput,
};
use wordless_school::exams::{
    certificate_for_attempt, close_expired_attempts, create_exam, delete_exam, get_attempt,
    get_certificate_by_code, get_exam, get_result, grade_answer, grading_queue, leaderboard,
    list_attempts, list_certificates, list_exams, list_results, result_for_attempt, save_answer,
    start_attempt, submit_attempt, update_exam, CreateExamInput, UpdateExamInput,
};
use wordless_school::questions::{
    create_bank, create_question, delete_bank, delete_question, get_bank, list_banks,
    list_questions, update_question, CreateQuestionInput, UpdateQuestionInput,
};
use wordless_seo_core::head::{render_breadcrumbs_html, Breadcrumb};
use wordless_seo_core::jsonld::course_json_ld;

use crate::ctx::Ctx;
use crate::helpers::{actor, ok, ok_json, page_param, parse_id, per_page_param};

/// True when the acting user is a site administrator
/// (`manage_options`).
fn is_admin(ctx: &Ctx) -> bool {
    ctx.user()
        .map(|u| user_can(u, "manage_options"))
        .unwrap_or(false)
}

/// The acting user id, or `None` for anonymous.
fn viewer(ctx: &Ctx) -> Option<i64> {
    ctx.user().map(|u| u.id)
}

/// Optional array of integers from a JSON body field.
fn opt_i64_array(body: &Value, field: &str) -> Option<Vec<i64>> {
    body.get(field)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
}

/// `None` when the field is absent (no change), `Some(None)` when it
/// is JSON null (clear), `Some(Some(s))` when a string was supplied.
/// Used for the exam window fields on update.
fn body_opt_opt_str(body: &Value, field: &str) -> Option<Option<String>> {
    match body.get(field) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(_) => None,
    }
}

/// Register every school route on the router built by the school Worker entry.
pub fn register_school_routes(router: &mut Router<Ctx>) {
    register_course_routes(router);
    register_lesson_routes(router);
    register_bank_routes(router);
    register_exam_routes(router);
    register_attempt_routes(router);
    register_result_routes(router);
    register_public_school_routes(router);
}

fn register_course_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------------ courses
    router.get(
        "/api/school/courses",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let q = ctx.query("q").unwrap_or("").to_string();
                let (items, total) = list_courses(
                    ctx.db.as_ref(),
                    viewer(ctx),
                    is_admin(ctx),
                    &q,
                    page,
                    per_page,
                )
                .await?;
                ok(
                    json!({ "items": items, "total": total, "page": page, "perPage": per_page }),
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/school/courses",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let title = body
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let description = body
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let course =
                    create_course(ctx.db.as_ref(), actor(ctx), &title, &description).await?;
                ok(course, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/school/courses/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let course = get_course(ctx.db.as_ref(), id, viewer(ctx), is_admin(ctx))
                    .await?
                    .ok_or_else(|| not_found("Course not found."))?;
                ok(course, 200)
            })
        },
        RouteOptions::default(),
    );

    router.put(
        "/api/school/courses/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let body = read_json_body(ctx)?;
                let input = UpdateCourseInput {
                    title: body
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    description: body
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    status: body
                        .get("status")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };
                let course =
                    update_course(ctx.db.as_ref(), actor(ctx), id, &input, is_admin(ctx)).await?;
                ok(course, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.delete(
        "/api/school/courses/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                delete_course(ctx.db.as_ref(), actor(ctx), id, is_admin(ctx)).await?;
                ok_json(json!({ "deleted": true, "id": id }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.post(
        "/api/school/courses/:id/enrol",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let enrolment = enrol(ctx.db.as_ref(), actor(ctx), id).await?;
                ok(enrolment, 201)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.delete(
        "/api/school/courses/:id/enrol",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                drop_enrolment(ctx.db.as_ref(), actor(ctx), id).await?;
                ok_json(json!({ "dropped": true, "courseId": id }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/school/courses/:id/enrolments",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let items = list_enrolments(ctx.db.as_ref(), actor(ctx), id, is_admin(ctx)).await?;
                ok(json!({ "items": items, "total": items.len() }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/school/courses/:id/progress",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let enrolment = get_enrolment(ctx.db.as_ref(), actor(ctx), id)
                    .await?
                    .ok_or_else(|| not_found("You are not enrolled in this course."))?;
                ok(enrolment, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );
}

fn register_lesson_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------------ lessons
    router.get(
        "/api/school/courses/:id/lessons",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let items = list_lessons(ctx.db.as_ref(), id, viewer(ctx), is_admin(ctx)).await?;
                ok(json!({ "items": items, "total": items.len() }), 200)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/school/courses/:id/lessons",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let body = read_json_body(ctx)?;
                let title = body
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let content = body
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let position = body.get("position").and_then(|v| v.as_i64());
                let duration = body
                    .get("durationMinutes")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let lesson = create_lesson(
                    ctx.db.as_ref(),
                    actor(ctx),
                    id,
                    &title,
                    &content,
                    position,
                    duration,
                    is_admin(ctx),
                )
                .await?;
                ok(lesson, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.put(
        "/api/school/lessons/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "lesson")?;
                let body = read_json_body(ctx)?;
                let input = UpdateLessonInput {
                    title: body
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    content: body
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    position: body.get("position").and_then(|v| v.as_i64()),
                    duration_minutes: body.get("durationMinutes").and_then(|v| v.as_i64()),
                };
                let lesson =
                    update_lesson(ctx.db.as_ref(), actor(ctx), id, &input, is_admin(ctx)).await?;
                ok(lesson, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.delete(
        "/api/school/lessons/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "lesson")?;
                delete_lesson(ctx.db.as_ref(), actor(ctx), id, is_admin(ctx)).await?;
                ok_json(json!({ "deleted": true, "id": id }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.post(
        "/api/school/lessons/:id/complete",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "lesson")?;
                let enrolment = complete_lesson(ctx.db.as_ref(), actor(ctx), id).await?;
                ok(enrolment, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );
}

fn register_bank_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------- banks + questions
    router.get(
        "/api/school/courses/:id/banks",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let items = list_banks(ctx.db.as_ref(), id, actor(ctx), is_admin(ctx)).await?;
                ok(json!({ "items": items, "total": items.len() }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.post(
        "/api/school/courses/:id/banks",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let body = read_json_body(ctx)?;
                let title = body
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let description = body
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let bank = create_bank(
                    ctx.db.as_ref(),
                    actor(ctx),
                    id,
                    &title,
                    &description,
                    is_admin(ctx),
                )
                .await?;
                ok(bank, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.delete(
        "/api/school/banks/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "bank")?;
                delete_bank(ctx.db.as_ref(), actor(ctx), id, is_admin(ctx)).await?;
                ok_json(json!({ "deleted": true, "id": id }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/school/banks/:id/questions",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "bank")?;
                let bank = get_bank(ctx.db.as_ref(), id)
                    .await?
                    .ok_or_else(|| not_found("Question bank not found."))?;
                wordless_school::courses::require_manage_course(
                    ctx.db.as_ref(),
                    actor(ctx),
                    bank.course_id,
                    is_admin(ctx),
                )
                .await?;
                let items = list_questions(ctx.db.as_ref(), id).await?;
                ok(json!({ "items": items, "total": items.len() }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.post(
        "/api/school/banks/:id/questions",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "bank")?;
                let body = read_json_body(ctx)?;
                let input = CreateQuestionInput {
                    qtype: body
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    question_text: body
                        .get("questionText")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    options: body
                        .get("options")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    answer: body.get("answer").cloned().unwrap_or(Value::Null),
                    model_answer: body
                        .get("modelAnswer")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    marks: body.get("marks").and_then(|v| v.as_i64()).unwrap_or(1),
                    difficulty: body
                        .get("difficulty")
                        .and_then(|v| v.as_str())
                        .unwrap_or("medium")
                        .to_string(),
                    topic: body
                        .get("topic")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                };
                let question =
                    create_question(ctx.db.as_ref(), actor(ctx), id, &input, is_admin(ctx)).await?;
                ok(question, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.put(
        "/api/school/questions/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "question")?;
                let body = read_json_body(ctx)?;
                let input = UpdateQuestionInput {
                    question_text: body
                        .get("questionText")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    options: body.get("options").and_then(|v| v.as_array()).map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    }),
                    answer: body.get("answer").cloned(),
                    model_answer: body
                        .get("modelAnswer")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    marks: body.get("marks").and_then(|v| v.as_i64()),
                    difficulty: body
                        .get("difficulty")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    topic: body
                        .get("topic")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };
                let question =
                    update_question(ctx.db.as_ref(), actor(ctx), id, &input, is_admin(ctx)).await?;
                ok(question, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.delete(
        "/api/school/questions/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "question")?;
                delete_question(ctx.db.as_ref(), actor(ctx), id, is_admin(ctx)).await?;
                ok_json(json!({ "deleted": true, "id": id }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );
}

fn register_exam_routes(router: &mut Router<Ctx>) {
    // -------------------------------------------------------------- exams
    router.get(
        "/api/school/courses/:id/exams",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "course")?;
                let items = list_exams(ctx.db.as_ref(), id, viewer(ctx), is_admin(ctx)).await?;
                ok(json!({ "items": items, "total": items.len() }), 200)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/school/courses/:id/exams",
        |ctx| {
            Box::pin(async move {
                let course_id = parse_id(ctx, "id", "course")?;
                let body = read_json_body(ctx)?;
                let bank_id = body
                    .get("bankId")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| bad_request("Field \"bankId\" is required."))?;
                let input = CreateExamInput {
                    title: body
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    description: body
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    duration_minutes: body
                        .get("durationMinutes")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(30),
                    window_start: body
                        .get("windowStart")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    window_end: body
                        .get("windowEnd")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    max_attempts: body
                        .get("maxAttempts")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(1),
                    pass_mark_percent: body
                        .get("passMarkPercent")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(50),
                    shuffle_questions: body
                        .get("shuffleQuestions")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
                    question_ids: opt_i64_array(&body, "questionIds").unwrap_or_default(),
                };
                let exam = create_exam(
                    ctx.db.as_ref(),
                    actor(ctx),
                    course_id,
                    bank_id,
                    &input,
                    is_admin(ctx),
                )
                .await?;
                ok(exam, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/school/exams/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "exam")?;
                let exam = get_exam(ctx.db.as_ref(), id, viewer(ctx), is_admin(ctx))
                    .await?
                    .ok_or_else(|| not_found("Exam not found."))?;
                ok(exam, 200)
            })
        },
        RouteOptions::default(),
    );

    router.put(
        "/api/school/exams/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "exam")?;
                let body = read_json_body(ctx)?;
                let input = UpdateExamInput {
                    title: body
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    description: body
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    duration_minutes: body.get("durationMinutes").and_then(|v| v.as_i64()),
                    window_start: body_opt_opt_str(&body, "windowStart"),
                    window_end: body_opt_opt_str(&body, "windowEnd"),
                    max_attempts: body.get("maxAttempts").and_then(|v| v.as_i64()),
                    pass_mark_percent: body.get("passMarkPercent").and_then(|v| v.as_i64()),
                    shuffle_questions: body.get("shuffleQuestions").and_then(|v| v.as_bool()),
                    question_ids: opt_i64_array(&body, "questionIds"),
                    status: body
                        .get("status")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };
                let exam =
                    update_exam(ctx.db.as_ref(), actor(ctx), id, &input, is_admin(ctx)).await?;
                ok(exam, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.delete(
        "/api/school/exams/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "exam")?;
                delete_exam(ctx.db.as_ref(), actor(ctx), id, is_admin(ctx)).await?;
                ok_json(json!({ "deleted": true, "id": id }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.post(
        "/api/school/exams/:id/start",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "exam")?;
                let ip = ctx.header("cf-connecting-ip").unwrap_or("").to_string();
                let attempt = start_attempt(ctx.db.as_ref(), actor(ctx), id, &ip, now_ms()).await?;
                ok(attempt, 201)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/school/exams/:id/attempts",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "exam")?;
                let items = list_attempts(ctx.db.as_ref(), id, actor(ctx), is_admin(ctx)).await?;
                ok(json!({ "items": items, "total": items.len() }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/school/exams/:id/leaderboard",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "exam")?;
                let rows = leaderboard(ctx.db.as_ref(), id).await?;
                ok(json!({ "items": rows, "total": rows.len() }), 200)
            })
        },
        RouteOptions::default(),
    );
}

fn register_attempt_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------------ attempts
    router.get(
        "/api/school/attempts/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "attempt")?;
                let attempt = get_attempt(ctx.db.as_ref(), id, actor(ctx), is_admin(ctx), now_ms())
                    .await?
                    .ok_or_else(|| not_found("Attempt not found."))?;
                ok(attempt, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/school/attempts/:id/answers",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "attempt")?;
                let body = read_json_body(ctx)?;
                let question_id = body
                    .get("questionId")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| bad_request("Field \"questionId\" is required."))?;
                let answer = body.get("answer").cloned().unwrap_or(Value::Null);
                let attempt = save_answer(
                    ctx.db.as_ref(),
                    actor(ctx),
                    id,
                    question_id,
                    answer,
                    now_ms(),
                )
                .await?;
                ok(attempt, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/school/attempts/:id/submit",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "attempt")?;
                let attempt = submit_attempt(ctx.db.as_ref(), actor(ctx), id, now_ms()).await?;
                ok(attempt, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/school/attempts/:id/result",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "attempt")?;
                let result = result_for_attempt(ctx.db.as_ref(), id, actor(ctx), is_admin(ctx))
                    .await?
                    .ok_or_else(|| {
                        WordlessError::new(
                            "result_not_ready",
                            404,
                            "This attempt is still being graded.",
                        )
                    })?;
                ok(result, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );
}

fn register_result_routes(router: &mut Router<Ctx>) {
    // --------------------------------------- results, grading, certificates
    router.get(
        "/api/school/results",
        |ctx| {
            Box::pin(async move {
                let exam_id = ctx.query("examId").and_then(|v| v.parse::<i64>().ok());
                let items =
                    list_results(ctx.db.as_ref(), actor(ctx), is_admin(ctx), exam_id).await?;
                ok(json!({ "items": items, "total": items.len() }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/school/results/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "result")?;
                let result = get_result(ctx.db.as_ref(), id, actor(ctx), is_admin(ctx))
                    .await?
                    .ok_or_else(|| not_found("Result not found."))?;
                ok(result, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/school/grading-queue",
        |ctx| {
            Box::pin(async move {
                let course_id = ctx.query("courseId").and_then(|v| v.parse::<i64>().ok());
                let items =
                    grading_queue(ctx.db.as_ref(), actor(ctx), course_id, is_admin(ctx)).await?;
                ok(json!({ "items": items, "total": items.len() }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.post(
        "/api/school/answers/:id/grade",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "answer")?;
                let body = read_json_body(ctx)?;
                let marks = body
                    .get("marksAwarded")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| bad_request("Field \"marksAwarded\" is required."))?;
                let feedback = body
                    .get("feedback")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let answer = grade_answer(
                    ctx.db.as_ref(),
                    actor(ctx),
                    id,
                    marks,
                    &feedback,
                    is_admin(ctx),
                    now_ms(),
                )
                .await?;
                ok(answer, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/school/certificates",
        |ctx| {
            Box::pin(async move {
                let course_id = ctx.query("courseId").and_then(|v| v.parse::<i64>().ok());
                let items =
                    list_certificates(ctx.db.as_ref(), actor(ctx), is_admin(ctx), course_id)
                        .await?;
                ok(json!({ "items": items, "total": items.len() }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/school/certificates/:code",
        |ctx| {
            Box::pin(async move {
                let code = ctx.param("code").unwrap_or("").to_string();
                let cert = get_certificate_by_code(ctx.db.as_ref(), &code)
                    .await?
                    .ok_or_else(|| not_found("Certificate not found."))?;
                ok(cert, 200)
            })
        },
        RouteOptions::default(),
    );
}

fn register_public_school_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------- public pages
    // The catalogue and the course page use the theme layout
    // (`wordless-render::layout`); the certificate page adds a print
    // stylesheet so it prints cleanly to paper or to PDF from the
    // browser (Q13: HTML certificate, not PDF).
    router.get(
        "/school",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let (items, total) =
                    list_courses(ctx.db.as_ref(), viewer(ctx), is_admin(ctx), "", page, per_page)
                        .await?;
                let settings = get_settings(ctx.db.as_ref(), ctx.site_id).await?;
                let mut body = String::from("<h1>School</h1>\n<p>All courses.</p>\n");
                // P10: visible breadcrumbs (Home → School).
                let crumbs_html = render_breadcrumbs_html(
                    &wordless_seo_core::site::site_url(ctx.db.as_ref(), ctx.site_id).await?,
                    &[Breadcrumb {
                        name: "School".to_string(),
                        path: "/school".to_string(),
                    }],
                );
                body = format!("{crumbs_html}{body}");
                if items.is_empty() {
                    body.push_str("<p>No courses have been published yet.</p>\n");
                } else {
                    body.push_str("<ul class=\"post-list\">\n");
                    for c in &items {
                        body.push_str(&format!(
                            "<li><h2><a href=\"/school/courses/{}\">{}</a></h2><p>{}</p><p class=\"meta\">{} lesson(s) &middot; {} student(s)</p></li>\n",
                            escape_html(&c.slug),
                            escape_html(&c.title),
                            escape_html(&c.description),
                            c.lesson_count,
                            c.enrolment_count,
                        ));
                    }
                    body.push_str("</ul>\n");
                    body.push_str(&format!("<p class=\"meta\">{} course(s)</p>\n", total));
                }
                Ok(RouteResponse::html(
                    layout(
                        "School",
                        &settings.site_title,
                        "All courses.",
                        &[NavItem { title: "Home".to_string(), url: "/".to_string() }],
                        &body,
                        None,
                        None,
                    ),
                    200,
                ))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/school/courses/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let course = get_course_by_slug(ctx.db.as_ref(), &slug, viewer(ctx), is_admin(ctx))
                    .await?
                    .ok_or_else(|| not_found("Course not found."))?;
                let lessons =
                    list_lessons(ctx.db.as_ref(), course.id, viewer(ctx), is_admin(ctx)).await?;
                let exams = list_exams(ctx.db.as_ref(), course.id, viewer(ctx), is_admin(ctx)).await?;
                let settings = get_settings(ctx.db.as_ref(), ctx.site_id).await?;
                let mut body = format!(
                    "<h1>{}</h1>\n<p>{}</p>\n<p class=\"meta\">Taught by {} &middot; {} lesson(s) &middot; {} student(s)</p>\n",
                    escape_html(&course.title),
                    escape_html(&course.description),
                    escape_html(if course.instructor_name.is_empty() {
                        &course.instructor_login
                    } else {
                        &course.instructor_name
                    }),
                    course.lesson_count,
                    course.enrolment_count,
                );
                body.push_str("<h2>Lessons</h2>\n");
                if lessons.is_empty() {
                    body.push_str("<p>No lessons yet.</p>\n");
                } else {
                    body.push_str("<ol>\n");
                    for l in &lessons {
                        body.push_str(&format!(
                            "<li>{}{}</li>\n",
                            escape_html(&l.title),
                            if l.completed { " &mdash; completed" } else { "" },
                        ));
                    }
                    body.push_str("</ol>\n");
                }
                body.push_str("<h2>Exams</h2>\n");
                if exams.is_empty() {
                    body.push_str("<p>No exams yet.</p>\n");
                } else {
                    body.push_str("<ul>\n");
                    for e in &exams {
                        body.push_str(&format!(
                            "<li>{} &mdash; {} minute(s), pass mark {}%, {} question(s)</li>\n",
                            escape_html(&e.title),
                            e.duration_minutes,
                            e.pass_mark_percent,
                            e.question_count,
                        ));
                    }
                    body.push_str("</ul>\n");
                }
                body.push_str("<p><a href=\"/admin#/school\">Open the School admin</a> to enrol and sit an exam.</p>\n");
                // P10: Course JSON-LD + visible breadcrumbs (Home →
                // School → course). The course URL is absolute; the
                // provider is the site itself.
                let base = wordless_seo_core::site::site_url(ctx.db.as_ref(), ctx.site_id).await?;
                let instructor = if course.instructor_name.is_empty() {
                    course.instructor_login.clone()
                } else {
                    course.instructor_name.clone()
                };
                let course_ld = course_json_ld(
                    &course.title,
                    &course.description,
                    &format!("{base}/school/courses/{}", course.slug),
                    &settings.site_title,
                    &format!("{base}/"),
                    Some(&instructor),
                );
                let head_extra = format!(
                    "<script type=\"application/ld+json\">{}</script>",
                    format!(
                        "{{\"@context\":\"https://schema.org\",\"@graph\":[{course_ld}]}}"
                    )
                    // Same escaping as the main worker's JSON-LD: a
                    // course title must never break out of <script>.
                    .replace('<', "\\u003c")
                    .replace('>', "\\u003e")
                    .replace('&', "\\u0026")
                );
                let crumbs_html = render_breadcrumbs_html(
                    &base,
                    &[
                        Breadcrumb {
                            name: "School".to_string(),
                            path: "/school".to_string(),
                        },
                        Breadcrumb {
                            name: course.title.clone(),
                            path: format!("/school/courses/{}", course.slug),
                        },
                    ],
                );
                body = format!("{crumbs_html}{body}");
                Ok(RouteResponse::html(
                    layout(
                        &course.title,
                        &settings.site_title,
                        &course.description,
                        &[
                            NavItem { title: "Home".to_string(), url: "/".to_string() },
                            NavItem { title: "School".to_string(), url: "/school".to_string() },
                        ],
                        &body,
                        Some(&head_extra),
                        None,
                    ),
                    200,
                ))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/school/certificates/:code",
        |ctx| {
            Box::pin(async move {
                let code = ctx.param("code").unwrap_or("").to_string();
                let cert = get_certificate_by_code(ctx.db.as_ref(), &code)
                    .await?
                    .ok_or_else(|| not_found("Certificate not found."))?;
                let settings = get_settings(ctx.db.as_ref(), ctx.site_id).await?;
                let body = format!(
                    "<article class=\"certificate\">\n<h1>Certificate of Completion</h1>\n<p>This is to certify that</p>\n<h2>{}</h2>\n<p>has successfully completed</p>\n<h3>{}</h3>\n<p>{} &mdash; {} out of {} marks ({}%)</p>\n<p class=\"meta\">Issued {} &middot; Verification code: {}</p>\n<p><a href=\"/school/certificates/{}\">Verify this certificate</a></p>\n</article>\n",
                    escape_html(if cert.student_name.is_empty() {
                        &cert.student_login
                    } else {
                        &cert.student_name
                    }),
                    escape_html(&cert.course_title),
                    escape_html(&cert.exam_title),
                    cert.score,
                    cert.max_score,
                    cert.percent,
                    escape_html(&cert.issued_at),
                    escape_html(&cert.code),
                    escape_html(&cert.code),
                );
                let print_css = "<style>@media print { header.site, footer.site { display: none; } main { max-width: 100%; } .certificate { border: 3px double #0d9488; padding: 48px; text-align: center; } }</style>";
                Ok(RouteResponse::html(
                    layout(
                        &format!("Certificate {}", cert.code),
                        &settings.site_title,
                        &format!("Certificate of completion for {}", cert.course_title),
                        &[
                            NavItem { title: "Home".to_string(), url: "/".to_string() },
                            NavItem { title: "School".to_string(), url: "/school".to_string() },
                        ],
                        &body,
                        Some(print_css),
                        None,
                    ),
                    200,
                ))
            })
        },
        RouteOptions::default(),
    );
}

/// The cron sweep closes expired attempts (the other half is the
/// lazy close on attempt read). Called by the worker's scheduled
/// handler; returns how many attempts were closed.
pub async fn sweep_expired_attempts(
    db: &dyn wordless_db::Db,
    now: u64,
) -> Result<u64, WordlessError> {
    close_expired_attempts(db, now).await
}

/// Re-issue lookup used by the admin UI: the certificate of one
/// attempt, if it exists.
pub async fn certificate_of_attempt(
    db: &dyn wordless_db::Db,
    attempt_id: i64,
) -> Result<Option<wordless_school::types::Certificate>, WordlessError> {
    certificate_for_attempt(db, attempt_id).await
}
