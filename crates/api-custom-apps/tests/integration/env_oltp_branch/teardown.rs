//! A sandbox's OLTP branch under teardown and expiry (Task 9).
//!
//! - **a teardown** deletes the branch's database, its `oltp_env_branches` row
//!   and its ledger rows under the sandbox's lock, and only then the sandbox's
//!   row; a delete that fails (a row naming production, refused) keeps the
//!   sandbox `deleting` for the retry;
//! - **the fork expiry** counts from the fork point, never from activity, and
//!   only for a sandbox with a plane that copies production rows;
//! - **a reset** between the sweep's read and its delete saves the sandbox;
//! - **an orphaned branch** — its sandbox row gone by a cascade, with no
//!   teardown — is deleted by the sweep, and a live sandbox's is kept.

use chrono::{DateTime, Duration, Utc};
use entity::{app_environments, apps};
use oxy_api_custom_apps::custom_apps_sandboxes::delete::{Expect, delete_if};
use oxy_api_custom_apps::custom_apps_sandboxes::maintenance::sweep;
use oxy_api_custom_apps::custom_apps_sandboxes::{TeardownReason, teardown};
use oxy_oltp::entity::tenants;
use oxy_oltp::env_branches::find_for_app;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde_json::json;

use super::{
    CUT, ENVIRONMENT, HANDLE, app_with_a_branch_sandbox, branch_of, ledger, run_queued, set_planes,
    task_in,
};
use crate::sandbox_oltp_isolation::{functions, publish};
use crate::sandbox_oltp_teardown::begin_delete;
use crate::sandbox_publish::sandbox;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};
use oxy_api_custom_apps::custom_apps_env_branch::task::run;

const ADD_TIER: &[u8] = b"alter table orders add column tier text default 'gold';";
const SIBLING: &str = "dev-b2";

pub(super) async fn database_exists(name: &str) -> bool {
    let admin = crate::common::admin_url().await;
    let client = oxy_oltp::connect::connect(&admin, "env branch teardown test")
        .await
        .expect("connect admin");
    let n: i64 = client
        .query_one(
            "SELECT count(*) FROM pg_database WHERE datname = $1",
            &[&name],
        )
        .await
        .expect("probe")
        .get(0);
    n > 0
}

async fn sandbox_row(app: &OltpApp, app_id: uuid::Uuid, name: &str) -> app_environments::Model {
    app_environments::Entity::find_by_id((app_id, name.to_string()))
        .one(&app.t.db)
        .await
        .expect("read the sandbox's row")
        .expect("the sandbox's row")
}

pub(super) async fn set_fork_at(app: &OltpApp, app_id: uuid::Uuid, name: &str, at: DateTime<Utc>) {
    app_environments::Entity::update_many()
        .col_expr(
            app_environments::Column::ForkAt,
            Expr::value(at.fixed_offset()),
        )
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(name))
        .exec(&app.t.db)
        .await
        .expect("set the fork point");
}

/// The reason of the teardown queued for `name`, if one is.
pub(super) async fn queued_reason(app: &OltpApp, app_id: uuid::Uuid, name: &str) -> Option<String> {
    let row = app
        .t
        .db
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT spec->'payload'->>'reason' AS reason FROM agentic_task_queue \
              WHERE spec->>'kind' = $1 AND spec->'payload'->>'app_id' = $2 \
                AND spec->'payload'->>'environment' = $3",
            [
                teardown::SANDBOX_TEARDOWN_KIND.into(),
                app_id.to_string().into(),
                name.into(),
            ],
        ))
        .await
        .expect("read the queue")?;
    Some(row.try_get("", "reason").expect("reason"))
}

pub(super) async fn is_deleting(app: &OltpApp, app_id: uuid::Uuid, name: &str) -> bool {
    sandbox_row(app, app_id, name).await.deleting_at.is_some()
}

/// Both sandboxes ask for a branch of their own.
async fn two_branch_sandboxes(app: &OltpApp) -> apps::Model {
    let row = app_with_a_branch_sandbox(app, json!({"oltp": "branch"})).await;
    set_planes(app, &row, SIBLING, json!({"oltp": "branch"})).await;
    row
}

#[tokio::test]
async fn delete_tears_down_the_branch_under_the_teardown_lock() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        let files: &[(&str, &[u8])] = &[("migrations/0001_tier.sql", ADD_TIER)];
        publish(&app, HANDLE, "a-1", files).await;
        run_queued(&app.t.db, row.id).await;
        let branch = branch_of(&app, &row).await;
        let id = branch
            .provider_branch_id
            .clone()
            .expect("a provider branch");
        let database = branch.database_name.clone().expect("a database");
        assert!(database_exists(&database).await, "the branch was cut");
        assert_eq!(ledger(&app, &row, &id).await, ["0001_tier.sql"]);

        let task = begin_delete(&app, &row, HANDLE).await;
        let summary = teardown::run(&app.t.db, &task).await.expect("teardown");
        assert!(summary.contains("OLTP branch deleted"), "{summary}");

        assert!(!database_exists(&database).await, "the database is gone");
        let found = find_for_app(&app.t.db, row.id, ENVIRONMENT).await.unwrap();
        assert!(found.is_none(), "the branch row is gone");
        assert!(ledger(&app, &row, &id).await.is_empty(), "so is its ledger");
        let gone = app_environments::Entity::find_by_id((row.id, ENVIRONMENT.to_string()))
            .one(&app.t.db)
            .await
            .unwrap();
        assert!(gone.is_none(), "the sandbox's row goes last");
        assert_eq!(app.order_count().await, 1, "production is untouched");
    })
    .await;
}

#[tokio::test]
async fn a_failed_branch_delete_keeps_the_row_for_retry() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(&app, HANDLE, "a-1", &[]).await;
        run_queued(&app.t.db, row.id).await;
        let branch = branch_of(&app, &row).await;
        // Point the row at production's own database, which the provisioner
        // refuses to delete (`LocalProvider` decides by database name).
        let tenant = tenants::Entity::find()
            .filter(tenants::Column::OrgId.eq(app.t.org_id))
            .one(&app.t.db)
            .await
            .unwrap()
            .expect("the org's tenant");
        oxy_oltp::entity::env_branches::Entity::update_many()
            .col_expr(
                oxy_oltp::entity::env_branches::Column::DatabaseName,
                Expr::value(tenant.database_name.clone()),
            )
            .filter(oxy_oltp::entity::env_branches::Column::Id.eq(branch.id))
            .exec(&app.t.db)
            .await
            .expect("point the row at production");

        let task = begin_delete(&app, &row, HANDLE).await;
        let why = teardown::run(&app.t.db, &task).await.expect_err("refused");
        // The teardown wraps every cause in "OLTP branch"; the cause here must
        // be the production refusal, not some other failure.
        assert!(
            why.contains("which is its production database — refusing to touch it"),
            "{why}"
        );

        let kept = sandbox_row(&app, row.id, ENVIRONMENT).await;
        assert!(kept.deleting_at.is_some(), "it stays deleting for a retry");
        let found = find_for_app(&app.t.db, row.id, ENVIRONMENT).await.unwrap();
        assert!(found.is_some(), "the branch row is kept");
        assert!(
            database_exists(&tenant.database_name).await,
            "production stands"
        );
        assert_eq!(app.order_count().await, 1, "production is untouched");
    })
    .await;
}

#[tokio::test]
async fn expiry_counts_from_the_fork_not_activity() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = two_branch_sandboxes(&app).await;
        let now = Utc::now();
        set_fork_at(&app, row.id, ENVIRONMENT, now - Duration::days(31)).await;
        set_fork_at(&app, row.id, SIBLING, now - Duration::days(29)).await;
        // Both were used a moment ago: activity is not what expires them.
        assert!(!is_deleting(&app, row.id, ENVIRONMENT).await);

        let queued = sweep(&app.t.db, now).await.expect("sweep");

        assert!(queued >= 1, "the 31-day-old fork is queued");
        assert!(is_deleting(&app, row.id, ENVIRONMENT).await);
        let reason = queued_reason(&app, row.id, ENVIRONMENT).await;
        assert_eq!(reason.as_deref(), Some("oltp_branch_expired"));
        assert!(
            !is_deleting(&app, row.id, SIBLING).await,
            "29 days is young"
        );
        assert_eq!(queued_reason(&app, row.id, SIBLING).await, None);
    })
    .await;
}

#[tokio::test]
async fn a_sandbox_with_no_forking_plane_does_not_expire() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        // `dev-a1` keeps the default OLTP plane, a schema; `dev-b2` asks for
        // an empty branch, which copies nothing.
        let row = app_with_a_branch_sandbox(&app, json!({})).await;
        set_planes(&app, &row, SIBLING, json!({"oltp": "empty"})).await;
        let now = Utc::now();
        for name in [ENVIRONMENT, SIBLING] {
            set_fork_at(&app, row.id, name, now - Duration::days(31)).await;
        }
        // The control: the same age, with a plane that does copy rows.
        let control = app_with_a_branch_sandbox_control(&app, &row).await;

        sweep(&app.t.db, now).await.expect("sweep");

        for name in [ENVIRONMENT, SIBLING] {
            assert!(!is_deleting(&app, row.id, name).await, "{name}");
        }
        assert!(is_deleting(&app, row.id, &control).await, "the control");
    })
    .await;
}

/// A row of `name`'s Airhouse overlay registry, as a copy-on-write leaves it.
pub(super) async fn overlay_row(app: &OltpApp, row: &apps::Model, name: &str, state: &str) {
    app.t
        .db
        .execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "INSERT INTO workspace_preview_tables \
                 (workspace_id, preview_key, live_schema, table_name, state, last_run_id) \
             VALUES ($1, $2, 'app_orders', 'orders', $3, 'r-1')",
            [
                row.project_id.into(),
                format!("env:{}:{name}", row.id).into(),
                state.into(),
            ],
        ))
        .await
        .expect("record an overlay table");
}

/// A sandbox moved off `branch` keeps the Airhouse rows it copied, so it
/// expires from its fork while its registry lists one; a table it dropped is
/// no copy. Both keep the default planes, which copy nothing.
#[tokio::test]
async fn leftover_airhouse_copies_expire_from_the_fork() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({})).await;
        let now = Utc::now();
        for name in [ENVIRONMENT, SIBLING] {
            set_fork_at(&app, row.id, name, now - Duration::days(31)).await;
        }
        overlay_row(&app, &row, ENVIRONMENT, "shadow").await;
        overlay_row(&app, &row, SIBLING, "dropped").await;

        sweep(&app.t.db, now).await.expect("sweep");

        assert!(is_deleting(&app, row.id, ENVIRONMENT).await, "holds a copy");
        let reason = queued_reason(&app, row.id, ENVIRONMENT).await;
        assert_eq!(reason.as_deref(), Some("oltp_branch_expired"));
        assert!(!is_deleting(&app, row.id, SIBLING).await, "only dropped");
    })
    .await;
}

/// A third sandbox, `dev-c3`, 31 days past its fork and in OLTP `branch`.
async fn app_with_a_branch_sandbox_control(app: &OltpApp, row: &apps::Model) -> String {
    let c3 = sandbox("c3");
    oxy_api_custom_apps::custom_apps_sandboxes::ops::create(&app.t.db, row, &c3, &app.t.guest())
        .await
        .expect("create dev-c3");
    set_planes(app, row, "dev-c3", json!({"oltp": "branch"})).await;
    set_fork_at(app, row.id, "dev-c3", Utc::now() - Duration::days(31)).await;
    "dev-c3".to_string()
}

#[tokio::test]
async fn a_reset_between_select_and_delete_saves_the_sandbox() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = two_branch_sandboxes(&app).await;
        let now = Utc::now();
        let cutoff = now - Duration::days(30);
        let expect = Expect::ForkedBefore(cutoff);
        for name in [ENVIRONMENT, SIBLING] {
            set_fork_at(&app, row.id, name, now - Duration::days(31)).await;
        }
        // `dev-a1` is reset after the sweep read its row: its fork moves.
        set_fork_at(&app, row.id, ENVIRONMENT, now).await;
        let reason = TeardownReason::BranchExpired;

        let saved = delete_if(&app.t.db, &row, &sandbox(HANDLE), None, reason, expect).await;
        assert_eq!(saved.expect("the delete"), None, "nothing written");
        assert!(!is_deleting(&app, row.id, ENVIRONMENT).await);

        let still = delete_if(&app.t.db, &row, &sandbox("b2"), None, reason, expect).await;
        let deletion = still.expect("the delete").expect("still forked before it");
        assert!(deletion.queued);
        assert!(is_deleting(&app, row.id, SIBLING).await, "the control");
    })
    .await;
}

/// An app or user delete cascades a sandbox's row away with no teardown. The
/// sweep deletes the branch it leaves — row and database — and keeps a live
/// sandbox's.
#[tokio::test]
async fn the_sweep_deletes_a_branch_no_sandbox_holds() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = two_branch_sandboxes(&app).await;
        for name in [ENVIRONMENT, SIBLING] {
            run(&app.t.db, &task_in(&app, &row, name, CUT))
                .await
                .expect("cut");
        }
        let orphan = branch_of(&app, &row).await;
        let orphan_db = orphan.database_name.clone().expect("a database");
        let live = find_for_app(&app.t.db, row.id, SIBLING)
            .await
            .unwrap()
            .expect("the control's branch");
        let live_db = live.database_name.clone().expect("a database");
        // As the cascade does: the sandbox's row goes, and no teardown runs.
        app_environments::Entity::delete_by_id((row.id, ENVIRONMENT.to_string()))
            .exec(&app.t.db)
            .await
            .expect("drop the sandbox's row");

        sweep(&app.t.db, Utc::now()).await.expect("sweep");

        let found = find_for_app(&app.t.db, row.id, ENVIRONMENT).await.unwrap();
        assert!(found.is_none(), "the orphaned branch row is gone");
        assert!(!database_exists(&orphan_db).await, "and its database");
        let kept = find_for_app(&app.t.db, row.id, SIBLING).await.unwrap();
        assert_eq!(
            kept.map(|b| b.id),
            Some(live.id),
            "a live sandbox's is kept"
        );
        assert!(database_exists(&live_db).await, "with its database");
        assert_eq!(app.order_count().await, 1, "production is untouched");
    })
    .await;
}
