//! What a host may say about an automation `http_request` step before it
//! sends anything.
//!
//! The answer to `agentic_automation::WorkspaceContext::review_http`. It lives
//! here, not in `agentic-automation`, because the request-scoped write hold
//! (`oxy-app-core`'s `write_hold`) answers it too, and that crate sits below
//! the agentic domains. `agentic_automation::HttpReview` re-exports it.
//! `SqlReview`, its sibling, stays in automation: no host below it names one.

/// The answer to `review_http(method, url)`, asked after the URL is rendered
/// and before any request is built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpReview {
    /// Send the request. The default.
    Proceed,
    /// Do not send it; the step succeeds with the hold recorded.
    Hold { reason: String },
}
