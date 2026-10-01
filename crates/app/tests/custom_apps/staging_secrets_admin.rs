//! Setting a **staging** app secret on the staff secrets surface
//! (`/api/customer-apps/{id}/secrets`, `custom_apps_secrets`): the
//! `environment` option writes `apps/<id>/staging/<KEY>`, is decided by
//! oxy-authz (`AppNonProduction`) so only staff who may open the app's
//! non-production environments reach it, and each environment's view lists
//! its own keys alone.
//!
//! A key is one path segment on every route, so no request names another
//! environment's row; staff writes of a staging value are audited with the
//! environment; and the tenant project-secrets routes (`api::secrets`), which
//! address rows by id, never list or reach a staging row.
//!
//! The handlers are called directly with the extractors the router would
//! build; the mount's own guard (PlatformApps + scope) is `authz`'s.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::users::UserStatus;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app::server::api::custom_apps_secrets::environment::EnvironmentQuery;
use oxy_app::server::api::custom_apps_secrets::{
    SetSecretRequest, admin_delete, admin_list, admin_reveal, admin_set,
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, publish_build, seeded_tenant};
use crate::staging_functions::make_guest_staff;

fn user(id: Uuid, email: &str) -> AuthenticatedUserExtractor {
    AuthenticatedUserExtractor(AuthenticatedUser {
        id,
        email: Some(email.to_string()),
        name: "Secrets".to_string(),
        picture: None,
        status: UserStatus::Active,
    })
}

fn request(key: &str, value: &str, environment: Option<&str>) -> Json<SetSecretRequest> {
    Json(
        serde_json::from_value(json!({ "key": key, "value": value, "environment": environment }))
            .expect("request"),
    )
}

fn env(environment: &str) -> Query<EnvironmentQuery> {
    Query(EnvironmentQuery {
        environment: Some(environment.to_string()),
    })
}

#[tokio::test]
async fn staff_set_a_staging_secret_that_a_tenant_admin_cannot() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );
    }
    let t = seeded_tenant().await;
    let spec = FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    };
    let app = publish_build(&t, "stg-admin", demo_workspace_id(), "adm-1", true, &[spec])
        .await
        .app_id;
    make_guest_staff();
    let staff = || user(t.guest_id, LOCAL_GUEST_EMAIL);

    let status = admin_set(
        Path(app),
        staff(),
        request("QB_TOKEN", "stg", Some("staging")),
    )
    .await
    .expect("staff set staging");
    assert_eq!(status, StatusCode::NO_CONTENT);
    admin_set(Path(app), staff(), request("QB_TOKEN", "prod", None))
        .await
        .expect("production is the default");

    let value = |name: String| async move {
        SecretManagerService::new(demo_workspace_id())
            .get_secret(&name)
            .await
    };
    assert_eq!(
        value(format!("apps/{app}/staging/QB_TOKEN"))
            .await
            .as_deref(),
        Some("stg")
    );
    assert_eq!(
        value(format!("apps/{app}/QB_TOKEN")).await.as_deref(),
        Some("prod")
    );

    let Json(prod) = admin_list(Path(app), Query(EnvironmentQuery::default()), staff())
        .await
        .expect("production view");
    let keys: Vec<&str> = prod.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, vec!["QB_TOKEN"], "never `staging/QB_TOKEN`");
    assert_eq!(prod.environment, "production");
    let Json(stg) = admin_list(Path(app), env("staging"), staff())
        .await
        .expect("staging view");
    assert_eq!(stg.environment, "staging");
    assert_eq!(stg.entries.len(), 1);
    assert!(stg.entries[0].is_set);

    // Someone oxy-authz does not let open staging: refused, nothing written.
    let tenant = || user(Uuid::new_v4(), "tenant-admin@customer.example");
    let (code, _) = admin_set(
        Path(app),
        tenant(),
        request("QB_TOKEN", "x", Some("staging")),
    )
    .await
    .expect_err("a non-staff caller is refused staging");
    assert_eq!(code, StatusCode::FORBIDDEN);
    let (code, _) = admin_list(Path(app), env("staging"), tenant())
        .await
        .map(|_| ())
        .expect_err("and its view");
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert_eq!(
        value(format!("apps/{app}/staging/QB_TOKEN"))
            .await
            .as_deref(),
        Some("stg")
    );
    let (code, _) = admin_set(Path(app), staff(), request("K", "v", Some("dev-luong")))
        .await
        .expect_err("a dev slot holds no secrets");
    assert_eq!(code, StatusCode::BAD_REQUEST);

    admin_delete(Path((app, "QB_TOKEN".to_string())), env("staging"), staff())
        .await
        .expect("delete staging's");
    assert_eq!(value(format!("apps/{app}/staging/QB_TOKEN")).await, None);
    assert_eq!(
        value(format!("apps/{app}/QB_TOKEN")).await.as_deref(),
        Some("prod"),
        "production's key is untouched"
    );
}

fn use_a_fixed_encryption_key() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );
    }
}

/// A published app with `QB_TOKEN` set in production (`prod`) and staging
/// (`stg`) by staff.
async fn app_with_both_values(t: &Tenant, slug: &str) -> Uuid {
    use_a_fixed_encryption_key();
    let spec = FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    };
    let app = publish_build(t, slug, demo_workspace_id(), "adm-1", true, &[spec])
        .await
        .app_id;
    make_guest_staff();
    let staff = || user(t.guest_id, LOCAL_GUEST_EMAIL);
    admin_set(
        Path(app),
        staff(),
        request("QB_TOKEN", "stg", Some("staging")),
    )
    .await
    .expect("staging value");
    admin_set(Path(app), staff(), request("QB_TOKEN", "prod", None))
        .await
        .expect("production value");
    app
}

async fn stored(name: String) -> Option<String> {
    SecretManagerService::new(demo_workspace_id())
        .get_secret(&name)
        .await
}

/// The review's escape: `DELETE …/secrets/staging%2FQB_TOKEN` decodes to the
/// key `staging/QB_TOKEN`, which named staging's row on a production request —
/// past the `AppNonProduction` gate, and audited as production.
#[tokio::test]
async fn a_key_never_names_another_environments_row() {
    let t = seeded_tenant().await;
    let app = app_with_both_values(&t, "stg-keyseg").await;
    let staff = || user(t.guest_id, LOCAL_GUEST_EMAIL);
    let production = || Query(EnvironmentQuery::default());
    let escape = "staging/QB_TOKEN".to_string();

    let (code, _) = admin_delete(Path((app, escape.clone())), production(), staff())
        .await
        .expect_err("delete");
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, _) = admin_reveal(Path((app, escape.clone())), production(), staff())
        .await
        .map(|_| ())
        .expect_err("reveal");
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, _) = admin_set(Path(app), staff(), request(&escape, "x", None))
        .await
        .expect_err("set");
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(
        stored(format!("apps/{app}/staging/QB_TOKEN"))
            .await
            .as_deref(),
        Some("stg"),
        "staging's row is untouched"
    );
}

/// Every staff write of a staging value leaves a row naming the environment.
#[tokio::test]
async fn staging_secret_writes_are_audited_with_their_environment() {
    use entity::audit_events;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let t = seeded_tenant().await;
    let app = app_with_both_values(&t, "stg-audit").await;
    admin_delete(
        Path((app, "QB_TOKEN".to_string())),
        env("staging"),
        user(t.guest_id, LOCAL_GUEST_EMAIL),
    )
    .await
    .expect("delete staging's");
    let rows = audit_events::Entity::find()
        .filter(audit_events::Column::OrgId.eq(t.org_id))
        .filter(audit_events::Column::Action.starts_with("custom_app.secret."))
        .filter(audit_events::Column::TargetId.contains(app.to_string()))
        .all(&t.db)
        .await
        .expect("audit rows");
    let mut seen: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.action.clone(), r.environment.clone()))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (
                "custom_app.secret.deleted".to_string(),
                "staging".to_string()
            ),
            ("custom_app.secret.set".to_string(), "staging".to_string()),
        ],
        "one row per staging write; production's writes are logged as before"
    );
}

async fn body_of(response: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

/// The tenant project-secrets routes address rows by id: a workspace admin
/// must neither list staging's row nor read, rotate or delete it by its id.
#[tokio::test]
async fn tenant_project_secret_routes_never_reach_a_staging_row() {
    use axum::response::IntoResponse;
    use entity::workspace_members::WorkspaceRole;
    use oxy_app::server::api::secrets;
    use oxy_server_authz::role_guards::WorkspaceAdmin;
    let t = seeded_tenant().await;
    let app = app_with_both_values(&t, "stg-tenant").await;
    let ws = demo_workspace_id();
    let admin = || WorkspaceAdmin(WorkspaceRole::Admin);
    let tenant = || user(Uuid::new_v4(), "tenant-admin@customer.example");

    let listed = secrets::list_secrets(admin(), tenant(), Path(ws))
        .await
        .expect("list")
        .into_response();
    let (_, listed) = body_of(listed).await;
    let names: Vec<&str> = listed["secrets"]
        .as_array()
        .expect("secrets")
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert!(
        names.contains(&format!("apps/{app}/QB_TOKEN").as_str()),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|n| n.contains("/staging/")),
        "a staging row is staff-only: {names:?}"
    );

    let staging_id = SecretManagerService::new(ws)
        .list_secrets(&t.db)
        .await
        .expect("rows")
        .into_iter()
        .find(|s| s.name == format!("apps/{app}/staging/QB_TOKEN"))
        .expect("staging row")
        .id
        .to_string();
    let by_id = || Path((ws, staging_id.clone()));
    let got = secrets::get_secret(admin(), tenant(), by_id())
        .await
        .unwrap();
    assert_eq!(got.into_response().status(), StatusCode::NOT_FOUND, "get");
    let revealed = secrets::reveal_secret(admin(), tenant(), by_id())
        .await
        .unwrap();
    assert_eq!(
        revealed.into_response().status(),
        StatusCode::NOT_FOUND,
        "reveal"
    );
    let rotate: secrets::UpdateSecretRequest =
        serde_json::from_value(json!({ "value": "rotated" })).expect("request");
    let updated = secrets::update_secret(admin(), tenant(), by_id(), axum::Json(rotate))
        .await
        .unwrap();
    assert_eq!(
        updated.into_response().status(),
        StatusCode::NOT_FOUND,
        "update"
    );
    let deleted = secrets::delete_secret(admin(), tenant(), by_id())
        .await
        .unwrap();
    assert_eq!(
        deleted.into_response().status(),
        StatusCode::NOT_FOUND,
        "delete"
    );
    assert_eq!(
        stored(format!("apps/{app}/staging/QB_TOKEN"))
            .await
            .as_deref(),
        Some("stg"),
        "staging's row is untouched"
    );
}
