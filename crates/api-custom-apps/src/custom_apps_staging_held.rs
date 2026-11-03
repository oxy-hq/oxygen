//! The caller's own staging held-write list: `GET /api/customer-apps/{id}/staging/held`.
//!
//! A developer sees what *their* staging requests held, never another
//! developer's, and a caller oxy-authz does not let open the app's staging
//! (`may_open_non_production`) gets a 404, as for an app that does not exist.
//! Scheduled and system runs never appear: staging refuses queued runs, and
//! their rows carry no user. The row itself is written by
//! `oxy_custom_apps::staging_held::record_held`.
//!
//! A row written before `app_id` was stamped is still found, by
//! `metadata.app_slug` within the app's org.

pub use oxy_custom_apps::staging_held::{HeldActor, HeldRow, held_entry, record_held};

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::{apps, audit_events};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_custom_apps::write_record::ACTION_STAGING_HELD;
use sea_orm::{DatabaseConnection, DbBackend, DbErr, EntityTrait, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::custom_apps_env_resolve::may_open_non_production;

// ── the list ─────────────────────────────────────────────────────────────

const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 500;

#[derive(Debug, Default, Deserialize)]
pub struct HeldQuery {
    #[serde(default)]
    pub limit: Option<u64>,
}

/// One held row as the console lists it.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct HeldEntry {
    pub at: chrono::DateTime<chrono::FixedOffset>,
    pub function: String,
    pub writes: Vec<HeldWrite>,
}

/// One held call. Unknown or missing fields read as empty, so an older row
/// still lists.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HeldWrite {
    pub plane: String,
    pub namespace: String,
    pub verb: String,
    pub table: String,
    pub op: Option<String>,
    pub note: Option<String>,
    /// A sent provider-sandbox call's path (never its query) and response
    /// status; absent on a held call, and left out of the JSON then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

type Failure = (StatusCode, String);

fn not_found() -> Failure {
    (StatusCode::NOT_FOUND, "app not found".to_string())
}

fn internal(context: &'static str) -> impl FnOnce(DbErr) -> Failure {
    move |e| {
        tracing::error!(error = %e, "{context}");
        (StatusCode::INTERNAL_SERVER_ERROR, context.to_string())
    }
}

/// `GET /api/customer-apps/{id}/staging/held?limit=`: the caller's own held
/// rows for this app, newest first (default 100, at most 500). 404 for a
/// caller who may not open the app's staging.
pub async fn list_held(
    Path(app_id): Path<Uuid>,
    Query(q): Query<HeldQuery>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<Json<Vec<HeldEntry>>, Failure> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!(error = %e, "staging held: database unavailable");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "database unavailable".to_string(),
        )
    })?;
    let app = apps::Entity::find_by_id(app_id)
        .one(&db)
        .await
        .map_err(internal("app lookup failed"))?
        .ok_or_else(not_found)?;
    let caller = oxy_server_authz::Caller::from_user(&user);
    if !may_open_non_production(&db, &caller, &app).await {
        return Err(not_found());
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let rows = held_for(&db, &app, user.id, limit)
        .await
        .map_err(internal("staging held lookup failed"))?;
    Ok(Json(rows))
}

/// `actor`'s `app.staging.held` rows for `app` in staging, newest first. A
/// row with no `app_id` (written before it was stamped) matches by slug
/// within the app's org.
async fn held_for(
    db: &DatabaseConnection,
    app: &apps::Model,
    actor: Uuid,
    limit: u64,
) -> Result<Vec<HeldEntry>, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"SELECT * FROM audit_events
           WHERE action = $1
             AND environment = 'staging'
             AND actor_user_id = $2
             AND (metadata->>'app_id' = $3
                  OR (metadata->>'app_id' IS NULL
                      AND metadata->>'app_slug' = $4
                      AND org_id = $5))
           ORDER BY created_at DESC, seq DESC
           LIMIT $6"#,
        [
            ACTION_STAGING_HELD.into(),
            actor.into(),
            app.id.to_string().into(),
            app.slug.clone().into(),
            app.org_id.into(),
            (limit as i64).into(),
        ],
    );
    let rows = audit_events::Entity::find()
        .from_raw_sql(stmt)
        .all(db)
        .await?;
    Ok(rows.iter().map(listed).collect())
}

fn listed(row: &audit_events::Model) -> HeldEntry {
    let writes = row.metadata["writes"]
        .as_array()
        .map(|ws| {
            ws.iter()
                .map(|w| serde_json::from_value(w.clone()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    HeldEntry {
        at: row.created_at,
        function: row.metadata["function"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        writes,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_held_write_from_an_older_row_lists_with_missing_fields_empty() {
        let w: HeldWrite =
            serde_json::from_value(json!({ "plane": "oltp", "verb": "INSERT" })).unwrap();
        assert_eq!(w.plane, "oltp");
        assert_eq!(w.table, "");
        assert_eq!(w.op, None);
        assert_eq!((w.path, w.status), (None, None));
    }

    #[test]
    fn a_sent_call_lists_with_its_path_and_status() {
        let w: HeldWrite = serde_json::from_value(json!({
            "plane": "fetch", "namespace": "api.stripe.com", "verb": "POST",
            "op": "fetch", "path": "/v1/charges", "status": 200
        }))
        .unwrap();
        assert_eq!(w.path.as_deref(), Some("/v1/charges"));
        assert_eq!(w.status, Some(200));
    }

    #[test]
    fn a_held_call_serializes_without_path_or_status_and_a_sent_one_with() {
        let held = serde_json::to_value(HeldWrite::default()).unwrap();
        assert!(held.get("path").is_none() && held.get("status").is_none());
        let sent = serde_json::to_value(HeldWrite {
            path: Some("/v1/charges".into()),
            status: Some(200),
            ..HeldWrite::default()
        })
        .unwrap();
        assert_eq!(
            (sent["path"].as_str(), sent["status"].as_u64()),
            (Some("/v1/charges"), Some(200))
        );
    }
}
