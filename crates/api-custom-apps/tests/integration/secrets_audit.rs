//! A **production** app-secret write is audited like every other environment's:
//! set and delete on the staff mount (`/api/customer-apps/{id}/secrets`) and a
//! set on the tenant mount (`/api/workspaces/{ws}/custom-apps/{app_id}/secrets`)
//! each leave a `custom_app.secret.*` row naming `production`, beside the
//! reveal that always did. Before, only a non-production write was recorded,
//! so the console — which wrote production alone — left no row at all.
//!
//! The tenant project-secrets routes reach the same production row by id —
//! Settings → Secrets rotates (`PUT`) and deletes (`DELETE`) an app secret
//! there — and leave the same rows; that half is `oxy-api-workspace-config`'s
//! `tenant_secrets` (moved there with those handlers, 2026-11-15).
//!
//! The handlers are called directly with the extractors the router would
//! build, as in `staging_secrets_admin`.

use axum::extract::{Path, Query};
use entity::audit_events;
use entity::workspace_members::WorkspaceRole;
use oxy_api_custom_apps::custom_apps_secrets::environment::EnvironmentQuery;
use oxy_api_custom_apps::custom_apps_secrets::{admin_delete, admin_set, workspace_set};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_server_authz::role_guards::WorkspaceAdmin;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::seeded_tenant;
use crate::staging_secrets_admin::{app_with_both_values, request, user};

/// `(action, environment, target_id)` of every secret row about `app`, sorted:
/// rows written in the same instant have no order to assert.
async fn secret_rows(
    db: &sea_orm::DatabaseConnection,
    org: Uuid,
    app: Uuid,
) -> Vec<(String, String, String)> {
    let mut rows: Vec<_> = audit_events::Entity::find()
        .filter(audit_events::Column::OrgId.eq(org))
        .filter(audit_events::Column::Action.starts_with("custom_app.secret."))
        .filter(audit_events::Column::TargetId.contains(app.to_string()))
        .all(db)
        .await
        .expect("audit rows")
        .into_iter()
        .map(|r| (r.action, r.environment, r.target_id.unwrap_or_default()))
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn production_secret_writes_are_audited_on_both_mounts() {
    let t = seeded_tenant().await;
    // Staging's value, then production's — both set by staff.
    let app = app_with_both_values(&t, "prod-audit").await;
    let staff = || user(t.guest_id, LOCAL_GUEST_EMAIL);

    admin_delete(
        Path((app, "QB_TOKEN".to_string())),
        Query(EnvironmentQuery::default()),
        staff(),
        None,
    )
    .await
    .expect("staff delete production's");
    workspace_set(
        WorkspaceAdmin(WorkspaceRole::Admin),
        Path((demo_workspace_id(), app)),
        user(t.guest_id, LOCAL_GUEST_EMAIL),
        None,
        request("WEBHOOK_SECRET", "whsec", None),
    )
    .await
    .expect("tenant set production's");
    admin_set(Path(app), staff(), None, request("QB_TOKEN", "again", None))
        .await
        .expect("staff set production's again");

    let production = |action: &str, key: &str| {
        (
            action.to_string(),
            "production".to_string(),
            format!("apps/{app}/{key}"),
        )
    };
    let rows = secret_rows(&t.db, t.org_id, app).await;
    let production_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.1 == "production")
        .cloned()
        .collect();
    assert_eq!(
        production_rows,
        vec![
            production("custom_app.secret.deleted", "QB_TOKEN"),
            production("custom_app.secret.set", "QB_TOKEN"),
            production("custom_app.secret.set", "QB_TOKEN"),
            production("custom_app.secret.set", "WEBHOOK_SECRET"),
        ],
        "every production write, on either mount, leaves a row: {rows:?}"
    );
}
