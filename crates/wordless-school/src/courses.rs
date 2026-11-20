//! Courses, lessons, enrolments, progress.
//!
//! Rules (doc §3 P9 outcome block):
//! - Creating a course needs `edit_posts` (enforced by the route
//!   guard); the creator becomes the instructor. Managing a course
//!   (update / delete / lessons / enrolment list) needs the caller to
//!   be the instructor or a site admin (`manage_options`).
//! - Draft courses are invisible to everyone except the instructor
//!   and site admins (404, not 403 — do not leak existence).
//! - Enrolment is open for published courses, any logged-in user.
//! - Lesson content is visible to enrolled students, the instructor
//!   and site admins; anyone else sees titles only (content "").
//! - Progress: one `wl_lesson_progress` row per completed lesson; a
//!   course completes automatically when every lesson is complete.
//! - A course with enrolments or exams cannot be deleted (409
//!   `course_in_use`); delete removes its lessons, banks and
//!   questions when it is still unused.

use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, conflict, forbidden, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::types::{Course, Enrolment, Lesson};

fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if (c == ' ' || c == '-' || c == '_') && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "course".to_string()
    } else {
        out
    }
}

/// (login, display_name) for a user id, or `None` when the user does
/// not exist.
pub async fn user_basic(
    db: &dyn Db,
    user_id: i64,
) -> Result<Option<(String, String)>, WordlessError> {
    let row = db
        .query_one(
            "SELECT user_login, display_name FROM wp_users WHERE ID = ? LIMIT 1",
            &[Param::Integer(user_id)],
        )
        .await?;
    Ok(row.map(|r| (row_text(&r, "user_login"), row_text(&r, "display_name"))))
}

/// True when `user_id` is the instructor of `course_id`.
pub async fn is_instructor(
    db: &dyn Db,
    user_id: i64,
    course_id: i64,
) -> Result<bool, WordlessError> {
    let row = db
        .query_one(
            "SELECT instructor_id FROM wl_courses WHERE id = ? LIMIT 1",
            &[Param::Integer(course_id)],
        )
        .await?;
    Ok(row
        .map(|r| row_int(&r, "instructor_id") == user_id)
        .unwrap_or(false))
}

/// Instructor of the course, or a site admin, may manage it.
pub async fn require_manage_course(
    db: &dyn Db,
    user_id: i64,
    course_id: i64,
    is_admin: bool,
) -> Result<(), WordlessError> {
    if is_admin {
        return Ok(());
    }
    if is_instructor(db, user_id, course_id).await? {
        Ok(())
    } else {
        Err(forbidden("Only the course instructor can do this."))
    }
}

/// Create a course; the creator becomes its instructor. Starts as a
/// draft — publish is an explicit update.
pub async fn create_course(
    db: &dyn Db,
    instructor_id: i64,
    title: &str,
    description: &str,
) -> Result<Course, WordlessError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(bad_request("Course title is required."));
    }
    let mut slug = slugify(title);
    let mut n = 2;
    while db
        .query_one(
            "SELECT 1 AS x FROM wl_courses WHERE slug = ? LIMIT 1",
            &[Param::Text(slug.clone())],
        )
        .await?
        .is_some()
    {
        slug = format!("{}-{n}", slugify(title));
        n += 1;
    }
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_courses (slug, title, description, status, instructor_id, created_at, updated_at) \
             VALUES (?, ?, ?, 'draft', ?, ?, ?)",
            &[
                Param::Text(slug),
                Param::Text(title.to_string()),
                Param::Text(description.trim().to_string()),
                Param::Integer(instructor_id),
                Param::Text(now.clone()),
                Param::Text(now),
            ],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    get_course(db, id, Some(instructor_id), false)
        .await?
        .ok_or_else(|| not_found("Course not found."))
}

/// Read one course. Drafts are invisible to non-instructors and
/// non-admins (404, not 403).
pub async fn get_course(
    db: &dyn Db,
    id: i64,
    viewer: Option<i64>,
    is_admin: bool,
) -> Result<Option<Course>, WordlessError> {
    let row = db
        .query_one(
            "SELECT c.*, COALESCE(u.user_login, '') AS instructor_login, \
             COALESCE(u.display_name, '') AS instructor_name, \
             (SELECT COUNT(*) FROM wl_lessons l WHERE l.course_id = c.id) AS lesson_count, \
             (SELECT COUNT(*) FROM wl_enrolments e WHERE e.course_id = c.id AND e.status != 'dropped') AS enrolment_count \
             FROM wl_courses c LEFT JOIN wp_users u ON u.ID = c.instructor_id \
             WHERE c.id = ? LIMIT 1",
            &[Param::Integer(id)],
        )
        .await?;
    let Some(r) = row else { return Ok(None) };
    let status = row_text(&r, "status");
    let instructor_id = row_int(&r, "instructor_id");
    if status == "draft" && !is_admin && viewer != Some(instructor_id) {
        return Ok(None);
    }
    Ok(Some(row_to_course(&r)))
}

pub async fn get_course_by_slug(
    db: &dyn Db,
    slug: &str,
    viewer: Option<i64>,
    is_admin: bool,
) -> Result<Option<Course>, WordlessError> {
    let row = db
        .query_one(
            "SELECT id FROM wl_courses WHERE slug = ? LIMIT 1",
            &[Param::Text(slug.to_string())],
        )
        .await?;
    match row {
        None => Ok(None),
        Some(r) => get_course(db, row_int(&r, "id"), viewer, is_admin).await,
    }
}

/// List courses. Anonymous / non-admin viewers see published only;
/// instructors also see their own drafts; admins see everything.
pub async fn list_courses(
    db: &dyn Db,
    viewer: Option<i64>,
    is_admin: bool,
    q: &str,
    page: i64,
    per_page: i64,
) -> Result<(Vec<Course>, i64), WordlessError> {
    let visibility = if is_admin {
        "1 = 1".to_string()
    } else {
        match viewer {
            Some(uid) => format!("(c.status = 'published' OR c.instructor_id = {uid})"),
            None => "c.status = 'published'".to_string(),
        }
    };
    let like = format!("%{}%", q.trim());
    let list_sql = format!(
        "SELECT c.*, COALESCE(u.user_login, '') AS instructor_login, \
         COALESCE(u.display_name, '') AS instructor_name, \
         (SELECT COUNT(*) FROM wl_lessons l WHERE l.course_id = c.id) AS lesson_count, \
         (SELECT COUNT(*) FROM wl_enrolments e WHERE e.course_id = c.id AND e.status != 'dropped') AS enrolment_count \
         FROM wl_courses c LEFT JOIN wp_users u ON u.ID = c.instructor_id \
         WHERE (? = '' OR c.title LIKE ? OR c.description LIKE ?) AND {visibility} \
         ORDER BY c.created_at DESC, c.id DESC LIMIT ? OFFSET ?"
    );
    let count_sql = format!(
        "SELECT COUNT(*) AS c FROM wl_courses c \
         WHERE (? = '' OR c.title LIKE ? OR c.description LIKE ?) AND {visibility}"
    );
    let rows = db
        .query_all(
            &list_sql,
            &[
                Param::Text(q.trim().to_string()),
                Param::Text(like.clone()),
                Param::Text(like),
                Param::Integer(per_page),
                Param::Integer((page - 1) * per_page),
            ],
        )
        .await?;
    let total_row = db
        .query_one(
            &count_sql,
            &[
                Param::Text(q.trim().to_string()),
                Param::Text(format!("%{}%", q.trim())),
                Param::Text(format!("%{}%", q.trim())),
            ],
        )
        .await?;
    let total = total_row.map(|r| row_int(&r, "c")).unwrap_or(0);
    Ok((rows.iter().map(row_to_course).collect(), total))
}

pub struct UpdateCourseInput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
}

/// Update title / description / status (instructor or admin).
pub async fn update_course(
    db: &dyn Db,
    updater: i64,
    id: i64,
    input: &UpdateCourseInput,
    is_admin: bool,
) -> Result<Course, WordlessError> {
    let course = get_course(db, id, Some(updater), is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    require_manage_course(db, updater, id, is_admin).await?;
    let mut title = course.title.clone();
    let mut description = course.description.clone();
    let mut status = course.status.clone();
    if let Some(t) = &input.title {
        let t = t.trim();
        if t.is_empty() {
            return Err(bad_request("Course title is required."));
        }
        title = t.to_string();
    }
    if let Some(d) = &input.description {
        description = d.trim().to_string();
    }
    if let Some(s) = &input.status {
        if !["draft", "published"].contains(&s.as_str()) {
            return Err(bad_request("Invalid course status."));
        }
        status = s.clone();
    }
    db.exec(
        "UPDATE wl_courses SET title = ?, description = ?, status = ?, updated_at = ? WHERE id = ?",
        &[
            Param::Text(title),
            Param::Text(description),
            Param::Text(status),
            Param::Text(now_iso()),
            Param::Integer(id),
        ],
    )
    .await?;
    get_course(db, id, Some(updater), is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))
}

/// Delete a course. Refused while it has enrolments or exams (409
/// `course_in_use`); otherwise its lessons, banks and questions go
/// with it (progress rows belong to the enrolments, which do not
/// exist in that case).
pub async fn delete_course(
    db: &dyn Db,
    deleter: i64,
    id: i64,
    is_admin: bool,
) -> Result<(), WordlessError> {
    let _course = get_course(db, id, Some(deleter), is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    require_manage_course(db, deleter, id, is_admin).await?;
    let enrolments = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_enrolments WHERE course_id = ?",
            &[Param::Integer(id)],
        )
        .await?;
    if enrolments.map(|r| row_int(&r, "c")).unwrap_or(0) > 0 {
        return Err(conflict(
            "This course has enrolments and cannot be deleted.",
            "course_in_use",
        ));
    }
    let exams = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_exams WHERE course_id = ?",
            &[Param::Integer(id)],
        )
        .await?;
    if exams.map(|r| row_int(&r, "c")).unwrap_or(0) > 0 {
        return Err(conflict(
            "This course has exams and cannot be deleted.",
            "course_in_use",
        ));
    }
    // Banks and their questions first, then lessons, then the course.
    let banks = db
        .query_all(
            "SELECT id FROM wl_question_banks WHERE course_id = ?",
            &[Param::Integer(id)],
        )
        .await?;
    for b in &banks {
        let bid = row_int(b, "id");
        db.exec(
            "DELETE FROM wl_questions WHERE bank_id = ?",
            &[Param::Integer(bid)],
        )
        .await?;
    }
    db.exec(
        "DELETE FROM wl_question_banks WHERE course_id = ?",
        &[Param::Integer(id)],
    )
    .await?;
    db.exec(
        "DELETE FROM wl_lessons WHERE course_id = ?",
        &[Param::Integer(id)],
    )
    .await?;
    db.exec("DELETE FROM wl_courses WHERE id = ?", &[Param::Integer(id)])
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- lessons

/// List a course's lessons in position order. Lesson content is
/// returned only when the viewer may study (enrolled + active,
/// instructor, or site admin); everyone else sees titles only.
pub async fn list_lessons(
    db: &dyn Db,
    course_id: i64,
    viewer: Option<i64>,
    is_admin: bool,
) -> Result<Vec<Lesson>, WordlessError> {
    let course = get_course(db, course_id, viewer, is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    let may_study = is_admin
        || viewer == Some(course.instructor_id)
        || match viewer {
            Some(uid) => matches!(
                get_enrolment(db, uid, course_id).await?,
                Some(ref e) if e.status == "active" || e.status == "completed"
            ),
            None => false,
        };
    let rows = db
        .query_all(
            "SELECT l.*, \
             (SELECT 1 FROM wl_lesson_progress p \
              JOIN wl_enrolments e ON e.id = p.enrolment_id \
              WHERE p.lesson_id = l.id AND e.user_id = ? AND e.course_id = ?) AS completed \
             FROM wl_lessons l WHERE l.course_id = ? ORDER BY l.position ASC, l.id ASC",
            &[
                Param::Integer(viewer.unwrap_or(0)),
                Param::Integer(course_id),
                Param::Integer(course_id),
            ],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let mut lesson = row_to_lesson(r);
            if !may_study {
                lesson.content = String::new();
            }
            lesson
        })
        .collect())
}

/// Create a lesson (instructor or admin). Position defaults to the
/// end of the course when omitted.
pub async fn create_lesson(
    db: &dyn Db,
    creator: i64,
    course_id: i64,
    title: &str,
    content: &str,
    position: Option<i64>,
    duration_minutes: i64,
    is_admin: bool,
) -> Result<Lesson, WordlessError> {
    let _course = get_course(db, course_id, Some(creator), is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    require_manage_course(db, creator, course_id, is_admin).await?;
    let title = title.trim();
    if title.is_empty() {
        return Err(bad_request("Lesson title is required."));
    }
    if duration_minutes < 0 {
        return Err(bad_request("Lesson duration cannot be negative."));
    }
    let position = match position {
        Some(p) => p,
        None => {
            let row = db
                .query_one(
                    "SELECT COALESCE(MAX(position), -1) + 1 AS next_pos FROM wl_lessons WHERE course_id = ?",
                    &[Param::Integer(course_id)],
                )
                .await?;
            row.map(|r| row_int(&r, "next_pos")).unwrap_or(0)
        }
    };
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_lessons (course_id, title, content, position, duration_minutes, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            &[
                Param::Integer(course_id),
                Param::Text(title.to_string()),
                Param::Text(content.to_string()),
                Param::Integer(position),
                Param::Integer(duration_minutes),
                Param::Text(now.clone()),
                Param::Text(now),
            ],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    let row = db
        .query_one(
            "SELECT l.*, 0 AS completed FROM wl_lessons l WHERE l.id = ? LIMIT 1",
            &[Param::Integer(id)],
        )
        .await?;
    row.map(|r| row_to_lesson(&r))
        .ok_or_else(|| not_found("Lesson not found."))
}

pub struct UpdateLessonInput {
    pub title: Option<String>,
    pub content: Option<String>,
    pub position: Option<i64>,
    pub duration_minutes: Option<i64>,
}

pub async fn update_lesson(
    db: &dyn Db,
    updater: i64,
    lesson_id: i64,
    input: &UpdateLessonInput,
    is_admin: bool,
) -> Result<Lesson, WordlessError> {
    let row = db
        .query_one(
            "SELECT l.*, 0 AS completed FROM wl_lessons l WHERE l.id = ? LIMIT 1",
            &[Param::Integer(lesson_id)],
        )
        .await?;
    let Some(r) = row else {
        return Err(not_found("Lesson not found."));
    };
    let lesson = row_to_lesson(&r);
    require_manage_course(db, updater, lesson.course_id, is_admin).await?;
    let mut title = lesson.title.clone();
    let mut content = lesson.content.clone();
    let mut position = lesson.position;
    let mut duration = lesson.duration_minutes;
    if let Some(t) = &input.title {
        let t = t.trim();
        if t.is_empty() {
            return Err(bad_request("Lesson title is required."));
        }
        title = t.to_string();
    }
    if let Some(c) = &input.content {
        content = c.clone();
    }
    if let Some(p) = input.position {
        position = p;
    }
    if let Some(d) = input.duration_minutes {
        if d < 0 {
            return Err(bad_request("Lesson duration cannot be negative."));
        }
        duration = d;
    }
    db.exec(
        "UPDATE wl_lessons SET title = ?, content = ?, position = ?, duration_minutes = ?, updated_at = ? WHERE id = ?",
        &[
            Param::Text(title),
            Param::Text(content),
            Param::Integer(position),
            Param::Integer(duration),
            Param::Text(now_iso()),
            Param::Integer(lesson_id),
        ],
    )
    .await?;
    let row = db
        .query_one(
            "SELECT l.*, 0 AS completed FROM wl_lessons l WHERE l.id = ? LIMIT 1",
            &[Param::Integer(lesson_id)],
        )
        .await?;
    row.map(|r| row_to_lesson(&r))
        .ok_or_else(|| not_found("Lesson not found."))
}

pub async fn delete_lesson(
    db: &dyn Db,
    deleter: i64,
    lesson_id: i64,
    is_admin: bool,
) -> Result<(), WordlessError> {
    let row = db
        .query_one(
            "SELECT course_id FROM wl_lessons WHERE id = ? LIMIT 1",
            &[Param::Integer(lesson_id)],
        )
        .await?;
    let Some(r) = row else {
        return Err(not_found("Lesson not found."));
    };
    let course_id = row_int(&r, "course_id");
    require_manage_course(db, deleter, course_id, is_admin).await?;
    db.exec(
        "DELETE FROM wl_lesson_progress WHERE lesson_id = ?",
        &[Param::Integer(lesson_id)],
    )
    .await?;
    db.exec(
        "DELETE FROM wl_lessons WHERE id = ?",
        &[Param::Integer(lesson_id)],
    )
    .await?;
    Ok(())
}

// ------------------------------------------------------------- enrolments

/// Enrol a user in a published course. A dropped enrolment can be
/// re-created (the old row is re-activated); an active/completed one
/// is a 409 `already_enrolled`.
pub async fn enrol(db: &dyn Db, user_id: i64, course_id: i64) -> Result<Enrolment, WordlessError> {
    let course = get_course(db, course_id, Some(user_id), false)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    if course.status != "published" {
        return Err(forbidden("This course is not open for enrolment."));
    }
    if let Some(existing) = get_enrolment(db, user_id, course_id).await? {
        if existing.status == "dropped" {
            db.exec(
                "UPDATE wl_enrolments SET status = 'active', enrolled_at = ?, completed_at = NULL WHERE id = ?",
                &[Param::Text(now_iso()), Param::Integer(existing.id)],
            )
            .await?;
            return get_enrolment(db, user_id, course_id)
                .await?
                .ok_or_else(|| not_found("Enrolment not found."));
        }
        return Err(conflict(
            "You are already enrolled in this course.",
            "already_enrolled",
        ));
    }
    let meta = db
        .exec(
            "INSERT INTO wl_enrolments (course_id, user_id, status, enrolled_at) VALUES (?, ?, 'active', ?)",
            &[Param::Integer(course_id), Param::Integer(user_id), Param::Text(now_iso())],
        )
        .await?;
    let _ = meta;
    get_enrolment(db, user_id, course_id)
        .await?
        .ok_or_else(|| not_found("Enrolment not found."))
}

/// Drop an enrolment (the student leaves the course; progress rows
/// stay, so re-enrolling restores them).
pub async fn drop_enrolment(
    db: &dyn Db,
    user_id: i64,
    course_id: i64,
) -> Result<(), WordlessError> {
    let existing = get_enrolment(db, user_id, course_id)
        .await?
        .ok_or_else(|| not_found("You are not enrolled in this course."))?;
    if existing.status == "dropped" {
        return Ok(());
    }
    db.exec(
        "UPDATE wl_enrolments SET status = 'dropped', completed_at = NULL WHERE id = ?",
        &[Param::Integer(existing.id)],
    )
    .await?;
    Ok(())
}

pub async fn get_enrolment(
    db: &dyn Db,
    user_id: i64,
    course_id: i64,
) -> Result<Option<Enrolment>, WordlessError> {
    let row = db
        .query_one(
            "SELECT e.*, \
             (SELECT COUNT(*) FROM wl_lesson_progress p WHERE p.enrolment_id = e.id) AS lessons_completed, \
             (SELECT COUNT(*) FROM wl_lessons l WHERE l.course_id = e.course_id) AS lessons_total \
             FROM wl_enrolments e WHERE e.course_id = ? AND e.user_id = ? LIMIT 1",
            &[Param::Integer(course_id), Param::Integer(user_id)],
        )
        .await?;
    Ok(row.map(|r| row_to_enrolment(&r, "", "")))
}

/// The instructor's / admin's enrolment list for a course, with
/// student login + display name.
pub async fn list_enrolments(
    db: &dyn Db,
    requester: i64,
    course_id: i64,
    is_admin: bool,
) -> Result<Vec<Enrolment>, WordlessError> {
    let _course = get_course(db, course_id, Some(requester), is_admin)
        .await?
        .ok_or_else(|| not_found("Course not found."))?;
    require_manage_course(db, requester, course_id, is_admin).await?;
    let rows = db
        .query_all(
            "SELECT e.*, COALESCE(u.user_login, '') AS login, COALESCE(u.display_name, '') AS display_name, \
             (SELECT COUNT(*) FROM wl_lesson_progress p WHERE p.enrolment_id = e.id) AS lessons_completed, \
             (SELECT COUNT(*) FROM wl_lessons l WHERE l.course_id = e.course_id) AS lessons_total \
             FROM wl_enrolments e LEFT JOIN wp_users u ON u.ID = e.user_id \
             WHERE e.course_id = ? ORDER BY e.enrolled_at ASC, e.id ASC",
            &[Param::Integer(course_id)],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| row_to_enrolment(r, &row_text(r, "login"), &row_text(r, "display_name")))
        .collect())
}

/// Mark a lesson complete for an enrolled student. Completing the
/// last lesson completes the course automatically (stamps
/// `completed_at`). Idempotent: completing twice is a no-op.
pub async fn complete_lesson(
    db: &dyn Db,
    user_id: i64,
    lesson_id: i64,
) -> Result<Enrolment, WordlessError> {
    let lesson_row = db
        .query_one(
            "SELECT course_id FROM wl_lessons WHERE id = ? LIMIT 1",
            &[Param::Integer(lesson_id)],
        )
        .await?;
    let Some(lr) = lesson_row else {
        return Err(not_found("Lesson not found."));
    };
    let course_id = row_int(&lr, "course_id");
    let enrolment = get_enrolment(db, user_id, course_id)
        .await?
        .ok_or_else(|| forbidden("You are not enrolled in this course."))?;
    if enrolment.status == "dropped" {
        return Err(forbidden("You are not enrolled in this course."));
    }
    if enrolment.status == "completed" {
        return Ok(enrolment);
    }
    db.exec(
        "INSERT OR IGNORE INTO wl_lesson_progress (enrolment_id, lesson_id, completed_at) VALUES (?, ?, ?)",
        &[
            Param::Integer(enrolment.id),
            Param::Integer(lesson_id),
            Param::Text(now_iso()),
        ],
    )
    .await?;
    let fresh = get_enrolment(db, user_id, course_id)
        .await?
        .ok_or_else(|| not_found("Enrolment not found."))?;
    if fresh.lessons_total > 0 && fresh.lessons_completed >= fresh.lessons_total {
        db.exec(
            "UPDATE wl_enrolments SET status = 'completed', completed_at = ? WHERE id = ?",
            &[Param::Text(now_iso()), Param::Integer(fresh.id)],
        )
        .await?;
        return get_enrolment(db, user_id, course_id)
            .await?
            .ok_or_else(|| not_found("Enrolment not found."));
    }
    Ok(fresh)
}

fn row_to_course(row: &wordless_db::Row) -> Course {
    let instructor_id = row_int(row, "instructor_id");
    Course {
        id: row_int(row, "id"),
        slug: row_text(row, "slug"),
        title: row_text(row, "title"),
        description: row_text(row, "description"),
        status: row_text(row, "status"),
        instructor_id,
        instructor_login: row_text(row, "instructor_login"),
        instructor_name: row_text(row, "instructor_name"),
        lesson_count: row_int(row, "lesson_count"),
        enrolment_count: row_int(row, "enrolment_count"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

fn row_to_lesson(row: &wordless_db::Row) -> Lesson {
    Lesson {
        id: row_int(row, "id"),
        course_id: row_int(row, "course_id"),
        title: row_text(row, "title"),
        content: row_text(row, "content"),
        position: row_int(row, "position"),
        duration_minutes: row_int(row, "duration_minutes"),
        completed: row_int(row, "completed") == 1,
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

fn row_to_enrolment(row: &wordless_db::Row, login: &str, display_name: &str) -> Enrolment {
    Enrolment {
        id: row_int(row, "id"),
        course_id: row_int(row, "course_id"),
        user_id: row_int(row, "user_id"),
        login: login.to_string(),
        display_name: display_name.to_string(),
        status: row_text(row, "status"),
        enrolled_at: row_text(row, "enrolled_at"),
        completed_at: row_opt_text(row, "completed_at"),
        lessons_completed: row_int(row, "lessons_completed"),
        lessons_total: row_int(row, "lessons_total"),
    }
}
