//! What a host may say about a step's side effect before it runs.
//!
//! The answers to [`crate::workspace::WorkspaceContext::review_sql`] and
//! [`crate::workspace::WorkspaceContext::review_http`]. Every host but one
//! answers `Proceed` — the trait defaults do, and production must not change —
//! and the one that doesn't is the workspace-preview platform, which runs a
//! branch's procedures against real data and must not write anything.

use serde_json::Value;

/// The answer to `review_sql(database, sql)`, asked after the SQL is rendered
/// and before any connector exists.
#[derive(Clone, Debug, PartialEq)]
pub enum SqlReview {
    /// Run the SQL as rendered. The default, and production's only answer.
    Proceed,
    /// Do not run it. The step still **succeeds**, so the procedure goes on,
    /// and its result records what would have been written — the hold is part
    /// of the step output the run viewer already shows.
    Hold {
        reason: String,
        verb: String,
        targets: Vec<String>,
    },
    /// Run `sql` instead, and attach `notes` to the step result under
    /// `"preview"`. For a host that redirects writes somewhere safe; no host
    /// does yet.
    Rewrite { sql: String, notes: Value },
}

/// The answer to `review_http(method, url)`, asked after the URL is rendered
/// and before any request is built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpReview {
    /// Send the request. The default.
    Proceed,
    /// Do not send it; the step succeeds with the hold recorded.
    Hold { reason: String },
}
