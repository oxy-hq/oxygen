//! Platform notices: what Oxygen itself tells a person, as inbox items with no
//! app and an `oxy.*` kind.
//!
//! - [`kinds`] — the closed list: [`PlatformNotice`], one variant per source,
//!   each carrying the ids of the record it is about.
//! - [`send`] — the write: the send, a signal per new recipient (at most once
//!   per recipient, kind and subject) and the `notification_deliver` task, in
//!   one transaction.
//! - [`render`] + [`subjects`] — the words, built from Oxy's records when the
//!   inbox is read or a channel delivers. Nothing is stored that says what
//!   happened, so a platform notice needs no org OLTP store, unlike an app's.
//! - [`people`] — who, by standing in the org or a workspace.
//! - [`preferences`] — mutes per (kind, channel); billing and a test notice
//!   cannot be muted.
//! - the sources: [`run_failed`] (installed into `agentic-runtime`; told once
//!   per failing streak, then daily, by [`run_streak`]),
//!   [`workspace_ready`] (called by onboarding's clone), billing's
//!   `billing/notify.rs`, the anomaly delivery task's people
//!   (`anomaly_notify/people.rs`), [`cli_approval`] (a CLI naming who must
//!   approve its device-code request), [`self_test`] (a person testing their
//!   own push, `POST /api/notifications/test`), [`ask_finished`] (an Ask
//!   answer someone asked to hear about; installed into `agentic-runtime` as a
//!   run-done observer), and [`ask_waiting`] (that run stopping to ask them
//!   something; installed as the run-suspended observer).

pub mod ask_finished;
pub mod ask_waiting;
pub mod cli_approval;
pub mod kinds;
pub mod people;
pub mod preferences;
pub mod render;
pub mod run_failed;
pub mod run_streak;
pub mod self_test;
pub mod send;
pub mod subjects;
pub mod workspace_ready;

pub use kinds::PlatformNotice;
pub use send::{PlatformSent, notify};
