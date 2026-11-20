//! WORDLESS v2 School portal + CBT (P9) — courses, enrolment, question
//! bank, timed exams, auto-grading, results, certificates.
//!
//! Design decisions (doc §3 P9 outcome block, recorded 2026-11-20
//! before any code):
//! - Question types, exactly four: `mcq`, `true_false`, `multi_select`,
//!   `short_answer`.
//! - Grading is pure logic (`grading.rs`, one test per rule): mcq /
//!   true-false = full marks or 0; multi-select = partial credit
//!   `(correct selected − incorrect selected) / total correct`, clamped
//!   at ≥ 0, × marks — no negative marking; short-answer is NEVER
//!   auto-graded (manual queue only).
//! - Timing is server-side only: `deadline_ms` is computed once at
//!   start (`started_at_ms + duration × 60,000`) and stored on the
//!   attempt; the browser clock is never trusted. A late submit is a
//!   409 `attempt_expired` and the attempt is auto-submitted with the
//!   answers saved so far. Expired attempts are closed by the cron
//!   sweep AND lazily on attempt read.
//! - Answer keys are NEVER sent to the student: the start response
//!   carries questions without answers.
//! - Certificates (Q13): server-rendered HTML page + print stylesheet,
//!   not PDF; unique code `WL-` + 16 hex chars from SHA-256; public
//!   verification at `/school/certificates/:code`.
//! - Honest limits: NO live proctoring, NO webcam monitoring, NO
//!   lockdown browser, NO IP-based identity proof. The controls that
//!   ARE provided are server-side timing, randomised question order,
//!   attempt limits, exam windows, enrolment-only access, and
//!   session-bound attempts.
//!
//! Service functions take `db: &dyn Db` plus plain parameters and
//! return typed results; route handlers live in `wordless-worker`
//! (`routes_p9.rs`).

pub mod courses;
pub mod exams;
pub mod grading;
pub mod questions;
pub mod types;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod service_tests;
