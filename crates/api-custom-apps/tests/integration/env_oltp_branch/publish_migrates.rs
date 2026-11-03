//! A publish to a branch-mode sandbox (Task 8): its migrations go to its own
//! OLTP branch, by the environment-branch task, and no sandbox-schema task is
//! queued for it.

use agentic_core::delegation::TaskOutcome;
use oxy_api_custom_apps::custom_apps_env_branch::task::ENV_OLTP_BRANCH_KIND;
use oxy_api_custom_apps::custom_apps_migrations::{MigrationTarget, read_ledger};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::json;
use uuid::Uuid;

use super::{app_with_a_branch_sandbox, branch_of, run_queued, tier_on_the_branch};
use crate::sandbox_oltp_isolation::{functions, publish, queued_oltp, run_in};
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

const ADD_TIER: &[u8] = b"alter table orders add column tier text default 'gold';";
const ADD_NOTE: &[u8] = b"alter table orders add column note text default 'n';";

/// The ops of the app's queued environment-branch tasks, oldest first.
async fn queued_ops(db: &DatabaseConnection, app_id: Uuid) -> Vec<String> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec->'payload'->'op'->>'op' AS op FROM agentic_task_queue \
             WHERE queue_status = 'queued' AND spec->>'kind' = $1 \
               AND spec->'payload'->>'app_id' = $2 ORDER BY created_at, task_id",
            [ENV_OLTP_BRANCH_KIND.into(), app_id.to_string().into()],
        ))
        .await
        .expect("read the queue");
    rows.iter()
        .map(|row| row.try_get("", "op").expect("op"))
        .collect()
}

#[tokio::test]
async fn a_publish_to_a_branch_sandbox_migrates_its_branch_not_a_schema() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        let db = &app.t.db;

        // No branch yet: the publish queues a cut, which applies the build.
        let first = publish(&app, "a1", "a-1", &[("migrations/0001_tier.sql", ADD_TIER)]).await;
        assert!(first.warnings.is_empty(), "{:?}", first.warnings);
        assert!(queued_oltp(db, row.id).await.is_empty(), "no schema task");
        assert_eq!(queued_ops(db, row.id).await, ["cut"]);
        let outcomes = run_queued(db, row.id).await;
        let [TaskOutcome::Done { answer, .. }] = outcomes.as_slice() else {
            panic!("one task, and it must succeed: {outcomes:?}");
        };
        assert!(answer.contains("1 schema migration(s) applied"), "{answer}");
        assert_eq!(tier_on_the_branch(&app, &row).await.as_deref(), Ok("gold"));

        // A branch exists: the next publish queues a migrate of its build.
        let files = [
            ("migrations/0001_tier.sql", ADD_TIER),
            ("migrations/0002_note.sql", ADD_NOTE),
        ];
        let second = publish(&app, "a1", "a-2", &files).await;
        assert!(second.warnings.is_empty(), "{:?}", second.warnings);
        assert!(queued_oltp(db, row.id).await.is_empty(), "no schema task");
        assert_eq!(queued_ops(db, row.id).await, ["migrate"]);
        let outcomes = run_queued(db, row.id).await;
        assert!(matches!(outcomes.as_slice(), [TaskOutcome::Done { .. }]));

        let id = branch_of(&app, &row)
            .await
            .provider_branch_id
            .expect("a provider branch");
        let target = MigrationTarget::Branch(id);
        let applied = read_ledger(db, row.id, "oltp", &target)
            .await
            .expect("read the ledger");
        let mut names: Vec<_> = applied.into_keys().collect();
        names.sort();
        assert_eq!(names, ["0001_tier.sql", "0002_note.sql"]);

        // The column is on the branch, and not in production.
        let tier: [&str; 3] = ["tier", "query", "select tier from orders where id = 1"];
        let production = run_in(&app, "production", &[tier]).await;
        let error = production["tier"]["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("tier"),
            "production has no such column: {production}"
        );
        assert_eq!(app.order_count().await, 1);
    })
    .await;
}

/// A sandbox whose fork point is older than the history the database keeps —
/// its first cut refused days ago — is cut now by its next publish, and its
/// fork point says so; never a cut that fails as outside the history.
#[tokio::test]
async fn a_publish_cuts_a_stale_fork_point_now() {
    use chrono::{Duration, Utc};
    use entity::app_environments;
    use sea_orm::sea_query::Expr;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        let db = &app.t.db;
        let stale = Utc::now() - Duration::days(3);
        app_environments::Entity::update_many()
            .col_expr(
                app_environments::Column::ForkAt,
                Expr::value(stale.fixed_offset()),
            )
            .filter(app_environments::Column::AppId.eq(row.id))
            .filter(app_environments::Column::Name.eq(super::ENVIRONMENT))
            .exec(db)
            .await
            .expect("age the fork point");

        publish(&app, "a1", "a-1", &[]).await;
        assert_eq!(queued_ops(db, row.id).await, ["cut"]);
        let outcomes = run_queued(db, row.id).await;
        assert!(
            matches!(outcomes.as_slice(), [TaskOutcome::Done { .. }]),
            "{outcomes:?}"
        );

        let sandbox =
            app_environments::Entity::find_by_id((row.id, super::ENVIRONMENT.to_string()))
                .one(db)
                .await
                .expect("read the sandbox")
                .expect("the sandbox");
        let fork_at = sandbox.fork_at.expect("a fork point").to_utc();
        assert!(
            fork_at > Utc::now() - Duration::hours(1),
            "moved to now: {fork_at}"
        );
    })
    .await;
}
