//! An agent ask from a custom app's **staging** host or a **sandbox** runs with
//! every write held (`internal-docs/customer-apps-staging.md` D5).
//!
//! The environment guard lets the ask routes through in an environment whose
//! `workspace-actions` plane is `held`
//! (`custom_app_env_request::is_held_workspace_action`) and nothing else. Here
//! [`ask_scope`] decides what that ask runs under:
//!
//! - **production** — `None`: the ask runs as it always has;
//! - **staging or a sandbox** — the app the request names (the environment's
//!   host; off an app host, `x-oxy-app` or the `Referer`) must be published
//!   from this workspace, and the caller must be one oxy-authz lets open that
//!   environment (`non_production_app`). Then the plane decides
//!   (`workspace_actions_mode`, the environment's row, never its kind):
//!   `held` is a [`HoldScope`] for that environment — staging's, or the
//!   sandbox's own, whose threads, run stamps and held rows are never
//!   staging's. `start_ask` builds the run's platform and starts the run inside
//!   it, so the platform captures the hold (`OxyProjectContext::holds_writes`):
//!   every connector held, no automation runner, no builder bridges, Airway
//!   refused, `http_request` `GET`/`HEAD` only; the run is stamped
//!   `workspace_preview: {revision_id, app_id}` (plus `environment` for a
//!   sandbox), so recovery and a cold resume retire it rather than drive it on
//!   production's platform. `refused` is today's refusal;
//! - **anything else** (an app that does not resolve, a caller who may not
//!   open the environment, a `refused` plane) — refused, `404
//!   EnvironmentRefused`, before any thread or run row is written. An
//!   unreadable plane is `409` ([`planes_unreadable`]). Fail closed: a lookup
//!   error is a refusal.
//!
//! Every statement the hold refuses is recorded through the one
//! `app.staging.held` writer (`oxy_custom_apps::staging_held::record_held`) as
//! `function = "agent"`, under the environment's own name. Cancelling makes the
//! same decision and reaches only a run this app's ask in this environment
//! started ([`staging_run_matches`]).

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::HeaderMap;
use axum::response::Response;
use oxy_app_core::custom_app_env_request::request_environment;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use agentic_pipeline::platform::preview_stamp::RUN_STAMP;
use oxy_custom_apps::planes::WorkspaceActionsMode;
use oxy_custom_apps::staging_held::{HeldActor, HeldRow, record_held};
use oxy_custom_apps::thread_source::{
    PRODUCTION_THREAD_SOURCE, sandbox_thread_source, staging_thread_source,
};
use oxy_custom_apps::write_record::{WriteRecord, plane_for_dialect};

use super::workspace_actions::{non_production_app, workspace_actions_mode};
use crate::server::previews::sql_kind::StatementKind;
use oxy_app_core::custom_app_env_request::environment_refused;
use oxy_app_core::write_hold::{HeldSink, HeldStatement, HoldScope};

/// The surface an ask's held rows are listed under, and their `mode`.
pub const ASK_SURFACE: &str = "agent";

/// What the ask from this request runs under: `None` in production, a hold in
/// staging or a sandbox whose plane is `held`, or the refusal to answer. See
/// the module docs.
pub async fn ask_scope(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user: &AuthenticatedUser,
    project_id: Uuid,
) -> Result<Option<HoldScope>, Response> {
    let environment = request_environment(headers).map_err(|e| e.into_response())?;
    if environment == AppEnvironment::Production {
        return Ok(None);
    }
    let refusal = || refused(&environment, "an agent ask");
    let Some(app) = non_production_app(db, headers, user, project_id, &environment).await else {
        return Err(refusal());
    };
    match workspace_actions_mode(db, app.id, &environment).await? {
        // `preview` changes what an automation run does, not an ask: the ask
        // runs with its writes held either way.
        WorkspaceActionsMode::Held | WorkspaceActionsMode::Preview => {
            Ok(Some(hold_scope(db, user, project_id, &app, environment)))
        }
        WorkspaceActionsMode::Refused => Err(refusal()),
    }
}

/// The `threads.source` an ask under `hold` writes, and the only one it may
/// continue (`oxy_custom_apps::thread_source`): production's history, this
/// app's staging history, or this sandbox's own.
pub fn thread_source(hold: Option<&HoldScope>) -> String {
    match hold.map(|h| (h.app_id(), h.environment())) {
        None => PRODUCTION_THREAD_SOURCE.to_string(),
        Some((app, Some(env))) if *env != AppEnvironment::Staging => {
            sandbox_thread_source(app, &env.name())
        }
        Some((app, _)) => staging_thread_source(app),
    }
}

/// Whether `metadata` (an `agentic_runs` row's) is a run the ask of `hold`'s
/// app and environment started: its `workspace_preview` stamp names that app,
/// and the same environment (a stamp without one is staging's). A production
/// run (unstamped), another app's, or another environment's never matches.
pub fn staging_run_matches(hold: &HoldScope, metadata: Option<&serde_json::Value>) -> bool {
    let stamp = metadata.and_then(|m| m.get(RUN_STAMP));
    let stamped = stamp
        .and_then(|s| s.get("app_id"))
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok());
    let stamped_env = stamp
        .and_then(|s| s.get("environment"))
        .and_then(|v| v.as_str())
        .unwrap_or("staging");
    stamped.is_some()
        && stamped == hold.app_id()
        && hold.environment().map(AppEnvironment::name).as_deref() == Some(stamped_env)
}

/// The hold for `environment` of `app`, whose held statements are recorded
/// under the environment's own name.
fn hold_scope(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    project_id: Uuid,
    app: &entity::apps::Model,
    environment: AppEnvironment,
) -> HoldScope {
    let row = HeldRow {
        app_id: app.id,
        app_slug: app.slug.clone(),
        org_id: app.org_id,
        project_id,
        environment: environment.name(),
        actor: HeldActor::User {
            id: user.id,
            email: user.email.clone(),
        },
        function_or_surface: ASK_SURFACE.to_string(),
        mode: ASK_SURFACE.to_string(),
        request_id: None,
        writes: Vec::new(),
        invocation_id: None,
        trace_id: None,
        token_id: None,
    };
    let sink = Arc::new(AskHeldSink {
        db: db.clone(),
        row,
    });
    HoldScope::in_environment(app.id, environment, sink)
}

/// `404 EnvironmentRefused` (`oxy_custom_apps::staging_pin::request::
/// environment_refused`). Shared with `projects::automation_run::staging_hold`.
pub(crate) fn refused(environment: &AppEnvironment, what: &str) -> Response {
    environment_refused(environment, what)
}

/// Records each statement an ask's hold refuses as one `app.staging.held` row,
/// before the refusal reaches the agent.
struct AskHeldSink {
    db: DatabaseConnection,
    /// Everything but `writes`.
    row: HeldRow,
}

#[async_trait]
impl HeldSink for AskHeldSink {
    async fn held(&self, statement: HeldStatement<'_>) {
        let mut row = self.row.clone();
        row.writes = vec![write_record(&statement)];
        record_held(&self.db, row).await;
    }
}

/// The held statement as an audit write: plane, database, verb and first
/// table — never the SQL.
fn write_record(statement: &HeldStatement<'_>) -> WriteRecord {
    let (verb, table, note) = match statement.kind {
        StatementKind::Write { verb, targets } => (
            verb.clone(),
            targets.first().cloned().unwrap_or_default(),
            None,
        ),
        StatementKind::Unclassified(_) | StatementKind::Read => (
            "UNCLASSIFIED".to_string(),
            String::new(),
            Some("the statement could not be classified as a read"),
        ),
    };
    WriteRecord {
        plane: plane_for_dialect(statement.dialect),
        namespace: statement.database.to_string(),
        verb,
        table,
        rows: None,
        statements: 1,
        op: None,
        note,
        sent: None,
    }
}
