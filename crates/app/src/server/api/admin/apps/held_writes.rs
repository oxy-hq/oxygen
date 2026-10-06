//! The held-write read-back: what one non-production invocation's environment
//! policy did **not** perform — what production would have done.
//!
//! The host records every held or refused call of an invocation in one
//! `app.staging.held` audit row (`custom_apps_functions::host::held_log`),
//! keyed by `metadata.invocation_id`. This reads those rows back for one
//! invocation of one app.
//!
//! **Three fences, in order**, because a held list names tables, statements
//! and third-party hosts of someone's app:
//!
//! 1. the mount's guards — staff with `manage_apps`, and an app inside the
//!    grant's org scope (`enforce_app_scope` reads `{id}`);
//! 2. **the invocation must belong to the app in the path.** An id that
//!    exists under another app answers the same `404` as one that does not
//!    exist, so the route cannot be used to learn which ids are real;
//! 3. a non-production invocation needs `may_open_non_production` for that
//!    app. A production invocation holds nothing and answers `held: []`
//!    without asking.
//!
//! The audit read is then scoped to the app's org and the invocation's id,
//! and bounded in time and rows. DB-only (FleetOk).

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Duration;
use entity::prelude::{AppBuilds, AppFunctionInvocations};
use entity::{app_function_invocations, apps, audit_events};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::sea_query::{Expr, extension::postgres::PgExpr};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::Serialize;
use uuid::Uuid;

use super::environment_scope::{self, ScopeError};

/// The audit action a held-write row is recorded under, in every
/// non-production environment (the `environment` column says which). A
/// stored, searched literal: `custom_apps_functions::data_audit`.
const HELD_ACTION: &str = "app.staging.held";

/// How long after an invocation starts its held rows can still be written:
/// far past the 300 s function ceiling, with room for a detached host call
/// that finishes late and is recorded on a row of its own.
const HELD_WINDOW_HOURS: i64 = 1;

/// The most audit rows one read concatenates. An invocation writes one row,
/// plus one per call noted after its log closed.
const MAX_HELD_ROWS: u64 = 200;

/// One invocation's held writes.
#[derive(Debug, Serialize)]
pub struct HeldWrites {
    pub invocation_id: Uuid,
    pub environment: String,
    pub function: String,
    /// The publish's build id of the build that ran.
    pub build_id: Option<String>,
    pub status: String,
    /// Each held or refused call, in the order the host noted them: the
    /// `writes` of the invocation's `app.staging.held` rows, concatenated.
    pub held: Vec<serde_json::Value>,
}

/// The invocation `invocation_id` **of app `app_id`**. One `404` for an id
/// that does not exist and for one that belongs to another app.
async fn invocation_of(
    db: &DatabaseConnection,
    app_id: Uuid,
    invocation_id: Uuid,
) -> Result<app_function_invocations::Model, ScopeError> {
    AppFunctionInvocations::find_by_id(invocation_id)
        .filter(app_function_invocations::Column::AppId.eq(app_id))
        .one(db)
        .await
        .map_err(|e| ScopeError::internal("invocation lookup failed", e))?
        .ok_or_else(|| {
            ScopeError::new(
                StatusCode::NOT_FOUND,
                "invocation_not_found",
                "no such invocation of this app",
            )
        })
}

/// The `writes` of every held row `invocation` wrote, oldest row first.
///
/// Scoped to the app's org and to the hour after the invocation began — the
/// index on `(org_id, created_at)` bounds the scan — and matched on the id
/// the host stamped in `metadata`, so another invocation's row, in this org
/// or any other, is never read.
async fn held_by(
    db: &DatabaseConnection,
    app: &apps::Model,
    invocation: &app_function_invocations::Model,
) -> Result<Vec<serde_json::Value>, ScopeError> {
    let from = invocation.created_at;
    let until = from + Duration::hours(HELD_WINDOW_HOURS);
    let rows = audit_events::Entity::find()
        .filter(audit_events::Column::OrgId.eq(app.org_id))
        .filter(audit_events::Column::Action.eq(HELD_ACTION))
        .filter(audit_events::Column::CreatedAt.gte(from))
        .filter(audit_events::Column::CreatedAt.lte(until))
        .filter(ExprTrait::eq(
            Expr::col(audit_events::Column::Metadata).cast_json_field("invocation_id"),
            invocation.id.to_string(),
        ))
        .order_by_asc(audit_events::Column::Seq)
        .limit(MAX_HELD_ROWS)
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("held-write lookup failed", e))?;
    Ok(rows.into_iter().flat_map(writes_of).collect())
}

/// The held calls one audit row lists.
fn writes_of(row: audit_events::Model) -> Vec<serde_json::Value> {
    match row.metadata {
        serde_json::Value::Object(mut metadata) => match metadata.remove("writes") {
            Some(serde_json::Value::Array(writes)) => writes,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// The publish's build id of build `build_id` of app `app_id`.
async fn build_label(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_id: Uuid,
) -> Result<Option<String>, ScopeError> {
    Ok(AppBuilds::find_by_id(build_id)
        .filter(entity::app_builds::Column::AppId.eq(app_id))
        .one(db)
        .await
        .map_err(|e| ScopeError::internal("app_builds lookup failed", e))?
        .map(|b| b.build_id))
}

/// `GET /admin/apps/{id}/invocations/{invocation_id}/held` — the held-write
/// list of one invocation of app `id`. `held` is `[]` for a production
/// invocation, which holds nothing.
pub async fn get_held_writes(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((id, invocation_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<HeldWrites>, ScopeError> {
    let db = environment_scope::connect().await?;
    let app = environment_scope::load_app(&db, id).await?;
    let invocation = invocation_of(&db, id, invocation_id).await?;
    environment_scope::require_reach(&db, &app, &user, &invocation.environment).await?;
    let held = if environment_scope::is_production(&invocation.environment) {
        Vec::new()
    } else {
        held_by(&db, &app, &invocation).await?
    };
    Ok(Json(HeldWrites {
        invocation_id: invocation.id,
        build_id: build_label(&db, id, invocation.build_id).await?,
        environment: invocation.environment,
        function: invocation.function_name,
        status: invocation.status,
        held,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(metadata: serde_json::Value) -> audit_events::Model {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(),
            "created_at": chrono::Utc::now().fixed_offset(),
            "actor_user_id": null,
            "actor_email": "system:app:x",
            "actor_type": "system",
            "action": HELD_ACTION,
            "org_id": Uuid::new_v4(),
            "workspace_id": null,
            "partner_id": null,
            "target_type": null,
            "target_id": null,
            "target_label": null,
            "before": null,
            "after": null,
            "ip": null,
            "user_agent": null,
            "request_id": null,
            "outcome": "success",
            "reason": null,
            "metadata": metadata,
            "prev_hash": null,
            "hash": null,
            "seq": 1,
            "environment": "staging",
        }))
        .expect("an audit_events row")
    }

    /// A held row's `writes` are returned as the host wrote them; a row with
    /// none — or with metadata of another shape — contributes nothing rather
    /// than failing the read.
    #[test]
    fn held_writes_are_the_rows_writes_and_a_malformed_row_holds_nothing() {
        let writes = serde_json::json!([
            { "op": "fetch", "plane": "fetch", "verb": "POST" },
            { "op": "oltp.exec", "plane": "oltp", "table": "orders" },
        ]);
        let listed = writes_of(row(serde_json::json!({
            "invocation_id": Uuid::new_v4(),
            "writes": writes,
        })));
        assert_eq!(serde_json::Value::Array(listed), writes);

        for malformed in [
            serde_json::json!({}),
            serde_json::json!({ "writes": "not a list" }),
            serde_json::json!({ "writes": null }),
            serde_json::json!("not an object"),
        ] {
            assert!(writes_of(row(malformed.clone())).is_empty(), "{malformed}");
        }
    }
}
