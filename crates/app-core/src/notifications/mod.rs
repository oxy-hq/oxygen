//! The notification service below the HTTP layer: the input a custom app's
//! `ctx.notify` sends, who it resolves to, the catalogue of kinds, the content
//! and the write path (inbox row, send, signals, queued delivery task).
//!
//! A service module of the prelude, not a crate: one surface calls it and
//! nothing needs a crate boundary to break a cycle (the decision is recorded in
//! `internal-docs/custom-apps-architecture.md`). The axum handlers, the delivery
//! transport (push, email, Slack: secrets and HTTP clients from `AppState`) and
//! the user-facing settings are the notifications surface's
//! (`oxy-api-notifications`), which re-exports these at their old paths.
//!
//! Also here, because their callers sit in contexts that may not reach that
//! surface: the platform notices' write half ([`platform`]), the seam that
//! hands the write path its inline transport ([`inline`]), and the port the
//! staff console reads an app's kinds through ([`catalog::live_kinds`]).

pub mod catalog;
pub mod compose;
pub mod default_email;
pub mod inline;
pub mod input;
pub mod links;
pub mod platform;
pub mod recipients;
pub mod send;
pub mod task;
