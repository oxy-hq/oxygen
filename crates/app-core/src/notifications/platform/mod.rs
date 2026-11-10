//! Platform notices, the write half: what Oxygen itself tells a person, as
//! inbox items with no app and an `oxy.*` kind.
//!
//! What every source needs to *raise* one lives here, beside the app send
//! ([`super::send`]), because the sources are spread over contexts that cannot
//! reach the notifications surface: billing and the anomaly delivery in
//! `oxy-app`, onboarding's clone in `oxy-api-tenancy`, the device-code flow.
//!
//! - [`kinds`] — the closed list: [`PlatformNotice`], one variant per source,
//!   each carrying the ids of the record it is about.
//! - [`send`] — the write: the send, a signal per new recipient (at most once
//!   per recipient, kind and subject) and the `notification_deliver` task, in
//!   one transaction.
//! - [`people`] — who, by standing in the org or a workspace.
//! - [`workspace_ready`], [`cli_approval`] — two sources whose callers sit
//!   outside the notifications surface.
//! - [`self_test`] — the test notice's cooldown, which the staff console's app
//!   test shares.
//!
//! The rest — rendering the words, the sources driven by the agentic runtime's
//! observers, per-kind mutes — is the notifications surface's
//! (`oxy-api-notifications`, `platform`).

pub mod cli_approval;
pub mod kinds;
pub mod people;
pub mod self_test;
pub mod send;
pub mod workspace_ready;

pub use kinds::PlatformNotice;
pub use send::{PlatformSent, notify};
