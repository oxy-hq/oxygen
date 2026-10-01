//! A workspace-preview **request** cannot write — enforced where work
//! executes, not by the route it arrived on.
//!
//! [`super::read_only`] refuses a preview request by route before a handler
//! runs. That table cannot see what an allowed route does next: rendering a
//! data app runs the branch's task SQL and HTTP calls, `/sql/*` runs whatever
//! it is sent, and a chat run can delegate to a branch automation. So the
//! workspace middleware runs every preview-pinned request inside [`scope`],
//! and everything that executes consults it:
//!
//! * **Connectors** — every warehouse and Airhouse connector built for the
//!   request is a [`HoldingConnector`] ([`hold_if`]): reads are sent, any
//!   statement `sql_kind` does not classify as a read is refused, and nothing
//!   is sent. Built in `agentic_wiring::project_ctx` — `build_connector_for_db`
//!   and the platform's `resolve_pre_built_connector`, the two doors every
//!   request-path connector comes through.
//! * **The platform** (`OxyProjectContext`) captures [`active`] when it is
//!   built, so the decision travels with it into the task a run is driven on
//!   (a task-local does not cross `tokio::spawn`). A held platform reports
//!   `is_workspace_preview`, which stops agentic-pipeline building an
//!   automation runner or handing out builder bridges, and refuses an Airway
//!   step. It sends only `GET`/`HEAD` from an `http_request` step
//!   ([`http_review`]), persists no secret, resolves no pipeline destination,
//!   and turns off the anomaly, monitor-scan and compile ports.
//!
//! Separate from the staging pin (`custom_apps_staging_pin`), which a
//! custom-app draft build also runs under: that pin chooses what is READ, and
//! says nothing about whether a write may go out.

use std::future::Future;
use std::sync::Arc;

use agentic_automation::HttpReview;
use agentic_connector::DatabaseConnector;

use super::hold::HoldingConnector;

tokio::task_local! {
    /// Set only by [`scope`]: this task serves a workspace-preview request.
    static PREVIEW_REQUEST: ();
}

/// Run `fut` as a workspace-preview request: every write it would make is
/// held. The workspace middleware wraps a preview-pinned request in this.
pub async fn scope<F: Future>(fut: F) -> F::Output {
    PREVIEW_REQUEST.scope((), fut).await
}

/// Whether the current task serves a workspace-preview request. `false` in a
/// task spawned from one — code that outlives the request carries the answer
/// with it (`OxyProjectContext` captures it at construction).
pub fn active() -> bool {
    PREVIEW_REQUEST.try_with(|_| ()).is_ok()
}

/// `conn` for `database`, held when `held`. A preview request's connector
/// forwards reads and refuses everything else without sending it.
pub fn hold_if(
    held: bool,
    conn: Arc<dyn DatabaseConnector>,
    database: &str,
) -> Arc<dyn DatabaseConnector> {
    if held {
        Arc::new(HoldingConnector::new(conn, database))
    } else {
        conn
    }
}

/// An `http_request` step in a preview request sends only `GET` and `HEAD`.
pub fn http_review(method: &str) -> HttpReview {
    if matches!(method, "GET" | "HEAD") {
        return HttpReview::Proceed;
    }
    HttpReview::Hold {
        reason: format!(
            "a {method} request is not sent in a workspace preview; only GET and HEAD are"
        ),
    }
}

/// The refusal for a write a preview request cannot hold and so does not make
/// (a secret, a pipeline destination).
pub fn refusal(what: &str) -> String {
    format!("{what} is not written in a workspace preview; merge the branch to run it")
}

#[cfg(test)]
#[path = "request_hold_tests.rs"]
mod tests;
