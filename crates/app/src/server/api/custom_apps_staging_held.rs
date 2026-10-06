//! What a custom app's **staging** environment held, written once and listed
//! back to the developer who caused it.
//!
//! - **The writer** ([`record_held`]) is the only place an `app.staging.held`
//!   audit row is built. A staging `/fn` invocation's held-write log
//!   (`custom_apps_functions::host::held_log`) calls it on flush, on a late
//!   note and on drop; a staging agent ask and a staging automation run call
//!   it with their own `function_or_surface`. One shape, so the list reads
//!   every row the same way.
//! - **The list** (`GET /api/customer-apps/{id}/staging/held`) answers the
//!   caller's **own** rows for this app, newest first. The rule (Review focus
//!   3 of the staging-console plan): a developer sees what *their* staging
//!   requests held, never another developer's, and a caller oxy-authz does
//!   not let open the app's staging (`may_open_non_production`) gets a 404,
//!   as for an app that does not exist. Scheduled and system runs never
//!   appear: staging refuses queued runs, and their rows carry no user.
//!
//! A row written before `app_id` was stamped is still found, by
//! `metadata.app_slug` within the app's org.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::{apps, audit_events};
use oxy::database::client::establish_connection;
use oxy_app_core::audit::AuditEntry;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseConnection, DbBackend, DbErr, EntityTrait, Statement};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::custom_apps_env_resolve::may_open_non_production;
use super::custom_apps_functions::write_record::{
    ACTION_STAGING_HELD, WriteRecord, actor_entry, with_first_target,
};

/// Who caused a held row.
#[derive(Debug, Clone)]
pub(crate) enum HeldActor {
    /// The verified user (`audit_events.actor_user_id`).
    User { id: Uuid, email: Option<String> },
    /// The platform acting for the app (`system:app:<slug>`). Only a
    /// function run (behind `custom-app-functions`) has no human caller.
    #[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
    System,
}

/// One `app.staging.held` row: every call one staging invocation, ask or run
/// held or refused.
#[derive(Debug, Clone)]
pub(crate) struct HeldRow {
    pub app_id: Uuid,
    pub app_slug: String,
    pub org_id: Uuid,
    pub project_id: Uuid,
    /// `staging`, or a `dev-<handle>` slot.
    pub environment: String,
    pub actor: HeldActor,
    /// The function's name, or the surface (`agent`, `automation`).
    pub function_or_surface: String,
    /// `route` / `schedule` / `airway` / `manual`, or the surface's own.
    pub mode: String,
    pub request_id: Option<Uuid>,
    pub writes: Vec<WriteRecord>,
    pub invocation_id: Option<Uuid>,
    pub trace_id: Option<String>,
}

/// The audit entry for `row`. Metadata keeps every field the host wrote
/// before (`app_slug`, `function`, `invocation_id`, `mode`, `request_id`,
/// `trace_id`, `writes`) and adds `app_id` and `actor_user_id`.
pub(crate) fn held_entry(row: &HeldRow) -> AuditEntry {
    let user = match &row.actor {
        HeldActor::User { id, email } => Some((*id, email.as_deref())),
        HeldActor::System => None,
    };
    let e = actor_entry(ACTION_STAGING_HELD, user, &row.app_slug)
        .org(row.org_id)
        .workspace(row.project_id);
    with_first_target(e, &row.writes)
        .metadata(json!({
            "app_id": row.app_id,
            "app_slug": row.app_slug,
            "actor_user_id": user.map(|(id, _)| id),
            "function": row.function_or_surface,
            "invocation_id": row.invocation_id,
            "mode": row.mode,
            "request_id": row.request_id,
            "trace_id": row.trace_id,
            "writes": row.writes,
        }))
        .environment(row.environment.clone())
}

/// Write `row`, best effort. A row with nothing held writes nothing, so a
/// production caller that noted nothing never produces one.
pub(crate) async fn record_held(db: &DatabaseConnection, row: HeldRow) {
    if row.writes.is_empty() {
        return;
    }
    oxy_app_core::audit::record_best_effort(db, held_entry(&row)).await;
}

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
    let caller = crate::server::authz::Caller::from_user(&user);
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
    use super::*;

    fn row(actor: HeldActor) -> HeldRow {
        HeldRow {
            app_id: Uuid::from_u128(1),
            app_slug: "orders".into(),
            org_id: Uuid::from_u128(2),
            project_id: Uuid::from_u128(3),
            environment: "staging".into(),
            actor,
            function_or_surface: "save".into(),
            mode: "route".into(),
            request_id: None,
            writes: vec![WriteRecord {
                plane: "fetch",
                namespace: "api.example.com".into(),
                verb: "POST".into(),
                table: String::new(),
                rows: None,
                statements: 1,
                op: Some("fetch"),
                note: None,
            }],
            invocation_id: Some(Uuid::from_u128(4)),
            trace_id: Some("t".into()),
        }
    }

    #[test]
    fn a_held_entry_stamps_the_app_and_the_actor_column() {
        let user = Uuid::from_u128(9);
        let e = held_entry(&row(HeldActor::User {
            id: user,
            email: Some("dev@oxy.tech".into()),
        }));
        assert_eq!(e.action, "app.staging.held");
        assert_eq!(e.actor_user_id, Some(user), "the column, not only metadata");
        assert_eq!(e.actor_email, "dev@oxy.tech");
        assert_eq!(e.environment.as_deref(), Some("staging"));
        assert_eq!(e.org_id, Some(Uuid::from_u128(2)));
        assert_eq!(e.workspace_id, Some(Uuid::from_u128(3)));
        assert_eq!(e.target_id.as_deref(), Some("fetch:api.example.com"));
        let m = &e.metadata;
        assert_eq!(m["app_id"], json!(Uuid::from_u128(1)));
        assert_eq!(m["actor_user_id"], json!(user));
        for key in [
            "app_slug",
            "function",
            "invocation_id",
            "mode",
            "request_id",
            "trace_id",
            "writes",
        ] {
            assert!(m.get(key).is_some(), "keeps {key}: {m}");
        }
        assert_eq!(m["writes"][0]["op"], "fetch");
    }

    #[test]
    fn a_held_entry_with_no_user_is_the_platform_acting_for_the_app() {
        let e = held_entry(&row(HeldActor::System));
        assert_eq!(e.actor_user_id, None);
        assert_eq!(e.actor_email, "system:app:orders");
        assert!(e.metadata["actor_user_id"].is_null());
    }

    #[test]
    fn a_held_write_from_an_older_row_lists_with_missing_fields_empty() {
        let w: HeldWrite =
            serde_json::from_value(json!({ "plane": "oltp", "verb": "INSERT" })).unwrap();
        assert_eq!(w.plane, "oltp");
        assert_eq!(w.table, "");
        assert_eq!(w.op, None);
    }
}
