//! A sample platform's secrets (fix round 1): on a rotate-on-use (QuickBooks)
//! sample only the registered sandbox's names resolve — so no app-scoped
//! production token (`apps/<id>/…`, Pokehouse's `refresh-qb-token` grant) can
//! be read or rotated — a sandbox naming one never builds a platform, and the
//! sandbox's rotated token is updated in place, never created.

use agentic_automation::WorkspaceContext;
use agentic_pipeline::platform::ProjectContext;
use oxy::service::secret_manager::CreateSecretParams;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseBackend, Set, Statement};
use serde_json::json;
use uuid::Uuid;

use super::fixture::exec;
use super::sample::{DSN, airhouse, sample_row_of};
use super::*;
use crate::agentic_wiring::preview_airhouse::PipelineWriter;
use crate::server::service::secret_manager::SecretManagerService;
use crate::server::test_support::{SKIP_MSG, test_db};

const APP_TOKEN: &str = "apps/5b7e0c55-1f2a-4f7e-9d1c-7a0b1c2d3e4f/QB_REFRESH_TOKEN_EASTBAY";

async fn register(db: &sea_orm::DatabaseConnection, ws: Uuid, overrides: serde_json::Value) {
    exec(
        db,
        "INSERT INTO workspace_preview_sources (workspace_id, pipeline_name, environment, overrides) \
         VALUES ($1, 'qb', 'sandbox', $2)",
        vec![ws.into(), overrides.into()],
    )
    .await;
}

fn sandbox(refresh: &str) -> serde_json::Value {
    json!({ "realm_id": "4620816365000000", "refresh_token_var": refresh,
            "client_secret_var": "S11_SANDBOX_SECRET" })
}

async fn platform(
    db: &sea_orm::DatabaseConnection,
    row: &entity::workspace_preview_runs::Model,
) -> Result<PreviewPlatformContext, PreviewError> {
    PreviewPlatformContext::new_with(db, row, airhouse(PipelineWriter::Dsn(DSN.into()))).await
}

/// BLOCKING (run time): the allowlist. An app-scoped production token and an
/// ordinary workspace secret are both set; on a QuickBooks sample neither
/// resolves (or persists) — only the sandbox's own names do. The control: a
/// sample of a source that does not rotate resolves the ordinary one.
#[tokio::test]
async fn a_rotate_on_use_sample_resolves_only_its_sandbox_vars() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("S11_SANDBOX_REFRESH", "sandbox-token");
        std::env::set_var(APP_TOKEN, "production-app-token");
        std::env::set_var("S11_ORDINARY", "ordinary");
    }
    let row = sample_row_of(&db, "quickbooks").await;
    register(&db, row.workspace_id, sandbox("S11_SANDBOX_REFRESH")).await;
    let ctx = platform(&db, &row).await.expect("platform");
    assert_eq!(
        ProjectContext::resolve_secret(&ctx, "S11_SANDBOX_REFRESH")
            .await
            .as_deref(),
        Some("sandbox-token")
    );
    for outside in [APP_TOKEN, "S11_ORDINARY"] {
        assert_eq!(
            ProjectContext::resolve_secret(&ctx, outside).await,
            None,
            "{outside}"
        );
        assert_eq!(ctx.fetch_secret(outside).await, None, "{outside}");
        assert!(ctx.persist_secret(outside, "x").await.is_err(), "{outside}");
    }

    let other = sample_row_of(&db, "rest_api").await;
    let ctx = platform(&db, &other).await.expect("platform");
    assert_eq!(
        ProjectContext::resolve_secret(&ctx, "S11_ORDINARY")
            .await
            .as_deref(),
        Some("ordinary"),
        "the control: only a rotate-on-use sample is allowlisted"
    );
}

/// BLOCKING (run time): a sources row that names an app-scoped rotator — as it
/// could only have been written behind the save — never builds a platform.
#[tokio::test]
async fn an_app_scoped_rotator_behind_the_save_never_builds_a_platform() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let row = sample_row_of(&db, "quickbooks").await;
    register(&db, row.workspace_id, sandbox(APP_TOKEN)).await;
    match platform(&db, &row).await {
        Err(PreviewError::Unusable(why)) => assert!(why.contains("reserved"), "{why}"),
        Err(other) => panic!("unusable, not {other}"),
        Ok(_) => panic!("a sandbox naming an app-scoped token must never run"),
    }
}

async fn seed_user(db: &sea_orm::DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    entity::users::ActiveModel {
        id: Set(id),
        email: Set(Some(format!("{id}@staff.test"))),
        name: Set("staff".into()),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

async fn stored(db: &sea_orm::DatabaseConnection, ws: Uuid, name: &str) -> Vec<Option<Uuid>> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT updated_by FROM secrets WHERE project_id = $1 AND name = $2",
        [ws.into(), name.into()],
    ))
    .await
    .unwrap()
    .iter()
    .map(|r| r.try_get("", "updated_by").unwrap())
    .collect()
}

/// SHOULD-FIX 2: the rotated sandbox token is an update of the stored secret,
/// attributed to the staffer who started the sample; an absent one is an
/// error, and nothing is created.
#[tokio::test]
async fn the_sandbox_token_is_updated_in_place_never_created() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let staff = seed_user(&db).await;
    let mut row = sample_row_of(&db, "quickbooks").await;
    row.requested_by = Some(staff);
    let ws = row.workspace_id;
    let var = format!("S11_SANDBOX_REFRESH_{}", Uuid::new_v4().simple());
    let params = CreateSecretParams {
        name: var.clone(),
        value: "old".into(),
        description: None,
        created_by: staff,
    };
    SecretManagerService::new(ws)
        .create_secret(&db, params)
        .await
        .expect("store the sandbox token");
    register(&db, ws, sandbox(&var)).await;
    let ctx = platform(&db, &row).await.expect("platform");
    ctx.persist_secret(&var, "rotated")
        .await
        .expect("updated in place");
    assert_eq!(stored(&db, ws, &var).await, vec![Some(staff)]);

    let absent = format!("S11_SANDBOX_ABSENT_{}", Uuid::new_v4().simple());
    let mut row = sample_row_of(&db, "quickbooks").await;
    row.requested_by = Some(staff);
    register(&db, row.workspace_id, sandbox(&absent)).await;
    let ctx = platform(&db, &row).await.expect("platform");
    let err = ctx.persist_secret(&absent, "rotated").await.unwrap_err();
    assert!(err.contains("never creates"), "{err}");
    assert!(
        stored(&db, row.workspace_id, &absent).await.is_empty(),
        "nothing created"
    );
}
