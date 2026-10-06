//! The audit row of "run now" — `app.function.run_queued` — for **every**
//! credential (`POST …/functions/{name}/runs`, `handlers::run_function_job`).
//!
//! Queueing a run executes an app's code against its data and its third
//! parties, so who asked, and with what, is on the record: one row per run
//! that was queued, in the environment it was queued in, carrying the run id
//! and the function. A refused request queued nothing and leaves none.
//!
//! As for a publish (`custom_apps_publish_audit`, which this mirrors):
//!
//! * a session, a legacy API key or an API token is named by
//!   `AuditEntry::for_request`, so the row is the one a sandbox agent token's
//!   run already wrote (`agent_scope::audit_run_queued`), unchanged and shared;
//! * an app publish token (`oxypublish_`), which may run an app's declared
//!   checks in production, carries a marker instead of a credential and is
//!   stamped by `by_publish_token`.

use entity::apps;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::agent_scope;
use crate::server::api::custom_apps_publish_audit::by_publish_token;

pub(crate) const RUN_QUEUED: &str = "app.function.run_queued";

/// Who queued the run: the request's actor, and the publish-token marker when
/// an `oxypublish_` token authenticated it.
#[derive(Clone, Copy)]
pub(super) struct Asked<'a> {
    pub actor: &'a RequestActor,
    pub marker: Option<&'a AppPublishTokenAuth>,
}

/// One audit row for a run queued in `environment` of `app`. Best effort: the
/// run is already queued.
pub(super) async fn run_queued(
    db: &DatabaseConnection,
    asked: Asked<'_>,
    app: &apps::Model,
    environment: &AppEnvironment,
    (function, run_id): (&str, &str),
) {
    let Some(marker) = asked.marker else {
        let run = (function, run_id);
        return agent_scope::audit_run_queued(db, asked.actor, app, environment, run).await;
    };
    let entry = AuditEntry::for_request(asked.actor, RUN_QUEUED)
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_function",
            format!("{}/{function}", app.id),
            format!("{}/{function}", app.slug),
        )
        .environment(environment.name());
    let detail = serde_json::json!({ "run_id": run_id, "function": function });
    audit::record_best_effort(db, by_publish_token(db, entry, marker, detail).await).await;
}

/// [`run_queued`] for the trigger that names no environment: production, by
/// the app's id alone. One primary-key read for the app the row is about.
pub(super) async fn run_queued_in_production(
    db: &DatabaseConnection,
    asked: Asked<'_>,
    app_id: Uuid,
    run: (&str, &str),
) {
    match apps::Entity::find_by_id(app_id).one(db).await {
        Ok(Some(app)) => run_queued(db, asked, &app, &AppEnvironment::Production, run).await,
        Ok(None) => {}
        Err(e) => tracing::warn!(%app_id, error = %e, "run now: audit skipped"),
    }
}
