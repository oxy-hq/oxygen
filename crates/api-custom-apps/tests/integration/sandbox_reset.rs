//! `POST …/environments/{name}/reset` for the planes that have a reset
//! (`internal-docs/2026-11-02-custom-app-branches-p1-plan.md`, B8): `storage`
//! empties the sandbox's silo, `secrets` deletes its own secret path, and
//! `messages` deletes its `_outbox/` — each and nothing else, answering
//! `{"plane", "status": "done"}`. Storage and secrets are a sandbox's alone:
//! on staging, whose files and secrets the team shares, they answer
//! `409 plane_not_resettable`; staging's outbox resets like a sandbox's.
//! Every other plane still refuses (`sandbox_planes_routes`). Reached through
//! the console's routes, and — for the door — a sandbox agent token.

use axum::http::StatusCode;
use entity::apps;
use oxy::service::secret_manager::SecretManagerService;
use oxy_api_custom_apps::custom_apps_sandboxes::lock::SandboxLock;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};

use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};
use crate::sandbox_agent_token::fixture::agent;
use crate::sandbox_routes::send;
use crate::sandbox_teardown::sandbox;
use crate::sandbox_teardown_task::{
    furnished_sandbox, objects, published, put_object, secret, use_scratch_homes,
};

/// A published app with a furnished sandbox `dev-a1` (one object, one secret,
/// `TOKEN`), the guest made Oxy staff, and the console reachable.
async fn furnished(t: &Tenant) -> apps::Model {
    let (app, build) = published(t, "sbx-reset").await;
    furnished_sandbox(t, &app, build, "a1").await;
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OXY_OWNER", LOCAL_GUEST_EMAIL);
        std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com");
    }
    app
}

async fn reset(app: &apps::Model, name: &str, planes: Value) -> (StatusCode, Value) {
    send(
        "POST",
        &format!("{}/environments/{name}/reset", app.id),
        Some(json!({ "planes": planes })),
    )
    .await
}

async fn set_secret(t: &Tenant, app: &apps::Model, environment: Option<&str>, value: &str) {
    SecretManagerService::new(app.project_id)
        .set_app_secret_in(&t.db, app.id, environment, "TOKEN", value, t.guest_id)
        .await
        .expect("set a secret");
}

async fn production_secret(app: &apps::Model) -> Option<String> {
    let manager = SecretManagerService::new(app.project_id);
    manager.clear_cache().await;
    manager.get_secret(&format!("apps/{}/TOKEN", app.id)).await
}

fn done(plane: &str) -> Value {
    json!({ "plane": plane, "status": "done", "run_id": null })
}

/// The silo is emptied; the secret, the neighbour sandbox, staging and
/// production are not.
#[tokio::test]
async fn a_storage_reset_empties_the_silo_and_keeps_the_secrets() {
    let _tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let app = furnished(&t).await;
    put_object(app.id, &sandbox("a1"), "_outbox/m.json").await;
    furnished_sandbox(&t, &app, published_build(&t, &app).await, "b2").await;
    put_object(app.id, &AppEnvironment::Production, "uploads/prod.bin").await;
    assert_eq!(objects(app.id, &sandbox("a1")).await, 2);

    let (status, answer) = reset(&app, "dev-a1", json!(["storage"])).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer,
        json!({ "name": "dev-a1", "planes": [done("storage")] })
    );

    assert_eq!(
        objects(app.id, &sandbox("a1")).await,
        0,
        "the silo is empty"
    );
    assert_eq!(secret(&app, "dev-a1").await.as_deref(), Some("a1"));
    assert_eq!(objects(app.id, &sandbox("b2")).await, 1);
    assert_eq!(objects(app.id, &AppEnvironment::Production).await, 1);
}

/// The sandbox's own secret path goes; staging's and production's keys of the
/// same name, and the sandbox's files, stay.
#[tokio::test]
async fn a_secrets_reset_deletes_the_environments_own_path_only() {
    let _tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let app = furnished(&t).await;
    set_secret(&t, &app, Some("staging"), "stg").await;
    set_secret(&t, &app, None, "prod").await;

    let (status, answer) = reset(&app, "dev-a1", json!(["secrets"])).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer,
        json!({ "name": "dev-a1", "planes": [done("secrets")] })
    );

    assert_eq!(secret(&app, "dev-a1").await, None);
    assert_eq!(secret(&app, "staging").await.as_deref(), Some("stg"));
    assert_eq!(production_secret(&app).await.as_deref(), Some("prod"));
    assert_eq!(objects(app.id, &sandbox("a1")).await, 1, "files stay");
}

/// `_outbox/` goes, on a sandbox and on staging; every other object stays.
#[tokio::test]
async fn a_messages_reset_deletes_the_outbox_alone_on_a_sandbox_and_on_staging() {
    let _tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let app = furnished(&t).await;
    let staging = AppEnvironment::Staging;
    for environment in [sandbox("a1"), staging.clone()] {
        put_object(app.id, &environment, "_outbox/m1.json").await;
        put_object(app.id, &environment, "_outbox/m2.json").await;
        put_object(app.id, &environment, "kept/file.bin").await;
    }
    assert_eq!(objects(app.id, &sandbox("a1")).await, 4);

    for name in ["dev-a1", "staging"] {
        let (status, answer) = reset(&app, name, json!(["messages"])).await;
        assert_eq!(status, StatusCode::OK, "{name}: {answer}");
        assert_eq!(answer["planes"], json!([done("messages")]), "{name}");
    }
    assert_eq!(
        objects(app.id, &sandbox("a1")).await,
        2,
        "uploads/a1.bin and kept/file.bin"
    );
    assert_eq!(objects(app.id, &staging).await, 1, "kept/file.bin");
    assert_eq!(secret(&app, "dev-a1").await.as_deref(), Some("a1"));
}

/// Staging's files and secrets are the team's: both answer 409 with the
/// reason, and nothing of staging's is deleted.
#[tokio::test]
async fn staging_storage_and_secrets_answer_409_and_delete_nothing() {
    let _tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let app = furnished(&t).await;
    set_secret(&t, &app, Some("staging"), "stg").await;
    put_object(app.id, &AppEnvironment::Staging, "uploads/team.bin").await;

    for plane in ["storage", "secrets"] {
        let (status, refused) = reset(&app, "staging", json!([plane])).await;
        assert_eq!(status, StatusCode::CONFLICT, "{plane}: {refused}");
        assert_eq!(refused["error"], "plane_not_resettable", "{plane}");
        let words = refused["message"].as_str().unwrap_or_default();
        assert!(words.contains("shared by the team"), "{plane}: {words}");
    }
    assert_eq!(objects(app.id, &AppEnvironment::Staging).await, 1);
    assert_eq!(secret(&app, "staging").await.as_deref(), Some("stg"));
}

/// Planes are walked in the order asked; one that cannot be reset stops the
/// walk, and those before it stay reset.
#[tokio::test]
async fn a_walk_resets_in_order_and_stops_at_the_first_refusal() {
    let _tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let app = furnished(&t).await;

    let (status, answer) = reset(&app, "dev-a1", json!(["secrets", "storage"])).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["planes"], json!([done("secrets"), done("storage")]));
    assert_eq!(secret(&app, "dev-a1").await, None);
    assert_eq!(objects(app.id, &sandbox("a1")).await, 0);

    put_object(app.id, &sandbox("a1"), "uploads/again.bin").await;
    let (status, refused) = reset(&app, "dev-a1", json!(["messages", "oltp", "storage"])).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "plane_not_resettable");
    assert_eq!(
        objects(app.id, &sandbox("a1")).await,
        1,
        "storage was never reached"
    );
}

/// A sandbox another teardown, migration or reset holds is left alone: `409
/// environment_busy`, nothing deleted.
#[tokio::test]
async fn a_busy_sandbox_is_not_reset() {
    let _tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let app = furnished(&t).await;
    let held = SandboxLock::try_acquire(&t.db, app.id, "dev-a1")
        .await
        .expect("take the lock")
        .expect("nobody holds it");

    let (status, refused) = reset(&app, "dev-a1", json!(["storage"])).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "environment_busy");
    assert_eq!(objects(app.id, &sandbox("a1")).await, 1);

    held.release().await;
    let (status, answer) = reset(&app, "dev-a1", json!(["storage"])).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
}

/// A sandbox that is being torn down is not reset.
#[tokio::test]
async fn a_sandbox_being_deleted_is_not_reset() {
    let _tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let app = furnished(&t).await;
    t.db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE app_environments SET deleting_at = now() WHERE app_id = $1 AND name = 'dev-a1'",
        [app.id.into()],
    ))
    .await
    .expect("mark deleting");
    let (status, refused) = reset(&app, "dev-a1", json!(["storage"])).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(objects(app.id, &sandbox("a1")).await, 1);
}

/// A sandbox agent token resets the sandbox it created, and no other: a
/// colleague's sandbox and staging answer `404`, deleting nothing.
#[tokio::test]
async fn a_token_resets_only_a_sandbox_it_created() {
    let _tmp = use_scratch_homes();
    let agent = agent().await;
    let (status, created) = agent.create("dev-a").await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    crate::app_environments::seed_sandbox(&agent.t.db, agent.app.id, "dev-b", agent.t.guest_id)
        .await;
    for name in ["dev-a", "dev-b"] {
        put_object(
            agent.app.id,
            &AppEnvironment::parse(name).unwrap(),
            "uploads/x.bin",
        )
        .await;
    }
    let uri = |name: &str| format!("/customer-apps/{}/environments/{name}/reset", agent.app.id);

    let (status, answer) = agent
        .api(
            "POST",
            &uri("dev-a"),
            Some(json!({ "planes": ["storage"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(objects(agent.app.id, &sandbox("a")).await, 0);

    for name in ["dev-b", "staging"] {
        let (status, refused) = agent
            .api("POST", &uri(name), Some(json!({ "planes": ["messages"] })))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}: {refused}");
    }
    assert_eq!(objects(agent.app.id, &sandbox("b")).await, 1);
}

async fn published_build(t: &Tenant, app: &apps::Model) -> uuid::Uuid {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    entity::app_builds::Entity::find()
        .filter(entity::app_builds::Column::AppId.eq(app.id))
        .one(&t.db)
        .await
        .expect("read the build")
        .expect("a build")
        .id
}
