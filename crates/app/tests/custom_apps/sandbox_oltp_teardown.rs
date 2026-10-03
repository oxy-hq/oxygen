//! A sandbox's teardown drops its own OLTP schema on the org's staging branch
//! and that schema's ledger rows — under the sandbox's lock, and only that
//! sandbox's (`internal-docs/custom-app-sandboxes.md` → Lifecycle).
//!
//! - the schema and its ledger rows go; staging's schema, the other sandbox's
//!   and its ledger stay;
//! - a drop that cannot be confirmed — the branch is mid-reset — **fails the
//!   teardown and keeps the row**; once the branch is back the same run
//!   finishes;
//! - a sandbox whose row records no schema (no branch, never published to)
//!   connects to nothing.

use agentic_core::delegation::TaskSpec;
use entity::app_environments;
use oxy_app::server::api::custom_apps_migrations::{MigrationTarget, read_ledger};
use oxy_app::server::api::custom_apps_sandboxes::teardown::{self, SandboxTeardownTask};
use oxy_app::server::api::custom_apps_sandboxes::{TeardownReason, ops};
use oxy_oltp::OltpBranch::Staging;
use oxy_oltp::entity::branches::{self, BranchStatus};
use oxy_oltp::sandbox_schema::exists_on_branch;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait,
    QueryFilter, Statement,
};

use crate::sandbox_oltp_isolation::{
    functions, publish, run_in, run_oltp_tasks_done, schema_of, with_two_sandboxes,
};
use crate::sandbox_publish::sandbox;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

const ADD_TIER: &[u8] = b"alter table orders add column tier text;";
const IDS: [&str; 3] = ["ids", "query", "select id from orders order by id"];

/// Mark `dev-<handle>` deleting, as a `DELETE` does, and hand back the
/// teardown it queued.
async fn begin_delete(
    app: &OltpApp,
    row: &entity::apps::Model,
    handle: &str,
) -> SandboxTeardownTask {
    let run_id = ops::begin_delete(
        &app.t.db,
        row,
        &sandbox(handle),
        Some(app.t.guest_id),
        TeardownReason::Deleted,
    )
    .await
    .expect("mark the sandbox deleting");
    let queued = app
        .t
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec FROM agentic_task_queue WHERE task_id = $1",
            [run_id.into()],
        ))
        .await
        .expect("read the queue")
        .expect("the teardown is queued");
    let spec: serde_json::Value = queued.try_get("", "spec").expect("spec");
    let spec: TaskSpec = serde_json::from_value(spec).expect("a TaskSpec");
    SandboxTeardownTask::from_spec(&spec).expect("a teardown task")
}

async fn sandbox_row(
    app: &OltpApp,
    app_id: uuid::Uuid,
    handle: &str,
) -> Option<app_environments::Model> {
    app_environments::Entity::find_by_id((app_id, format!("dev-{handle}")))
        .one(&app.t.db)
        .await
        .expect("read the sandbox's row")
}

/// Put the org's staging branch row in `status`.
async fn set_branch_status(app: &OltpApp, status: BranchStatus) {
    let (_, row) = oxy_oltp::branches::find(&app.t.db, app.t.org_id, Staging)
        .await
        .expect("read the branch");
    let mut active: branches::ActiveModel = row.expect("the org's branch").into();
    active.status = ActiveValue::Set(status);
    active
        .update(&app.t.db)
        .await
        .expect("set the branch's status");
}

#[tokio::test]
async fn a_teardown_drops_that_sandboxs_oltp_schema_and_ledger_and_no_other() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        app.store.provision_branch().await;
        let row = with_two_sandboxes(&app).await;
        let (db, org) = (&app.t.db, app.t.org_id);
        let files: &[(&str, &[u8])] = &[("migrations/0001_tier.sql", ADD_TIER)];
        publish(&app, "a1", "a-1", files).await;
        publish(&app, "b2", "b-1", files).await;
        run_oltp_tasks_done(db, row.id).await;
        let (a1, b2) = (schema_of(&app, "a1"), schema_of(&app, "b2"));
        let ledger = |schema: &oxy_oltp::sandbox_schema::SandboxSchema| {
            let target = MigrationTarget::Schema(schema.name().to_string());
            async move {
                read_ledger(db, row.id, "oltp", &target)
                    .await
                    .expect("ledger")
            }
        };
        assert_eq!(ledger(&a1).await.len(), 1);

        let task = begin_delete(&app, &row, "a1").await;
        let summary = teardown::run(db, &task).await.expect("the teardown");
        assert!(summary.contains("OLTP schema dropped"), "{summary}");

        assert_eq!(
            exists_on_branch(db, org, Staging, &a1).await.expect("read"),
            Some(false)
        );
        assert!(ledger(&a1).await.is_empty(), "its ledger rows went with it");
        assert!(
            sandbox_row(&app, row.id, "a1").await.is_none(),
            "the name is free"
        );
        // The other sandbox, and staging, keep their schema, rows and ledger.
        assert_eq!(
            exists_on_branch(db, org, Staging, &b2).await.expect("read"),
            Some(true)
        );
        assert_eq!(ledger(&b2).await.len(), 1);
        let other = run_in(&app, "dev-b2", &[IDS]).await;
        assert_eq!(other["ids"]["ok"][0]["id"], 1, "{other}");
        let staging = run_in(&app, "staging", &[IDS]).await;
        assert_eq!(staging["ids"]["ok"][0]["id"], 1, "{staging}");
        assert_eq!(app.order_count().await, 1);
    })
    .await;
}

#[tokio::test]
async fn a_teardown_that_cannot_confirm_the_drop_fails_and_keeps_the_row() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        app.store.provision_branch().await;
        let row = with_two_sandboxes(&app).await;
        let (db, org) = (&app.t.db, app.t.org_id);
        publish(&app, "a1", "a-1", &[]).await;
        run_oltp_tasks_done(db, row.id).await;
        let a1 = schema_of(&app, "a1");
        let task = begin_delete(&app, &row, "a1").await;

        // Mid-reset the branch hands out no connection: the schema may or may
        // not be there, and nothing can say.
        set_branch_status(&app, BranchStatus::Resetting).await;
        let failed = teardown::run(db, &task)
            .await
            .expect_err("the drop is not confirmed");
        assert!(
            failed.contains("OLTP schema") && failed.contains("was not dropped"),
            "{failed}"
        );
        assert!(failed.contains("stays deleting"), "{failed}");
        let kept = sandbox_row(&app, row.id, "a1")
            .await
            .expect("the row is kept");
        assert!(kept.deleting_at.is_some() && kept.oltp_schema.is_some());

        // Once the branch is back, the same run finishes the job.
        set_branch_status(&app, BranchStatus::Active).await;
        assert_eq!(
            exists_on_branch(db, org, Staging, &a1).await.expect("read"),
            Some(true)
        );
        let summary = teardown::run(db, &task).await.expect("the teardown");
        assert!(summary.contains("OLTP schema dropped"), "{summary}");
        assert_eq!(
            exists_on_branch(db, org, Staging, &a1).await.expect("read"),
            Some(false)
        );
        assert!(sandbox_row(&app, row.id, "a1").await.is_none());
    })
    .await;
}

/// A state written by a newer build — one this build cannot read — still
/// says a schema exists: the teardown drops it rather than forget it.
#[tokio::test]
async fn a_state_this_build_cannot_read_still_gets_its_schema_dropped() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        app.store.provision_branch().await;
        let row = with_two_sandboxes(&app).await;
        let (db, org) = (&app.t.db, app.t.org_id);
        publish(&app, "a1", "a-1", &[]).await;
        run_oltp_tasks_done(db, row.id).await;
        let a1 = schema_of(&app, "a1");
        let mut newer: app_environments::ActiveModel = sandbox_row(&app, row.id, "a1")
            .await
            .expect("the row")
            .into();
        newer.oltp_schema = ActiveValue::Set(Some(serde_json::json!({ "v": 2, "kept": true })));
        newer.update(db).await.expect("write a newer build's state");

        let task = begin_delete(&app, &row, "a1").await;
        let summary = teardown::run(db, &task).await.expect("the teardown");
        assert!(summary.contains("OLTP schema dropped"), "{summary}");
        assert_eq!(
            exists_on_branch(db, org, Staging, &a1).await.expect("read"),
            Some(false)
        );
    })
    .await;
}

/// No branch, so no publish ever queued a schema: the row records none, and
/// the teardown says so without connecting to anything.
#[tokio::test]
async fn a_sandbox_with_no_recorded_schema_has_nothing_to_drop() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = with_two_sandboxes(&app).await;
        publish(&app, "a1", "a-1", &[]).await;
        let task = begin_delete(&app, &row, "a1").await;
        let summary = teardown::run(&app.t.db, &task).await.expect("the teardown");
        assert!(summary.ends_with("no OLTP schema"), "{summary}");
        assert!(sandbox_row(&app, row.id, "a1").await.is_none());
        let kept = app_environments::Entity::find()
            .filter(app_environments::Column::AppId.eq(row.id))
            .filter(app_environments::Column::Name.eq("dev-b2"))
            .one(&app.t.db)
            .await
            .expect("read");
        assert!(kept.is_some(), "the other sandbox is untouched");
    })
    .await;
}
