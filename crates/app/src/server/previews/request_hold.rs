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
//! A custom app's **staging** agent ask runs under the same hold
//! ([`HoldScope::staging`], `projects::agent_ask_staging`): one connector, one
//! decision. Its scope also carries the app and a [`HeldSink`], so a statement
//! the connector holds is recorded where the console lists it, and the run is
//! stamped with the app.
//!
//! Separate from the staging pin (`custom_apps_staging_pin`), which a
//! custom-app draft build also runs under: that pin chooses what is READ, and
//! says nothing about whether a write may go out.

use std::future::Future;
use std::sync::Arc;

use agentic_automation::HttpReview;
use agentic_connector::{DatabaseConnector, SqlDialect};
use async_trait::async_trait;
use futures::future::Either;
use uuid::Uuid;

use super::hold::{HoldingConnector, Session};
use super::sql_kind::StatementKind;

/// A statement a held connector refused, as a [`HeldSink`] is told it: never
/// the SQL text, only what it would have done and where.
pub struct HeldStatement<'a> {
    /// The `config.yml` database the statement was sent to.
    pub database: &'a str,
    pub dialect: SqlDialect,
    /// The first statement in it that is not a read.
    pub kind: &'a StatementKind,
}

/// Told about every statement a held connector refuses, before the refusal is
/// returned. A workspace preview has none; a staging ask records an
/// `app.staging.held` row.
#[async_trait]
pub trait HeldSink: Send + Sync {
    async fn held(&self, statement: HeldStatement<'_>);
}

/// What a held request is, beyond "it writes nothing": a workspace preview
/// (the default), or a custom app's staging environment, which names the app
/// and where its held statements go.
#[derive(Clone, Default)]
pub struct HoldScope {
    staging: Option<StagingHold>,
}

#[derive(Clone)]
struct StagingHold {
    app_id: Uuid,
    sink: Arc<dyn HeldSink>,
}

impl std::fmt::Debug for HoldScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HoldScope")
            .field("staging_app", &self.app_id())
            .finish()
    }
}

impl HoldScope {
    /// A custom app's staging environment: every write held, each held
    /// statement told to `sink`.
    pub fn staging(app_id: Uuid, sink: Arc<dyn HeldSink>) -> Self {
        Self {
            staging: Some(StagingHold { app_id, sink }),
        }
    }

    /// The custom app whose staging environment this is.
    pub fn app_id(&self) -> Option<Uuid> {
        self.staging.as_ref().map(|s| s.app_id)
    }

    pub fn sink(&self) -> Option<Arc<dyn HeldSink>> {
        self.staging.as_ref().map(|s| s.sink.clone())
    }

    /// Where the write was held, as a refusal names it.
    pub fn place(&self) -> &'static str {
        match self.staging {
            Some(_) => "this app's staging environment",
            None => "a workspace preview",
        }
    }

    /// The refusal for a write this hold cannot hold and so does not make (a
    /// secret, a pipeline destination).
    pub fn refusal(&self, what: &str) -> String {
        match self.staging {
            Some(_) => format!(
                "{what} is not written in this app's staging environment; promote the build to \
                 run it"
            ),
            None => refusal(what),
        }
    }

    /// An `http_request` step under this hold sends only `GET` and `HEAD`.
    pub fn http_review(&self, method: &str) -> HttpReview {
        if matches!(method, "GET" | "HEAD") {
            return HttpReview::Proceed;
        }
        HttpReview::Hold {
            reason: format!(
                "a {method} request is not sent in {}; only GET and HEAD are",
                self.place()
            ),
        }
    }
}

tokio::task_local! {
    /// Set only by [`scope`] / [`scope_as`]: this task serves a held request.
    static PREVIEW_REQUEST: HoldScope;
}

// These three are plain fns that hand back the wrapped future, not `async fn`s.
// An `async fn` wrapper keeps `fut` as its argument AND inside the future it
// awaits, and a debug build gives each copy its own slot in every poll frame
// it passes through, so wrapping a large future (a pipeline start) multiplied
// it on the worker's stack (`projects::agent_ask` overflowed 2 MiB that way).

/// Run `fut` as a workspace-preview request: every write it would make is
/// held. The workspace middleware wraps a preview-pinned request in this.
pub fn scope<F: Future>(fut: F) -> impl Future<Output = F::Output> {
    scope_as(HoldScope::default(), fut)
}

/// Run `fut` under `hold`: every write it would make is held.
pub fn scope_as<F: Future>(hold: HoldScope, fut: F) -> impl Future<Output = F::Output> {
    PREVIEW_REQUEST.scope(hold, fut)
}

/// [`scope_as`] when `hold` is `Some`, `fut` as it is otherwise.
pub fn scope_if<F: Future>(hold: Option<HoldScope>, fut: F) -> impl Future<Output = F::Output> {
    match hold {
        Some(hold) => Either::Left(scope_as(hold, fut)),
        None => Either::Right(fut),
    }
}

/// Whether the current task serves a held request. `false` in a task spawned
/// from one — code that outlives the request carries the answer with it
/// (`OxyProjectContext` captures [`current`] at construction).
pub fn active() -> bool {
    PREVIEW_REQUEST.try_with(|_| ()).is_ok()
}

/// The hold the current task serves under, if any.
pub fn current() -> Option<HoldScope> {
    PREVIEW_REQUEST.try_with(HoldScope::clone).ok()
}

/// `conn` for `database`, held when `held` (under the current task's hold, so
/// its sink hears what is refused). A preview request's connector forwards
/// reads and refuses everything else without sending it. Off the request's
/// task (a blocking thread, a spawned task) there is no current hold: the
/// connector still refuses, but with no sink, so a statement it holds there is
/// not listed — capture the hold and use [`hold_in`] to keep the sink.
/// `session` is what the database's configured type calls for
/// ([`Session::of`]).
pub fn hold_if(
    held: bool,
    conn: Arc<dyn DatabaseConnector>,
    database: &str,
    session: Session,
) -> Arc<dyn DatabaseConnector> {
    if !held {
        return conn;
    }
    hold_in(
        Some(&current().unwrap_or_default()),
        conn,
        database,
        session,
    )
}

/// `conn` for `database`, held under `hold` when there is one.
pub fn hold_in(
    hold: Option<&HoldScope>,
    conn: Arc<dyn DatabaseConnector>,
    database: &str,
    session: Session,
) -> Arc<dyn DatabaseConnector> {
    match hold {
        Some(hold) => Arc::new(HoldingConnector::under(conn, database, hold).with_session(session)),
        None => conn,
    }
}

/// An `http_request` step in a preview request sends only `GET` and `HEAD`.
pub fn http_review(method: &str) -> HttpReview {
    HoldScope::default().http_review(method)
}

/// The refusal for a write a preview request cannot hold and so does not make
/// (a secret, a pipeline destination).
pub fn refusal(what: &str) -> String {
    format!("{what} is not written in a workspace preview; merge the branch to run it")
}

#[cfg(test)]
#[path = "request_hold_tests.rs"]
mod tests;
