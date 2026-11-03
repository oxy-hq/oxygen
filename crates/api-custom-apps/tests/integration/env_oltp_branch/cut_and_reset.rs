//! What a cut or reset applies, and what a reset leaves (Task 6, fix round 1).
//!
//! - **a publish during a cut** moves the sandbox's build after the run read
//!   it; the run applies the build the row names *after* the provider call,
//!   because the publish's own `Migrate` cannot get the lock until the run is
//!   over;
//! - **a reset** cuts the branch again — its data is the parent's, not the
//!   branch's — and ends `ready`.

use entity::app_environments;
use oxy_api_custom_apps::custom_apps_env_branch::task::{BranchOp, run, run_with};
use oxy_api_custom_apps::custom_apps_migrations::{MigrationTarget, read_ledger};
use oxy_oltp::entity::env_branches::EnvBranchStatus;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use uuid::Uuid;

use super::{ENVIRONMENT, HANDLE, app_with_a_branch_sandbox, branch_of, task};
use crate::sandbox_oltp_isolation::{functions, publish, run_in};
use crate::sandbox_publish::build_pk;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

const ADD_TIER: &[u8] = b"alter table orders add column tier text default 'gold';";
const ADD_NOTE: &[u8] = b"alter table orders add column note text default 'n';";

async fn serve_build(app: &OltpApp, app_id: Uuid, build: Uuid) {
    app_environments::Entity::update_many()
        .col_expr(app_environments::Column::BuildId, Expr::value(build))
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(ENVIRONMENT))
        .exec(&app.t.db)
        .await
        .expect("move the sandbox's build");
}

#[tokio::test]
async fn a_cut_applies_the_build_the_sandbox_serves_after_the_provider_call() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        let first = [("migrations/0001_tier.sql", ADD_TIER)];
        publish(&app, HANDLE, "a-1", &first).await;
        let second = [
            ("migrations/0001_tier.sql", ADD_TIER),
            ("migrations/0002_note.sql", ADD_NOTE),
        ];
        publish(&app, HANDLE, "a-2", &second).await;
        let old = build_pk(&app.t.db, row.id, "a-1").await.expect("build a-1");
        let new = build_pk(&app.t.db, row.id, "a-2").await.expect("build a-2");

        // The run reads the sandbox serving a-1; a publish moves it to a-2
        // while the provider is cutting.
        serve_build(&app, row.id, old).await;
        let publish_lands = serve_build(&app, row.id, new);
        let cut = task(&app, &row, BranchOp::Cut { at: None });
        let answer = run_with(&app.t.db, &cut, publish_lands).await.expect("cut");
        assert!(answer.contains("2 schema migration(s) applied"), "{answer}");

        let branch = branch_of(&app, &row).await;
        assert_eq!(branch.status, EnvBranchStatus::Ready);
        let id = branch.provider_branch_id.expect("a provider branch");
        let target = MigrationTarget::Branch(id);
        let applied = read_ledger(&app.t.db, row.id, "oltp", &target)
            .await
            .expect("read the ledger");
        let mut names: Vec<_> = applied.into_keys().collect();
        names.sort();
        assert_eq!(names, ["0001_tier.sql", "0002_note.sql"]);
    })
    .await;
}

#[tokio::test]
async fn a_reset_cuts_the_branch_again_and_ends_ready() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(&app, HANDLE, "a-1", &[]).await;
        run(&app.t.db, &task(&app, &row, BranchOp::Cut { at: None }))
            .await
            .expect("cut");
        let ids = ["ids", "query", "select id from orders order by id"];
        let write = "insert into orders (id) values (100)";
        let wrote = run_in(&app, ENVIRONMENT, &[["w", "exec", write], ids]).await;
        assert_eq!(
            wrote["ids"]["ok"].as_array().map(Vec::len),
            Some(2),
            "{wrote}"
        );
        let before = branch_of(&app, &row).await;

        let answer = run(&app.t.db, &task(&app, &row, BranchOp::Reset { at: None }))
            .await
            .expect("reset");
        assert!(answer.contains("branch reset"), "{answer}");

        let after = branch_of(&app, &row).await;
        assert_eq!(after.status, EnvBranchStatus::Ready);
        assert_eq!(after.error, None);
        assert_ne!(
            (&after.provider_branch_id, after.last_reset_at),
            (&before.provider_branch_id, before.last_reset_at),
            "a different cut"
        );
        let got = run_in(&app, ENVIRONMENT, &[ids]).await;
        assert_eq!(
            got["ids"]["ok"].as_array().map(Vec::len),
            Some(1),
            "the branch's own write is gone: {got}"
        );
    })
    .await;
}
