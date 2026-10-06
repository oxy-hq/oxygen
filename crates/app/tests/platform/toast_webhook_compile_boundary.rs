//! Regression test for the Toast webhook's config source.
//!
//! `POST /api/webhooks/toast/orders` is mounted `RouteRole::FleetOk`, so it
//! runs on stateless `serve` replicas — which hold no
//! workspace working copy. It used to resolve `config.yml` from
//! `workspace.path` via `build_with_fallback_config`, and that helper
//! *silently* substitutes an empty `Config` when the file isn't there. The
//! empty config has no `toast` integration, so `resolve_toast` returned `None`
//! and the handler's fail-closed branch rejected every delivery with
//! `401 toast integration not configured for this workspace` — indistinguishable
//! from a customer who never set the integration up.
//!
//! In prod that meant 100% of Toast `order_updated` deliveries dropped at the
//! door (~2k/hour for pokehouse) and a permanently empty LIVE EVENTS panel.
//!
//! The fix reads the compile boundary first, exactly like `workspace_context`
//! does for every authenticated route. This test pins that: a promoted
//! workspace whose `path` points at a directory that does NOT exist (the
//! stateless-replica shape) must still resolve its Toast integration.
//!
//! Spins up a Postgres testcontainer (or reuses `OXY_DATABASE_URL` in CI) and
//! drives the real `load_toast_config` end-to-end.

use entity::workspaces::WorkspaceStatus;
use entity::{organizations, revisions, workspace_compiled_configs, workspaces};
use oxy_app::server::api::webhooks::toast::load_toast_config;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use serde_json::json;
use uuid::Uuid;

/// A path that cannot exist on the serving node — the whole point of the
/// stateless fleet. Any FS read of `config.yml` under here must miss.
const NONEXISTENT_WORKSPACE_PATH: &str = "/nonexistent/oxy-serve-replica/no-working-copy";

const WEBHOOK_SECRET_VAR: &str = "OXY_TEST_TOAST_WEBHOOK_SECRET";
const WEBHOOK_SECRET_VALUE: &str = "s3cr3t-from-the-workspace-secret-store";

/// Per-test database, migrated and wired so that `establish_connection()`
/// (used by the code under test) points at it.
///
/// The migration chain runs once per `cargo nextest run` into a template that
/// this clones; see `tests/common/mod.rs`.
async fn setup_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    // SAFETY: single-threaded test setup before any other env access. nextest
    // isolates each test in its own process, so pointing the process-wide
    // OnceCell at the per-test DB here is safe.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
    db
}

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("twcb-org".into()),
        slug: ActiveValue::Set(format!("twcb-{}", org_id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");
    org_id
}

/// Seed a workspace whose `path` points at a directory that does not exist —
/// the shape every `serve` replica sees. `compiled_integrations` is written to
/// `workspace_compiled_configs` and the revision promoted when `Some`; when
/// `None` the workspace has no promoted revision at all, so the compile
/// boundary misses and the FS fallback (which will also miss) takes over.
async fn seed_workspace(
    db: &DatabaseConnection,
    org_id: Uuid,
    compiled_integrations: Option<serde_json::Value>,
) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_id),
        name: ActiveValue::Set("twcb-ws".into()),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        path: ActiveValue::Set(Some(NONEXISTENT_WORKSPACE_PATH.into())),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        org_id: ActiveValue::Set(Some(org_id)),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");

    let Some(integrations) = compiled_integrations else {
        return ws_id;
    };

    let rev_id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        workspace_id: ActiveValue::Set(ws_id),
        git_sha: ActiveValue::Set("deadbeef".into()),
        branch: ActiveValue::Set(Some("main".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("full".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(1),
        file_count_compiled: ActiveValue::Set(1),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");

    workspace_compiled_configs::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        databases: ActiveValue::Set(json!([])),
        models: ActiveValue::Set(Some(json!([]))),
        integrations: ActiveValue::Set(Some(integrations)),
        repositories: ActiveValue::Set(None),
        builder_agent: ActiveValue::Set(None),
        mcp: ActiveValue::Set(None),
        other: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed compiled config");

    // Promote the revision (FK requires the revision to exist first).
    let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(ws_id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists")
        .into();
    ws.current_revision_id = ActiveValue::Set(Some(rev_id));
    ws.update(db).await.expect("promote revision");

    ws_id
}

/// The regression: a promoted workspace with a `toast` integration in its
/// COMPILED config resolves the signing secret even though there is no
/// `config.yml` anywhere on this node.
///
/// Every case lives in one test on purpose: `establish_connection()` memoises
/// `OXY_DATABASE_URL` per process, so a second `setup_db()` in the same process
/// would seed a database the pool never reopens. That holds under plain
/// `cargo test` (threads) as well as nextest (process per test).
#[tokio::test]
async fn compiled_config_resolves_toast_integration_without_a_working_copy() {
    let db = setup_db().await;
    let org_id = seed_org(&db).await;

    // The secret lives in the workspace secret store; the env fallback stands
    // in for it here. Either way it is NOT on the filesystem.
    // SAFETY: set before the resolver reads it, single-threaded.
    unsafe { std::env::set_var(WEBHOOK_SECRET_VAR, WEBHOOK_SECRET_VALUE) };

    let configured = seed_workspace(
        &db,
        org_id,
        Some(json!([{
            "name": "toast",
            "type": "toast",
            "webhook_secret_var": WEBHOOK_SECRET_VAR,
        }])),
    )
    .await;

    let resolved = load_toast_config(configured)
        .await
        .expect("load_toast_config must not error");

    // Before the fix this was `None` — the FS read missed, the fallback handed
    // back an empty Config, and the handler 401'd every Toast delivery.
    let (secret, allowlist) = resolved.expect(
        "toast integration must resolve from the compiled config on a node with no working copy",
    );
    assert_eq!(secret, WEBHOOK_SECRET_VALUE);
    assert!(
        allowlist.is_empty(),
        "no restaurant_guids configured => accept all, got {allowlist:?}"
    );

    // Control: a workspace with no promoted revision (and no working copy)
    // still resolves to `None`, so the handler keeps failing closed rather
    // than accepting unsigned payloads.
    let unpromoted = seed_workspace(&db, org_id, None).await;
    let resolved = load_toast_config(unpromoted)
        .await
        .expect("load_toast_config must not error for an unpromoted workspace");
    assert!(
        resolved.is_none(),
        "no compiled revision and no config.yml => unconfigured, got {resolved:?}"
    );

    // Control: a promoted workspace whose compiled config declares no
    // integrations is the genuine "customer never set Toast up" case that the
    // 401 is actually meant to describe.
    let no_integrations = seed_workspace(&db, org_id, Some(json!([]))).await;
    let resolved = load_toast_config(no_integrations)
        .await
        .expect("load_toast_config must not error for a workspace with no integrations");
    assert!(
        resolved.is_none(),
        "empty integrations => unconfigured, got {resolved:?}"
    );
}
