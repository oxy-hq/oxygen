//! The tenant's schedules API never sees a custom app's non-production
//! function schedule (environments design §4.5, ruling 1): `list` leaves it
//! out, every by-id route answers `404`, and create refuses a ref naming an
//! environment. Production's row is the tenant's as before.
//!
//! Rehomed from `oxy-api-custom-apps`' `env_schedules_tenant` when the
//! handlers left `oxy-app` (2026-11-15): S3 forbids that crate depending on
//! this one even for tests, and its fixture (a published app whose sandbox's
//! schedule switch is turned on) is several modules of that crate's own test
//! tree. What the handlers decide is the ref's shape alone
//! (`FunctionScheduleRef::environment`), so the two rows are inserted with the
//! scheduler directly: production's `<app>/tick` and the sandbox's
//! `<app>/dev-a1/tick`, the refs the switch writes. That the switch writes
//! those refs stays `oxy-api-custom-apps`' to prove (`env_schedules`).

use std::sync::Arc;

use agentic_pipeline::platform::PlatformContext;
use agentic_pipeline::scheduler::{BackfillRequest, ScheduleInput, create_schedule};
use axum::Json;
use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use entity::users::UserStatus;
use entity::workspace_members::WorkspaceRole;
use oxy_api_automations::schedules;
use oxy_app::surface::role_guards::WorkspaceAdmin;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use serde_json::Value;
use uuid::Uuid;

use crate::common::{Schema, demo_workspace_id, test_db_with};

/// Every run in this binary is threadless.
struct NoThreads;

#[async_trait::async_trait]
impl agentic_pipeline::platform::ThreadOwnerLookup for NoThreads {
    async fn thread_owner(&self, _thread: Uuid) -> Result<Option<Option<Uuid>>, String> {
        Ok(None)
    }
}

async fn answer(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn input(target_ref: String) -> ScheduleInput {
    ScheduleInput {
        name: "mine".into(),
        target_kind: "function".into(),
        target_ref,
        question: None,
        variables: None,
        cron_expr: "0 * * * *".into(),
        timezone: "UTC".into(),
        enabled: true,
    }
}

/// The driver's `PlatformContext` run-now and backfill take, over an empty
/// workspace. Neither reaches it here: an environment's row is refused first.
async fn platform() -> (Arc<dyn PlatformContext>, tempfile::TempDir) {
    use oxy::adapters::workspace::builder::WorkspaceBuilder;
    use oxy::config::OnMissing;
    let root = tempfile::tempdir().expect("platform dir");
    std::fs::write(
        root.path().join("config.yml"),
        "databases: []\nmodels: []\n",
    )
    .expect("write config.yml");
    let manager = WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(root.path(), None, OnMissing::Fail)
        .await
        .expect("config.yml loads")
        .build()
        .await
        .expect("workspace manager");
    (
        Arc::new(oxy_app::surface::workspace_runtime::OxyProjectContext::new(
            manager,
        )),
        root,
    )
}

/// The tenant's schedules API never sees an environment's row: the list
/// leaves it out, every by-id route answers `404`, and create refuses a ref
/// naming an environment. Production's row is the tenant's as before.
#[tokio::test]
async fn the_tenant_schedules_api_never_sees_an_environments_row() {
    let db = test_db_with(Schema::All).await;
    let ws = demo_workspace_id();
    let app = Uuid::new_v4();
    let production_ref = format!("{app}/tick");
    let sandbox_ref = format!("{app}/dev-a1/tick");
    create_schedule(&db, ws, input(production_ref.clone()))
        .await
        .expect("production's row");
    let id = create_schedule(&db, ws, input(sandbox_ref.clone()))
        .await
        .expect("the sandbox's row")
        .id;

    let state = Extension(Arc::new(agentic_http::AgenticState::new(
        tokio_util::sync::CancellationToken::new(),
        db.clone(),
        Arc::new(NoThreads),
    )));
    let (platform, _dir) = platform().await;
    let user = || {
        AuthenticatedUserExtractor(AuthenticatedUser {
            id: Uuid::new_v4(),
            email: Some("tenant-admin@customer.example".into()),
            name: "Tenant".into(),
            picture: None,
            status: UserStatus::Active,
            credential: None,
        })
    };
    let admin = || WorkspaceAdmin(WorkspaceRole::Admin);

    let (status, listed) = answer(schedules::list(state.clone(), user(), Path(ws)).await).await;
    assert_eq!(status, StatusCode::OK);
    let refs: Vec<&str> = listed
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|s| s["target_ref"].as_str())
        .collect();
    assert!(
        refs.contains(&production_ref.as_str()),
        "production's: {refs:?}"
    );
    assert!(!refs.contains(&sandbox_ref.as_str()), "never the sandbox's");

    let by_id = || Path((ws, id.clone()));
    let not_found = [
        schedules::get(state.clone(), user(), by_id()).await,
        schedules::update(
            admin(),
            state.clone(),
            user(),
            by_id(),
            Json(input(production_ref.clone())),
        )
        .await,
        schedules::delete(admin(), state.clone(), user(), by_id()).await,
        schedules::run_now(
            admin(),
            state.clone(),
            Extension(platform.clone()),
            user(),
            by_id(),
        )
        .await,
        schedules::backfill(
            admin(),
            state.clone(),
            Extension(platform.clone()),
            user(),
            by_id(),
            Json(BackfillRequest {
                from: chrono::Utc::now() - chrono::Duration::hours(3),
                to: chrono::Utc::now(),
                concurrency: None,
            }),
        )
        .await,
    ];
    for response in not_found {
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let created = schedules::create(
        admin(),
        state.clone(),
        user(),
        Path(ws),
        Json(input(sandbox_ref.clone())),
    )
    .await;
    assert_eq!(
        created.status(),
        StatusCode::BAD_REQUEST,
        "a ref naming an environment"
    );

    let mut rows: Vec<String> = agentic_pipeline::scheduler::list_schedules(&db, ws)
        .await
        .expect("list")
        .into_iter()
        .map(|s| s.target_ref)
        .collect();
    rows.sort();
    let mut want = vec![sandbox_ref, production_ref];
    want.sort();
    assert_eq!(
        rows, want,
        "nothing the tenant sent touched the sandbox's row"
    );
}
