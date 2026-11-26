//! `POST /api/customer-apps/{id}/oltp-changes/resync`: re-baseline an app's
//! captured tables (`oxyc oltp changes resync`), over
//! [`crate::oltp_changes::resync`]. Staff only: it is not on a publish
//! token's allow-list, and it is audited before and after.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::prelude::Apps;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::{self, RequestActor};
use sea_orm::EntityTrait;
use serde_json::json;
use uuid::Uuid;

use crate::oltp_changes::relay::PRODUCTION;
use crate::oltp_changes::resync::{self, ResyncError, ResyncReport, ResyncRequest};

fn internal<E>(_: E) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_owned(),
    )
}

fn status(e: &ResyncError) -> StatusCode {
    match e {
        ResyncError::Off | ResyncError::Restamping | ResyncError::Bumped => StatusCode::CONFLICT,
        ResyncError::NotProduction(_) => StatusCode::BAD_REQUEST,
        ResyncError::NoCapture | ResyncError::UnknownTable(_) => StatusCode::NOT_FOUND,
        ResyncError::Infra(_) | ResyncError::AfterDiscard { .. } => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// One audit row of a resync: `actor` on `app`, `ok` or failed.
struct Audit<'a> {
    actor: &'a RequestActor,
    app: &'a entity::apps::Model,
    ok: bool,
}

impl Audit<'_> {
    async fn record(
        &self,
        db: &sea_orm::DatabaseConnection,
        action: &'static str,
        metadata: serde_json::Value,
    ) {
        let entry = audit::AuditEntry::for_request(self.actor, action)
            .org(self.app.org_id)
            .target("app", self.app.id.to_string(), self.app.slug.clone())
            .metadata(metadata);
        let entry = if self.ok {
            entry
        } else {
            entry.failure("the resync was refused or failed")
        };
        audit::record_best_effort(db, entry).await;
    }
}

/// The route, merged into the app admin router (and so behind its guards).
pub fn routes() -> axum::Router<oxy_app_core::AppState> {
    axum::Router::new().route("/{id}/oltp-changes/resync", axum::routing::post(resync))
}

pub async fn resync(
    _: oxy_app_core::agent_refusal::RefuseSandboxAgent,
    actor: RequestActor,
    Path(id): Path<Uuid>,
    Json(body): Json<ResyncRequest>,
) -> Result<Json<ResyncReport>, (StatusCode, String)> {
    let db = establish_connection().await.map_err(internal)?;
    let app = Apps::find_by_id(id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "not found".to_owned()))?;
    oxy_server_authz::staff::scope::deny_out_of_scope(&db, &actor.user, app.org_id)
        .await
        .map_err(|s| (s, "not found".to_owned()))?;
    let table = body.table.as_deref();
    let started = json!({ "app": app.slug, "table": table });
    Audit {
        actor: &actor,
        app: &app,
        ok: true,
    }
    .record(&db, "oltp.changes.resync_started", started)
    .await;

    let out = match body.environment.as_deref().unwrap_or(PRODUCTION) {
        PRODUCTION => resync::resync(&db, app.org_id, &app.slug, table).await,
        other => Err(ResyncError::NotProduction(other.to_owned())),
    };
    let metadata = match &out {
        Ok(r) => json!({
            "app": app.slug,
            "writer": r.writer,
            "requested": r.requested,
            "released": r.released,
            "discarded": r.discarded,
        }),
        Err(ResyncError::AfterDiscard { discarded, cause }) => {
            json!({ "app": app.slug, "table": table, "discarded": discarded, "error": cause })
        }
        Err(e) => json!({ "app": app.slug, "table": table, "error": e.to_string() }),
    };
    Audit {
        actor: &actor,
        app: &app,
        ok: out.is_ok(),
    }
    .record(&db, "oltp.changes.resynced", metadata)
    .await;
    out.map(Json).map_err(|e| (status(&e), e.to_string()))
}
