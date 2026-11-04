//! A sandbox's own OLTP branch, cut by the `custom_app_env_oltp_branch` task
//! (`custom_apps_env_branch::task`): the real provisioner (`LocalProvider`, as
//! `OXY_OLTP_PROVIDER=local` selects it) copies the tenant's database into a
//! sibling on the test cluster, and the real migration apply runs on it.
//!
//! - **a cut with a build** applies the build's OLTP files under the branch's
//!   own ledger target, `branch:<id>`, and is `ready`; the same task, queued
//!   and run by the executor the fleet registers, does the same;
//! - **a cut with no build** is `ready` with nothing applied;
//! - **a task for a deleted sandbox** does nothing;
//! - **a cut whose migration fails** is `failed` with the reason, never `ready`;
//! - **a migrate** applies the build the sandbox serves and nothing else;
//! - **`serve`**: `ctx.oltp` on the branch; **`publish_migrates`**: what a
//!   publish to a branch-mode sandbox queues; **`planes`**: what a create, a
//!   change of planes and a reset queue; **`teardown`**: the branch goes
//!   with its sandbox, and a sandbox 30 days past its fork goes with it.

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_runtime::worker::TaskExecutor;
use chrono::Utc;
use entity::{app_environments, apps};
use oxy_api_custom_apps::custom_apps_env_branch::task::{
    BranchOp, ENV_OLTP_BRANCH_KIND, EnvBranchExecutor, EnvBranchTask, run,
};
use oxy_api_custom_apps::custom_apps_migrations::{MigrationTarget, read_ledger};
use oxy_oltp::entity::env_branches::EnvBranchStatus;
use oxy_oltp::env_branches::find_for_app;
use oxy_oltp::resolver::{EnvBranchAccept, resolve_env_branch_writer_for_org};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

mod cut_and_reset;
mod diff;
mod lifecycle;
mod planes;
mod planes_reset;
mod publish_migrates;
mod serve;
mod teardown;

use crate::sandbox_oltp_isolation::{publish, with_two_sandboxes};
use crate::sandbox_publish::build_pk;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

const ADD_TIER: &[u8] = b"alter table orders add column tier text default 'gold';";
const BROKEN: &[u8] = b"create table broken (;";
const HANDLE: &str = "a1";
const ENVIRONMENT: &str = "dev-a1";

/// The provisioner `custom_apps_env_branch::provisioner` builds reads the
/// process environment; nextest gives each test its own process.
async fn point_at_the_test_cluster() {
    let admin = crate::common::admin_url().await;
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OXY_OLTP_PROVIDER", "local");
        std::env::set_var("OXY_OLTP_ADMIN_URL", admin);
    }
}

/// The app's row, its sandboxes created, and `dev-a1` asking for a branch.
async fn app_with_a_branch_sandbox(app: &OltpApp, planes: Value) -> apps::Model {
    point_at_the_test_cluster().await;
    let row = with_two_sandboxes(app).await;
    set_planes(app, &row, ENVIRONMENT, planes).await;
    row
}

fn task(app: &OltpApp, row: &apps::Model, op: BranchOp) -> EnvBranchTask {
    task_in(app, row, ENVIRONMENT, op)
}

fn task_in(app: &OltpApp, row: &apps::Model, environment: &str, op: BranchOp) -> EnvBranchTask {
    EnvBranchTask {
        app_id: row.id,
        app_slug: app.slug.clone(),
        org_id: app.t.org_id,
        workspace_id: app.workspace,
        environment: environment.to_string(),
        op,
    }
}

/// `planes` as the planes of sandbox `environment`.
async fn set_planes(app: &OltpApp, row: &apps::Model, environment: &str, planes: Value) {
    app_environments::Entity::update_many()
        .col_expr(app_environments::Column::Planes, Expr::value(planes))
        .filter(app_environments::Column::AppId.eq(row.id))
        .filter(app_environments::Column::Name.eq(environment))
        .exec(&app.t.db)
        .await
        .expect("write the sandbox's planes");
}

const CUT: BranchOp = BranchOp::Cut { at: None };

async fn branch_of(app: &OltpApp, row: &apps::Model) -> oxy_oltp::entity::env_branches::Model {
    find_for_app(&app.t.db, row.id, ENVIRONMENT)
        .await
        .expect("read the branch row")
        .expect("the environment has a branch row")
}

async fn ledger(app: &OltpApp, row: &apps::Model, branch_id: &str) -> Vec<String> {
    let target = MigrationTarget::Branch(branch_id.to_string());
    let mut names: Vec<String> = read_ledger(&app.t.db, row.id, "oltp", &target)
        .await
        .expect("read the ledger")
        .into_keys()
        .collect();
    names.sort();
    names
}

/// `select tier from orders where id = 1` on the environment's branch.
async fn tier_on_the_branch(app: &OltpApp, row: &apps::Model) -> Result<String, String> {
    let writer = resolve_env_branch_writer_for_org(
        &app.t.db,
        app.t.org_id,
        row.id,
        ENVIRONMENT,
        &app.store.writer,
        EnvBranchAccept::Ready,
    )
    .await
    .map_err(|e| e.to_string())?
    .expect("the environment has a branch row");
    let client = oxy_oltp::connect::connect(&writer.connection.dsn, "env branch test")
        .await
        .expect("connect to the branch");
    let got = client
        .query_one("select tier from orders where id = 1", &[])
        .await
        .map_err(|e| e.to_string())?;
    Ok(got.get(0))
}

/// Every queued environment-branch task of the app through the executor the
/// worker fleet registers, each taken off the queue: what each answered.
async fn run_queued(db: &DatabaseConnection, app_id: Uuid) -> Vec<TaskOutcome> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT task_id, spec FROM agentic_task_queue \
             WHERE queue_status = 'queued' AND spec->>'kind' = $1 \
               AND spec->'payload'->>'app_id' = $2 ORDER BY created_at, task_id",
            [ENV_OLTP_BRANCH_KIND.into(), app_id.to_string().into()],
        ))
        .await
        .expect("read the queue");
    let mut outcomes = Vec::new();
    for row in rows {
        let task_id: String = row.try_get("", "task_id").expect("task_id");
        let spec: Value = row.try_get("", "spec").expect("spec");
        let mut running = EnvBranchExecutor { db: db.clone() }
            .execute(TaskAssignment {
                task_id: task_id.clone(),
                parent_task_id: None,
                run_id: task_id.clone(),
                spec: serde_json::from_value(spec).expect("a TaskSpec"),
                policy: None,
                claim_epoch: agentic_core::NO_CLAIM,
            })
            .await
            .expect("the executor takes an environment branch task");
        outcomes.push(running.outcomes.recv().await.expect("an outcome"));
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue SET queue_status = 'completed' WHERE task_id = $1",
            [task_id.into()],
        ))
        .await
        .expect("complete the task");
    }
    outcomes
}

#[tokio::test]
async fn a_cut_with_a_build_applies_its_migrations_and_is_ready() {
    let app = OltpApp::provision(&crate::sandbox_oltp_isolation::functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(
            &app,
            HANDLE,
            "a-1",
            &[("migrations/0001_tier.sql", ADD_TIER)],
        )
        .await;
        // The publish queued the cut (`custom_apps_env_branch::publish`): the
        // sandbox had no branch yet.
        let outcomes = run_queued(&app.t.db, row.id).await;
        let [TaskOutcome::Done { answer, .. }] = outcomes.as_slice() else {
            panic!("one task, and it must succeed: {outcomes:?}");
        };
        assert!(answer.contains("1 schema migration(s) applied"), "{answer}");

        let branch = branch_of(&app, &row).await;
        assert_eq!(branch.status, EnvBranchStatus::Ready);
        assert_eq!(branch.error, None);
        let id = branch.provider_branch_id.expect("a provider branch");
        assert_eq!(ledger(&app, &row, &id).await, ["0001_tier.sql"]);
        assert_eq!(tier_on_the_branch(&app, &row).await.as_deref(), Ok("gold"));
        assert_eq!(app.order_count().await, 1, "production is untouched");
    })
    .await;
}

#[tokio::test]
async fn a_cut_without_a_build_is_ready_with_no_migrations() {
    let app = OltpApp::provision(&crate::sandbox_oltp_isolation::functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "empty"})).await;
        let answer = run(&app.t.db, &task(&app, &row, CUT)).await.expect("cut");
        assert!(answer.contains("no build to migrate"), "{answer}");

        let branch = branch_of(&app, &row).await;
        assert_eq!(branch.status, EnvBranchStatus::Ready);
        assert_eq!(branch.mode.as_str(), "empty");
        let id = branch.provider_branch_id.expect("a provider branch");
        assert!(ledger(&app, &row, &id).await.is_empty());
    })
    .await;
}

#[tokio::test]
async fn a_task_for_a_deleted_sandbox_does_nothing() {
    let app = OltpApp::provision(&crate::sandbox_oltp_isolation::functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        app_environments::Entity::update_many()
            .col_expr(
                app_environments::Column::DeletingAt,
                Expr::value(Utc::now().fixed_offset()),
            )
            .filter(app_environments::Column::AppId.eq(row.id))
            .filter(app_environments::Column::Name.eq(ENVIRONMENT))
            .exec(&app.t.db)
            .await
            .expect("mark the sandbox deleting");

        let answer = run(&app.t.db, &task(&app, &row, CUT))
            .await
            .expect("a no-op");
        assert!(answer.contains("nothing was done"), "{answer}");
        let branch = find_for_app(&app.t.db, row.id, ENVIRONMENT).await.unwrap();
        assert!(branch.is_none(), "no branch was cut");
    })
    .await;
}

#[tokio::test]
async fn a_cut_whose_migration_fails_is_failed_not_ready() {
    let app = OltpApp::provision(&crate::sandbox_oltp_isolation::functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(
            &app,
            HANDLE,
            "a-1",
            &[("migrations/0001_broken.sql", BROKEN)],
        )
        .await;

        let why = run(&app.t.db, &task(&app, &row, CUT))
            .await
            .expect_err("the migration cannot apply");
        assert!(why.contains("was not migrated"), "{why}");

        let branch = branch_of(&app, &row).await;
        assert_eq!(branch.status, EnvBranchStatus::Failed);
        assert!(branch.error.is_some_and(|e| e.contains("0001_broken.sql")));
        let refused = tier_on_the_branch(&app, &row).await.expect_err("not ready");
        assert!(
            refused.contains("not ready") || refused.contains("failed"),
            "{refused}"
        );
    })
    .await;
}

#[tokio::test]
async fn a_migrate_applies_only_the_build_the_sandbox_serves() {
    let app = OltpApp::provision(&crate::sandbox_oltp_isolation::functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        run(&app.t.db, &task(&app, &row, CUT)).await.expect("cut");
        publish(
            &app,
            HANDLE,
            "a-1",
            &[("migrations/0001_tier.sql", ADD_TIER)],
        )
        .await;
        let served = build_pk(&app.t.db, row.id, "a-1").await.expect("the build");

        let stale = BranchOp::Migrate {
            build_pk: Uuid::new_v4(),
        };
        let answer = run(&app.t.db, &task(&app, &row, stale))
            .await
            .expect("a no-op");
        assert!(answer.contains("no longer serves this build"), "{answer}");

        let current = BranchOp::Migrate { build_pk: served };
        let answer = run(&app.t.db, &task(&app, &row, current))
            .await
            .expect("migrate");
        assert!(answer.contains("1 schema migration(s) applied"), "{answer}");
        let branch = branch_of(&app, &row).await;
        assert_eq!(branch.status, EnvBranchStatus::Ready);
        let id = branch.provider_branch_id.expect("a provider branch");
        assert_eq!(ledger(&app, &row, &id).await, ["0001_tier.sql"]);
    })
    .await;
}
