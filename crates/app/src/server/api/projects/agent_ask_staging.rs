//! An agent ask from a custom app's **staging** host runs with every write
//! held (`internal-docs/customer-apps-staging.md` D5).
//!
//! The environment guard lets the ask routes through on the staging host
//! (`custom_app_env_request::is_staging_write`) and nothing else. Here
//! [`ask_scope`] decides what that ask runs under:
//!
//! - **production** — `None`: the ask runs as it always has;
//! - **staging** — the app the request names (the staging host; off an app
//!   host, `x-oxy-app` or the `Referer`) must be published from this
//!   workspace, and the
//!   caller must be one oxy-authz lets open its staging
//!   (`may_open_non_production`). Then a [`HoldScope::staging`]: `start_ask`
//!   builds the run's platform and starts the run inside it, so the platform
//!   captures the hold (`OxyProjectContext::holds_writes`) — every connector
//!   held, no automation runner, no builder bridges, Airway refused,
//!   `http_request` `GET`/`HEAD` only — and the run is stamped
//!   `workspace_preview: {revision_id, app_id}`, so recovery and a cold resume
//!   retire it rather than drive it on production's platform;
//! - **anything else** (a dev slot, an app that does not resolve, a caller who
//!   may not open staging) — refused, `404 EnvironmentRefused`, before any
//!   thread or run row is written. Fail closed: a lookup error is a refusal.
//!
//! Every statement the hold refuses is recorded through the one
//! `app.staging.held` writer (`custom_apps_staging_held::record_held`) as
//! `function = "agent"`, so the console's held list shows it to the developer
//! who asked. Cancelling on the staging host makes the same decision and
//! reaches only a run this app's staging ask started ([`staging_run_matches`]).

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::HeaderMap;
use axum::response::Response;
use oxy_app_core::custom_app_env_request::request_environment;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::server::api::custom_apps_functions::write_record::{WriteRecord, plane_for_dialect};
use crate::server::api::custom_apps_staging_held::{HeldActor, HeldRow, record_held};
use crate::server::api::custom_apps_threads::{PRODUCTION_THREAD_SOURCE, staging_thread_source};
use agentic_pipeline::platform::preview_stamp::RUN_STAMP;

use crate::server::api::custom_apps_staging_pin::request::{
    AppRef, environment_refused, staging_app_for, staging_app_ref,
};
use crate::server::previews::request_hold::{HeldSink, HeldStatement, HoldScope};
use crate::server::previews::sql_kind::StatementKind;

/// The surface an ask's held rows are listed under, and their `mode`.
pub const ASK_SURFACE: &str = "agent";

/// What the ask from this request runs under: `None` in production, a staging
/// hold on the staging host, or the refusal to answer. See the module docs.
pub async fn ask_scope(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user: &AuthenticatedUser,
    project_id: Uuid,
) -> Result<Option<HoldScope>, Response> {
    let environment = request_environment(headers).map_err(|e| e.into_response())?;
    match environment {
        AppEnvironment::Production => Ok(None),
        AppEnvironment::Staging => staging_hold(db, headers, user, project_id)
            .await
            .map(Some)
            .ok_or_else(|| refused(&AppEnvironment::Staging, "an agent ask")),
        other => Err(refused(&other, "an agent ask")),
    }
}

/// The `threads.source` an ask under `hold` writes, and the only one it may
/// continue (`custom_apps_threads::staging_thread_source`): production's
/// history, or this app's staging history.
pub fn thread_source(hold: Option<&HoldScope>) -> String {
    match hold {
        None => PRODUCTION_THREAD_SOURCE.to_string(),
        Some(h) => staging_thread_source(h.app_id()),
    }
}

/// Whether `metadata` (an `agentic_runs` row's) is a run the staging ask of
/// `hold`'s app started: its `workspace_preview` stamp names that app. A
/// production run (unstamped) or another app's never matches.
pub fn staging_run_matches(hold: &HoldScope, metadata: Option<&serde_json::Value>) -> bool {
    let stamped = metadata
        .and_then(|m| m.get(RUN_STAMP))
        .and_then(|s| s.get("app_id"))
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok());
    stamped.is_some() && stamped == hold.app_id()
}

/// The app a staging write names (`custom_apps_staging_pin::request::
/// staging_app_ref`). Shared with `projects::automation_run::staging_hold`.
pub(crate) fn ask_app_ref(headers: &HeaderMap) -> Option<AppRef> {
    staging_app_ref(headers)
}

/// The staging hold for the app this request names, when the caller may open
/// its staging. `None` on any miss, a lookup error included.
async fn staging_hold(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user: &AuthenticatedUser,
    project_id: Uuid,
) -> Option<HoldScope> {
    let caller = crate::server::authz::Caller::from_user(user);
    let app = staging_app_for(db, headers, &caller, project_id).await?;
    let row = HeldRow {
        app_id: app.id,
        app_slug: app.slug.clone(),
        org_id: app.org_id,
        project_id,
        environment: AppEnvironment::Staging.name(),
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
    Some(HoldScope::staging(app.id, sink))
}

/// `404 EnvironmentRefused` (`custom_apps_staging_pin::request::
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
    }
}
