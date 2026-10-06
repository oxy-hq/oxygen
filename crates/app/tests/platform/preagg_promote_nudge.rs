//! A promote that re-hashes a rollup must enqueue a pre-aggregation cycle.
//!
//! The defect this pins: adding a measure to a rollup's `measures:` list, then
//! compiling with `promote=true`, produced a new rollup hash with no built
//! artifact — correctly read as stale — and then built nothing for six
//! minutes. `reconcile_preagg_schedule` runs on every promoted compile but
//! preserves `next_run_at` unless the cron expression changed, so the promote
//! never kicked a cycle, and with no `heartbeat:` configured the interval is
//! 600s. From the outside it was indistinguishable from pre-aggregation being
//! broken; a server restart built it immediately.
//!
//! Drives the real `nudge_after_promote` against a real Postgres, so the
//! assertions are about a row in `agentic_task_queue`, not about the set
//! arithmetic (`preagg_promote`'s own unit tests cover that).

use entity::workspaces::WorkspaceStatus;
use entity::{organizations, revisions, semantic_views, workspaces};
use oxy_app::server::preagg_promote::nudge_after_promote;
use sea_orm::{
    ActiveModelTrait, ActiveValue, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult,
    Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// The rollup from the report: two measures over one dimension, on a 6h key.
/// `measures` is the list the edit changes.
fn orders_view(measures: &[&str]) -> Value {
    json!({
        "name": "orders",
        "datasource": "local",
        "table": "orders.csv",
        "refresh_key": { "every": "6h" },
        "dimensions": [
            { "name": "order_status", "type": "string", "expr": "status" },
            { "name": "order_date", "type": "date", "expr": "created_at" },
        ],
        "measures": [
            { "name": "total_orders", "type": "count" },
            { "name": "total_order_value", "type": "sum", "expr": "amount" },
        ],
        "pre_aggregations": [{
            "name": "orders_by_month",
            "dimensions": ["order_status"],
            "measures": measures,
            "time_dimension": "order_date",
            "granularity": "month",
        }],
    })
}

/// An org + a workspace with nothing promoted yet.
async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("preagg-org".into()),
        slug: ActiveValue::Set(format!("preagg-{}", org_id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");

    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_id),
        name: ActiveValue::Set("preagg-ws".into()),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        path: ActiveValue::Set(None),
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
    ws_id
}

/// Compile one revision carrying `definition` as the workspace's only
/// `.view.yml`, and promote it. Returns the revision id.
async fn promote_revision(db: &DatabaseConnection, ws_id: Uuid, definition: Value) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let rev_id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        workspace_id: ActiveValue::Set(ws_id),
        git_sha: ActiveValue::Set(format!("sha-{}", rev_id.simple())),
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

    semantic_views::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        name: ActiveValue::Set("orders".into()),
        file_path: ActiveValue::Set("semantics/views/orders.view.yml".into()),
        definition: ActiveValue::Set(definition),
        compiled_sql_blob_key: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed semantic view");

    let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(ws_id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists")
        .into();
    ws.current_revision_id = ActiveValue::Set(Some(rev_id));
    ws.update(db).await.expect("promote revision");
    rev_id
}

/// Queued `preagg_cycle` tasks for a workspace, with the two payload fields
/// that say whether this was a tick or a Rebuild click.
#[derive(FromQueryResult)]
struct QueuedCycle {
    force: bool,
    target: Option<String>,
}

async fn queued_cycles(db: &DatabaseConnection, ws_id: Uuid) -> Vec<QueuedCycle> {
    QueuedCycle::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT (spec->'payload'->>'force')::bool AS force, \
                spec->'payload'->>'target' AS target \
         FROM agentic_task_queue \
         WHERE spec->>'kind' = 'preagg_cycle' \
           AND spec->'payload'->>'workspace_id' = $1 \
         ORDER BY created_at",
        [ws_id.to_string().into()],
    ))
    .all(db)
    .await
    .expect("read queued cycles")
}

/// Every queued `preagg_cycle` task, workspace-agnostic.
async fn queued_cycle_count(db: &DatabaseConnection) -> i64 {
    #[derive(FromQueryResult)]
    struct Count {
        c: i64,
    }
    Count::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT count(*)::int8 AS c FROM agentic_task_queue \
         WHERE spec->>'kind' = 'preagg_cycle'",
    ))
    .one(db)
    .await
    .expect("count queued cycles")
    .expect("count returns a row")
    .c
}

async fn setup() -> DatabaseConnection {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    db
}

/// The reported case, end to end: a rollup gains a measure, the edit is
/// promoted, and the cycle is queued at once rather than on the next heartbeat.
#[tokio::test]
async fn adding_a_measure_and_promoting_enqueues_a_cycle() {
    let db = setup().await;
    let ws = seed_workspace(&db).await;

    let first = promote_revision(&db, ws, orders_view(&["total_orders"])).await;
    // Nothing queued for the first promote in this test's framing — the
    // baseline IS that revision, which is what the compile worker captures
    // before the compile that follows.
    let _ = promote_revision(&db, ws, orders_view(&["total_orders", "total_order_value"])).await;

    nudge_after_promote(&db, ws, Some(first), true).await;

    let cycles = queued_cycles(&db, ws).await;
    assert_eq!(cycles.len(), 1, "the promote owed exactly one cycle");
    assert!(
        !cycles[0].force,
        "the new hash is genuinely stale; a tick is what was missing, not an override"
    );
    assert!(
        cycles[0].target.is_none(),
        "one promote can re-hash rollups across several views"
    );
}

/// The steady-state path, which runs on every promoted compile in the fleet: a
/// promote that changed something other than a rollup must not queue anything.
/// Without this the nudge degenerates into a second, uncapped cadence.
#[tokio::test]
async fn a_promote_that_leaves_the_rollups_alone_queues_nothing() {
    let db = setup().await;
    let ws = seed_workspace(&db).await;

    let first = promote_revision(&db, ws, orders_view(&["total_orders"])).await;
    let _ = promote_revision(&db, ws, orders_view(&["total_orders"])).await;

    nudge_after_promote(&db, ws, Some(first), true).await;

    assert!(
        queued_cycles(&db, ws).await.is_empty(),
        "identical rollup hashes are not new work"
    );
}

/// A workspace's very first promoted revision has no baseline, so everything
/// it declares is new — and that is the case where waiting out a heartbeat is
/// least defensible, because nothing has ever been built.
#[tokio::test]
async fn a_first_promote_enqueues_a_cycle() {
    let db = setup().await;
    let ws = seed_workspace(&db).await;
    promote_revision(&db, ws, orders_view(&["total_orders"])).await;

    nudge_after_promote(&db, ws, None, true).await;

    assert_eq!(queued_cycles(&db, ws).await.len(), 1);
}

/// Pre-aggregation is opt-in: no `pre_aggregations:` block means no cycle is
/// scheduled at all, and the nudge must not be a way around that. The opt-in
/// is resolved once, by the reconcile, and passed in.
#[tokio::test]
async fn a_workspace_with_pre_aggregation_disabled_is_not_nudged() {
    let db = setup().await;
    let ws = seed_workspace(&db).await;

    let first = promote_revision(&db, ws, orders_view(&["total_orders"])).await;
    let _ = promote_revision(&db, ws, orders_view(&["total_orders", "total_order_value"])).await;

    nudge_after_promote(&db, ws, Some(first), false).await;

    assert!(
        queued_cycles(&db, ws).await.is_empty(),
        "an opted-out workspace has no cycle to tick"
    );
}

/// A `promote: true` compile that lost the causality race leaves
/// `current_revision_id` where it was. The served definitions are the ones the
/// last cycle already saw, so there is nothing to tick — and re-reading the
/// pointer rather than trusting the compile's own `Promotion` is what makes
/// that true whichever revision won.
#[tokio::test]
async fn a_promote_that_did_not_move_the_pointer_queues_nothing() {
    let db = setup().await;
    let ws = seed_workspace(&db).await;
    let current = promote_revision(&db, ws, orders_view(&["total_orders"])).await;

    nudge_after_promote(&db, ws, Some(current), true).await;

    assert!(queued_cycles(&db, ws).await.is_empty());
}

/// A revision that declares no rollups at all cannot owe a cycle, whatever the
/// baseline was. Guarded separately because the promoted set is checked for
/// emptiness before the baseline is read — one query, not two, on the path
/// every non-semantic workspace takes.
#[tokio::test]
async fn a_revision_declaring_no_rollups_queues_nothing() {
    let db = setup().await;
    let ws = seed_workspace(&db).await;

    let first = promote_revision(&db, ws, orders_view(&["total_orders"])).await;
    let _ = promote_revision(
        &db,
        ws,
        json!({ "name": "orders", "datasource": "local", "table": "orders.csv" }),
    )
    .await;

    nudge_after_promote(&db, ws, Some(first), true).await;

    assert!(queued_cycles(&db, ws).await.is_empty());
    // And nothing was queued for anyone else either: the removed rollup is
    // still nobody's rebuild, because its artifacts need a retraction and no
    // cycle tick reaches one.
    assert_eq!(queued_cycle_count(&db).await, 0);
}
