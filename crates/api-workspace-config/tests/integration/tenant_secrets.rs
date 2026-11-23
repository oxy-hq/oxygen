//! The tenant project-secrets routes against a custom app's secrets: they
//! address rows by id, so a workspace admin must neither list a staging row nor
//! read, rotate or delete it by its id; and a rotate or delete of a
//! **production** app secret by id leaves the same `custom_app.secret.*` audit
//! row a staff write does, while a description-only edit and a plain project
//! secret leave none.
//!
//! Rehomed from `oxy-api-custom-apps`' `staging_secrets_admin`
//! (`tenant_project_secret_routes_never_reach_a_staging_row`) and
//! `secrets_audit` (`tenant_rotate_and_delete_by_id_are_audited`) when the
//! handlers left `oxy-app` (2026-11-15): S3 forbids that crate depending on
//! this one even for tests. Their fixture published an app and set its values
//! through the staff surface; what these handlers decide reads the secret's
//! NAME alone (`oxy_custom_apps::secret_scope`) and, for the audit, the app row
//! the name points at. So the rows are written with the writer the staff
//! surface's handlers call (`SecretManagerService::set_app_secret_in`, which
//! owns the `apps/<id>/[<env>/]KEY` names), beside a bare `apps` row in the
//! seeded org. That the staff surface's routes reach that writer stays
//! `oxy-api-custom-apps`' to prove.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use entity::audit_events;
use entity::users::UserStatus;
use entity::workspace_members::WorkspaceRole;
use oxy::service::secret_manager::{CreateSecretParams, SecretManagerService};
use oxy_api_workspace_config::secrets;
use oxy_app_core::audit::RequestActor;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::{AuthenticatedUser, Identity};
use oxy_auth::user::{LOCAL_GUEST_EMAIL, UserService};
use oxy_server_authz::role_guards::WorkspaceAdmin;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::{Schema, demo_workspace_id, examples_path, test_db_with};

/// The seeded database, the Local org, its guest, and one app in it.
struct World {
    db: DatabaseConnection,
    org: Uuid,
    guest: Uuid,
    app: Uuid,
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

/// The demo seed's rows (the Local org and the demo workspace), the guest
/// user, and a bare `apps` row in that org and workspace: the row
/// `audit_if_app_secret` looks the secret's app up in.
async fn world() -> World {
    use_a_fixed_encryption_key();
    let db = test_db_with(Schema::All).await;
    oxy_server::dev::seed::seed_demo_with(
        Some(examples_path()),
        oxy_server::dev::seed::SeedOptions {
            compile: false,
            llm_keys: false,
            fixtures: false,
            airhouse: false,
        },
        None,
    )
    .await
    .expect("seed_demo");
    let org = entity::organizations::Entity::find()
        .filter(entity::organizations::Column::Slug.eq("local"))
        .one(&db)
        .await
        .expect("query org")
        .expect("seed_demo creates the local org")
        .id;
    let guest = UserService::get_or_create_user(&Identity {
        user_id: None,
        email: LOCAL_GUEST_EMAIL.to_string(),
        name: Some("Local User".to_string()),
        picture: None,
    })
    .await
    .expect("guest user")
    .id;
    let app = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    entity::apps::ActiveModel {
        id: Set(app),
        slug: Set(format!("secrets-{}", &app.simple().to_string()[..8])),
        name: Set("Secrets".into()),
        org_id: Set(org),
        project_id: Set(demo_workspace_id()),
        branch: Set("main".into()),
        source_repo: Set(String::new()),
        status: Set("live".into()),
        source_type: Set("upload".into()),
        source_config: Set(json!({})),
        visibility: Set("org".into()),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("app row");
    World {
        db,
        org,
        guest,
        app,
    }
}

async fn put(w: &World, name: String, value: &str) {
    SecretManagerService::new(demo_workspace_id())
        .create_secret(
            &w.db,
            CreateSecretParams {
                name,
                value: value.to_string(),
                description: None,
                created_by: w.guest,
            },
        )
        .await
        .expect("secret row");
}

/// The app's `QB_TOKEN`, staging's (`stg`) and production's (`prod`), through
/// the writer the staff surface's handlers call (`set_app_secret_in`), so the
/// rows carry the names it gives them.
async fn both_values(w: &World) {
    let secrets = SecretManagerService::new(demo_workspace_id());
    for (environment, value) in [(Some("staging"), "stg"), (None, "prod")] {
        secrets
            .set_app_secret_in(&w.db, w.app, environment, "QB_TOKEN", value, w.guest)
            .await
            .expect("app secret");
    }
}

async fn row_id(db: &DatabaseConnection, name: &str) -> String {
    SecretManagerService::new(demo_workspace_id())
        .list_secrets(db)
        .await
        .expect("rows")
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no row {name}"))
        .id
        .to_string()
}

async fn stored(name: String) -> Option<String> {
    SecretManagerService::new(demo_workspace_id())
        .get_secret(&name)
        .await
}

fn tenant_user() -> AuthenticatedUser {
    AuthenticatedUser {
        id: Uuid::new_v4(),
        email: Some("tenant-admin@customer.example".to_string()),
        name: "Secrets".to_string(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    }
}

fn tenant() -> RequestActor {
    RequestActor::session(tenant_user())
}

fn admin() -> WorkspaceAdmin {
    WorkspaceAdmin(WorkspaceRole::Admin)
}

async fn body_of(response: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

fn edit(body: serde_json::Value) -> axum::Json<secrets::UpdateSecretRequest> {
    axum::Json(serde_json::from_value(body).expect("request"))
}

/// The tenant project-secrets routes address rows by id: a workspace admin
/// must neither list staging's row nor read, rotate or delete it by its id.
#[tokio::test]
async fn tenant_project_secret_routes_never_reach_a_staging_row() {
    let w = world().await;
    both_values(&w).await;
    let ws = demo_workspace_id();
    let app = w.app;

    let listed =
        secrets::list_secrets(admin(), AuthenticatedUserExtractor(tenant_user()), Path(ws))
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

    let staging_id = row_id(&w.db, &format!("apps/{app}/staging/QB_TOKEN")).await;
    let by_id = || Path((ws, staging_id.clone()));
    let got = secrets::get_secret(admin(), AuthenticatedUserExtractor(tenant_user()), by_id())
        .await
        .unwrap();
    assert_eq!(got.into_response().status(), StatusCode::NOT_FOUND, "get");
    let revealed =
        secrets::reveal_secret(admin(), AuthenticatedUserExtractor(tenant_user()), by_id())
            .await
            .unwrap();
    assert_eq!(
        revealed.into_response().status(),
        StatusCode::NOT_FOUND,
        "reveal"
    );
    let updated = secrets::update_secret(
        admin(),
        tenant(),
        by_id(),
        edit(json!({ "value": "rotated" })),
    )
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

/// `(action, environment, target_id)` of every production secret row about
/// `app`, sorted: rows written in the same instant have no order to assert.
async fn production_rows(
    db: &DatabaseConnection,
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
        .filter(|r| r.1 == "production")
        .collect();
    rows.sort();
    rows
}

/// A rotate and a delete by id of a production app secret are audited; a
/// description-only edit and a plain project secret are not.
#[tokio::test]
async fn tenant_rotate_and_delete_by_id_are_audited() {
    let w = world().await;
    both_values(&w).await;
    let ws = demo_workspace_id();
    let app = w.app;
    // A plain project secret beside the app's, which no app row is about.
    let plain_name = format!("PLAIN_{}", Uuid::new_v4().simple());
    put(&w, plain_name.clone(), "v").await;

    let production = row_id(&w.db, &format!("apps/{app}/QB_TOKEN")).await;
    let plain = row_id(&w.db, &plain_name).await;
    let before = production_rows(&w.db, w.org, app).await;
    assert!(before.is_empty(), "rows written directly: {before:?}");
    for (id, body) in [
        (&production, json!({ "description": "notes only" })),
        (&production, json!({ "value": "rotated" })),
        (&plain, json!({ "value": "rotated" })),
    ] {
        let answer = secrets::update_secret(admin(), tenant(), Path((ws, id.clone())), edit(body))
            .await
            .expect("update");
        assert_eq!(answer.into_response().status(), StatusCode::OK);
    }
    for id in [&production, &plain] {
        let answer = secrets::delete_secret(admin(), tenant(), Path((ws, id.clone())))
            .await
            .expect("delete");
        assert_eq!(answer.into_response().status(), StatusCode::NO_CONTENT);
    }

    let target = format!("apps/{app}/QB_TOKEN");
    assert_eq!(
        production_rows(&w.db, w.org, app).await,
        vec![
            (
                "custom_app.secret.deleted".to_string(),
                "production".to_string(),
                target.clone()
            ),
            (
                "custom_app.secret.set".to_string(),
                "production".to_string(),
                target
            ),
        ],
        "one row for the rotation and one for the delete; none for the \
         description-only edit"
    );
    let plain_rows = audit_events::Entity::find()
        .filter(audit_events::Column::Action.starts_with("custom_app.secret."))
        .filter(audit_events::Column::TargetId.eq(plain_name.as_str()))
        .all(&w.db)
        .await
        .expect("audit rows");
    assert!(
        plain_rows.is_empty(),
        "a plain project secret is no app's: {plain_rows:?}"
    );
}
