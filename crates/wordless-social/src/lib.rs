//! WORDLESS v2 Social module (P7) — BuddyPress-like networking.
//!
//! Member profiles, **friends** (mutual, request/accept — Q11 decided
//! 2026-11-20; follows are NOT built in P7 and there is no `wl_follows`
//! table), activity feed, groups, direct messaging, notifications, and
//! the member directory.
//!
//! Privacy model (see `visibility` — pure functions, a test per rule):
//! - Profiles: `public` (anyone), `friends` (friends + self),
//!   `private` (self only; admins may view for moderation).
//! - Activity: `public`, `friends`, `group` (members only), `private`
//!   (actor only). Hidden/deleted items never appear in any feed.
//! - Groups: `public` (listed, content visible to all), `private`
//!   (listed, content members-only), `hidden` (unlisted, members-only,
//!   join by invite only).
//! - Blocking: a `wl_friendships` row with status `blocked` in either
//!   direction hides both users from each other everywhere (profiles,
//!   feed, directory, messages, friend requests).
//! - Messaging: direct messages require friendship OR shared group
//!   membership, and never cross a block.
//!
//! Service functions take `db: &dyn Db` plus plain parameters and
//! return typed results; route handlers live in `wordless-worker`
//! (`routes_p7.rs`). Notifications are in-app rows; the worker sends
//! the email copy best-effort through the integrations mailer (mock
//! provider only logs — never presented as delivered).

pub mod activity;
pub mod friends;
pub mod groups;
pub mod helpers;
pub mod messages;
pub mod notifications;
pub mod profiles;
pub mod types;
pub mod visibility;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod service_tests;
