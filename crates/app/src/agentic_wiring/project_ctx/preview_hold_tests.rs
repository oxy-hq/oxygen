//! A context built for a workspace-preview request holds every write, and
//! keeps holding after the request's task is gone; a production context is
//! unchanged.

use agentic_automation::{HttpReview, WorkspaceContext};
use agentic_pipeline::platform::ProjectContext;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use oxy_test_utils::fixtures::TestFixture;

use super::OxyProjectContext;
use crate::server::previews::request_hold;

/// A ClickHouse on the discard port: a statement that is sent fails to
/// connect, one that is held never tries.
const CONFIG: &str = "models: []\ndatabases:\n  - name: warehouse\n    type: clickhouse\n    host: http://127.0.0.1:9\n";

async fn manager(fixture: &TestFixture) -> WorkspaceManager<WorkingCopy> {
    fixture
        .create_file("config.yml", CONFIG)
        .expect("write config.yml");
    WorkspaceBuilder::new(uuid::Uuid::new_v4())
        .with_working_copy(fixture.path(), None, oxy::config::OnMissing::Fail)
        .await
        .expect("config builder")
        .build()
        .await
        .expect("workspace manager")
}

async fn refusal(ctx: &OxyProjectContext) -> String {
    let conn = ctx
        .resolve_pre_built_connector("warehouse")
        .await
        .expect("every database is pre-built, held");
    conn.execute_query("DELETE FROM orders WHERE 1", 1)
        .await
        .expect_err("a write is refused")
        .to_string()
}

#[tokio::test]
async fn a_context_built_for_a_preview_request_holds_every_write() {
    let fixture = TestFixture::new().expect("tempdir");
    let wm = manager(&fixture).await;
    // Built inside the request, used outside it — as the spawned task that
    // drives a chat run uses it.
    let ctx = request_hold::scope(async { OxyProjectContext::new(wm) }).await;
    assert!(!request_hold::active());
    assert!(ctx.holds_writes());
    assert!(
        ctx.is_workspace_preview(),
        "no runner, no bridges, no Airway"
    );

    assert!(
        ctx.resolve_connector("warehouse").await.is_none(),
        "no bare config: the pipeline would build it unwrapped"
    );
    let err = refusal(&ctx).await;
    assert!(
        err.contains("`warehouse` cannot be written in a workspace preview"),
        "{err}"
    );
    let step = WorkspaceContext::get_connector(&ctx, "warehouse")
        .await
        .expect("a step's connector");
    let err = step
        .execute_query("INSERT INTO t SELECT 1", 1)
        .await
        .expect_err("a step's write is refused")
        .to_string();
    assert!(err.contains("cannot be written"), "{err}");

    assert!(matches!(
        ctx.review_http("POST", "https://hooks.example.test").await,
        HttpReview::Hold { .. }
    ));
    assert_eq!(
        ctx.review_http("GET", "https://api.example.test").await,
        HttpReview::Proceed
    );
    assert!(ctx.store_secret("TOKEN", "v").await.is_err());
    assert!(ctx.persist_secret("TOKEN", "v").await.is_err());
    assert!(
        ctx.resolve_pipeline_destination("warehouse", "raw_orders")
            .await
            .is_none()
    );
    assert!(ctx.anomaly_store().is_none());
    assert!(ctx.as_monitor_scan_port().is_none());
    assert!(ctx.compile_dispatcher().is_none());
    assert!(ctx.metric_tree_runner_system().is_none());
}

#[tokio::test]
async fn a_production_context_is_unchanged() {
    let fixture = TestFixture::new().expect("tempdir");
    let ctx = OxyProjectContext::new(manager(&fixture).await);
    assert!(!ctx.holds_writes());
    assert!(!ctx.is_workspace_preview());
    assert!(ctx.resolve_connector("warehouse").await.is_some());
    assert!(
        ctx.resolve_pre_built_connector("warehouse").await.is_none(),
        "pre-built stays Airhouse-only"
    );
    assert_eq!(
        ctx.review_http("POST", "https://hooks.example.test").await,
        HttpReview::Proceed
    );
    assert!(ctx.as_monitor_scan_port().is_some());
}
