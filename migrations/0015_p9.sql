-- WORDLESS v2 P9 — School portal + CBT.
--
-- The ten tables in doc §0.1 E plus wl_lesson_progress (one row per
-- completed lesson — progress tracking needs a place to store it; the
-- §0.1 E list has none). Times are ISO-8601 UTC text, as everywhere
-- in v2; attempt timing is epoch milliseconds (started_at_ms /
-- deadline_ms) because the deadline is computed once at start and
-- compared numerically at submit — the browser clock is never trusted.
--
-- Money rule does not apply to scores: marks are integers, but
-- multi-select partial credit can be fractional, so score /
-- marks_awarded are REAL, rounded to 2 dp on write.

-- Courses. status: draft | published. The instructor is the creator
-- (wordless-school::courses::create sets instructor_id); an
-- administrator may manage any course.
CREATE TABLE wl_courses (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL UNIQUE,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published')),
  instructor_id INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_courses_status ON wl_courses (status, created_at);

-- Lessons belong to a course; position orders them inside it.
CREATE TABLE wl_lessons (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  course_id INTEGER NOT NULL REFERENCES wl_courses (id),
  title TEXT NOT NULL,
  content TEXT NOT NULL DEFAULT '',
  position INTEGER NOT NULL DEFAULT 0,
  duration_minutes INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_lessons_course ON wl_lessons (course_id, position);

-- Enrolments. status: active | completed | dropped. completed_at is
-- stamped when the last lesson completes (automatic, not manual).
CREATE TABLE wl_enrolments (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  course_id INTEGER NOT NULL REFERENCES wl_courses (id),
  user_id INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'completed', 'dropped')),
  enrolled_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE (course_id, user_id)
);
CREATE INDEX idx_wl_enrolments_user ON wl_enrolments (user_id, status);
CREATE INDEX idx_wl_enrolments_course ON wl_enrolments (course_id, status);

-- One row per completed lesson (P9 addition to the §0.1 E list).
CREATE TABLE wl_lesson_progress (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  enrolment_id INTEGER NOT NULL REFERENCES wl_enrolments (id),
  lesson_id INTEGER NOT NULL REFERENCES wl_lessons (id),
  completed_at TEXT NOT NULL,
  UNIQUE (enrolment_id, lesson_id)
);
CREATE INDEX idx_wl_lesson_progress_enrolment ON wl_lesson_progress (enrolment_id);

-- Question banks are course-scoped: a bank belongs to exactly one
-- course, and an exam draws from one bank of its own course.
CREATE TABLE wl_question_banks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  course_id INTEGER NOT NULL REFERENCES wl_courses (id),
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_question_banks_course ON wl_question_banks (course_id);

-- Questions. type: mcq | true_false | multi_select | short_answer.
-- options_json: JSON array of option strings (mcq / multi-select);
--   true/false stores ["True", "False"].
-- answer_json: mcq -> integer index; true_false -> boolean;
--   multi_select -> JSON array of integer indices; short_answer -> NULL.
-- model_answer: reference answer shown to the human grader for
--   short-answer questions (never auto-graded, never sent to students).
-- marks: integer marks for the question. difficulty: easy | medium |
--   hard (free text, validated in code). topic: free text.
CREATE TABLE wl_questions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  bank_id INTEGER NOT NULL REFERENCES wl_question_banks (id),
  type TEXT NOT NULL CHECK (type IN ('mcq', 'true_false', 'multi_select', 'short_answer')),
  question_text TEXT NOT NULL,
  options_json TEXT NOT NULL DEFAULT '[]',
  answer_json TEXT,
  model_answer TEXT NOT NULL DEFAULT '',
  marks INTEGER NOT NULL DEFAULT 1 CHECK (marks > 0),
  difficulty TEXT NOT NULL DEFAULT 'medium',
  topic TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_questions_bank ON wl_questions (bank_id, id);

-- Exams. duration_minutes: the timed length; window_start /
-- window_end: ISO-8601 UTC, NULL = open-ended. max_attempts: 0 =
-- unlimited. pass_mark_percent: 0-100. shuffle_questions: 1 =
-- shuffle the question order per attempt. question_ids_json: JSON
-- array of question ids (explicit selection); [] = all questions in
-- the bank. status: draft | published.
CREATE TABLE wl_exams (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  course_id INTEGER NOT NULL REFERENCES wl_courses (id),
  bank_id INTEGER NOT NULL REFERENCES wl_question_banks (id),
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  duration_minutes INTEGER NOT NULL DEFAULT 30 CHECK (duration_minutes > 0),
  window_start TEXT,
  window_end TEXT,
  max_attempts INTEGER NOT NULL DEFAULT 1 CHECK (max_attempts >= 0),
  pass_mark_percent INTEGER NOT NULL DEFAULT 50 CHECK (pass_mark_percent >= 0 AND pass_mark_percent <= 100),
  shuffle_questions INTEGER NOT NULL DEFAULT 1,
  question_ids_json TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_exams_course ON wl_exams (course_id, status);

-- Attempts. status: in_progress | submitted | awaiting_grading |
-- graded | expired. started_at_ms / deadline_ms are epoch ms;
-- deadline_ms is computed once at start and never recomputed.
-- question_order_json: the question ids in the order this attempt
-- sees them (shuffled when the exam asks for it; stable on re-fetch).
-- score: REAL, rounded to 2 dp; max_score: integer sum of marks.
-- started_ip: the cf-connecting-ip header when present (recorded,
-- never used as an identity proof — see the P9 outcome block).
CREATE TABLE wl_exam_attempts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  exam_id INTEGER NOT NULL REFERENCES wl_exams (id),
  user_id INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'in_progress'
    CHECK (status IN ('in_progress', 'submitted', 'awaiting_grading', 'graded', 'expired')),
  started_at_ms INTEGER NOT NULL,
  deadline_ms INTEGER NOT NULL,
  submitted_at_ms INTEGER,
  question_order_json TEXT NOT NULL DEFAULT '[]',
  score REAL NOT NULL DEFAULT 0,
  max_score INTEGER NOT NULL DEFAULT 0,
  started_ip TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_exam_attempts_exam ON wl_exam_attempts (exam_id, user_id, status);
CREATE INDEX idx_wl_exam_attempts_user ON wl_exam_attempts (user_id, status);
CREATE INDEX idx_wl_exam_attempts_expiry ON wl_exam_attempts (status, deadline_ms);

-- Answers. answer_json: mcq -> integer index; true_false -> boolean;
-- multi_select -> JSON array of indices; short_answer -> string.
-- is_correct: 1/0 once graded, NULL while ungraded. marks_awarded:
-- REAL, 2 dp. grader_feedback: free text from the human grader.
CREATE TABLE wl_exam_answers (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  attempt_id INTEGER NOT NULL REFERENCES wl_exam_attempts (id),
  question_id INTEGER NOT NULL REFERENCES wl_questions (id),
  answer_json TEXT NOT NULL DEFAULT 'null',
  is_correct INTEGER,
  marks_awarded REAL NOT NULL DEFAULT 0,
  grader_feedback TEXT NOT NULL DEFAULT '',
  graded_at TEXT,
  UNIQUE (attempt_id, question_id)
);
CREATE INDEX idx_wl_exam_answers_attempt ON wl_exam_answers (attempt_id, question_id);

-- Results: one graded-result snapshot per finished attempt (written
-- when the attempt leaves in_progress; updated when manual grading
-- completes). The leaderboard reads this table, not the attempts.
CREATE TABLE wl_results (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  attempt_id INTEGER NOT NULL UNIQUE REFERENCES wl_exam_attempts (id),
  user_id INTEGER NOT NULL,
  exam_id INTEGER NOT NULL REFERENCES wl_exams (id),
  course_id INTEGER NOT NULL REFERENCES wl_courses (id),
  score REAL NOT NULL DEFAULT 0,
  max_score INTEGER NOT NULL DEFAULT 0,
  percent REAL NOT NULL DEFAULT 0,
  passed INTEGER NOT NULL DEFAULT 0,
  graded_at TEXT NOT NULL
);
CREATE INDEX idx_wl_results_exam ON wl_results (exam_id, score DESC, graded_at ASC);
CREATE INDEX idx_wl_results_user ON wl_results (user_id, graded_at DESC);

-- Certificates: one per passing attempt (auto-issued on grading
-- completion at/above the pass mark). code is unique and is the
-- public verification key: WL-<16 hex from SHA-256>. issued_at is
-- ISO-8601 UTC; the code derivation also uses the attempt's epoch ms,
-- so re-issuing the same attempt yields the same code.
CREATE TABLE wl_certificates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  code TEXT NOT NULL UNIQUE,
  user_id INTEGER NOT NULL,
  course_id INTEGER NOT NULL REFERENCES wl_courses (id),
  exam_id INTEGER NOT NULL REFERENCES wl_exams (id),
  attempt_id INTEGER NOT NULL UNIQUE REFERENCES wl_exam_attempts (id),
  result_id INTEGER NOT NULL REFERENCES wl_results (id),
  issued_at TEXT NOT NULL
);
CREATE INDEX idx_wl_certificates_user ON wl_certificates (user_id, issued_at DESC);
