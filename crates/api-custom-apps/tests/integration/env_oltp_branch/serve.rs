//! `ctx.oltp` on a sandbox's own OLTP branch (Task 7): the real publish, the
//! real cut by the task, and the real serve route, against a branch
//! `LocalProvider` copied for real — every assertion counts rows or reads the
//! catalog in the databases themselves.
//!
//! - **two branch sandboxes** write one table and see only their own rows;
//!   production is unchanged and no staging branch is needed;
//! - **a branch reads production's rows as of the cut**, not after;
//! - **before the cut finishes** `ctx.oltp` is refused, saying how to cut it;
//! - **a call admitted before a reset** fails rather than follow it;
//! - **a statement reaching outside the branch** is refused;
//! - **a production call** still writes production.

use oxy_api_custom_apps::custom_apps_env_branch::home::home_for;
use oxy_api_custom_apps::custom_apps_env_branch::task::{BranchOp, run};
use oxy_api_custom_apps::custom_apps_functions::env_policy::{EnvBranchUnready, OltpHome};
use oxy_api_custom_apps::custom_apps_functions::host::writer_connection;
use oxy_custom_apps::planes::OltpMode;
use serde_json::{Value, json};

use super::{ENVIRONMENT, app_with_a_branch_sandbox, branch_of, task, task_in};
use crate::sandbox_oltp_isolation::{functions, publish, run_in};
use crate::sandbox_publish::sandbox;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

const IDS: [&str; 3] = ["ids", "query", "select id from orders order by id"];

fn insert(id: i64) -> String {
    format!("insert into orders (id) values ({id})")
}

fn ids(got: &Value, key: &str) -> Vec<i64> {
    let rows = got[key]["ok"]
        .as_array()
        .unwrap_or_else(|| panic!("{key}: {got}"));
    rows.iter()
        .map(|row| row["id"].as_i64().expect("an id"))
        .collect()
}

fn error_of(got: &Value, key: &str) -> String {
    got[key]["error"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn two_branch_sandboxes_write_the_same_table_and_see_only_their_own_rows() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        // No staging branch is provisioned: a branch sandbox needs none.
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(&app, "a1", "a-1", &[]).await;
        publish(&app, "b2", "b-1", &[]).await;
        super::set_planes(&app, &row, "dev-b2", json!({"oltp": "branch"})).await;
        for env in ["dev-a1", "dev-b2"] {
            run(
                &app.t.db,
                &task_in(&app, &row, env, BranchOp::Cut { at: None }),
            )
            .await
            .expect("cut");
        }

        let (in_a1, in_b2, in_production) = (insert(100), insert(200), insert(400));
        let a1 = run_in(&app, "dev-a1", &[["w", "exec", &in_a1], IDS]).await;
        let b2 = run_in(&app, "dev-b2", &[["w", "tx", &in_b2], IDS]).await;
        let production = run_in(&app, "production", &[["w", "exec", &in_production], IDS]).await;
        assert_eq!(ids(&a1, "ids"), [1, 100], "{a1}");
        assert_eq!(ids(&b2, "ids"), [1, 200], "{b2}");
        assert_eq!(ids(&production, "ids"), [1, 400], "{production}");

        assert_eq!(ids(&run_in(&app, "dev-a1", &[IDS]).await, "ids"), [1, 100]);
        assert_eq!(ids(&run_in(&app, "dev-b2", &[IDS]).await, "ids"), [1, 200]);
        assert_eq!(app.order_count().await, 2, "production holds its own two");
    })
    .await;
}

#[tokio::test]
async fn a_branch_sandbox_reads_production_rows_as_of_the_cut() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(&app, "a1", "a-1", &[]).await;
        let before = insert(5);
        run_in(&app, "production", &[["w", "exec", &before]]).await;
        run(&app.t.db, &task(&app, &row, BranchOp::Cut { at: None }))
            .await
            .expect("cut");
        let after = insert(6);
        run_in(&app, "production", &[["w", "exec", &after]]).await;

        let got = run_in(&app, ENVIRONMENT, &[IDS]).await;
        assert_eq!(ids(&got, "ids"), [1, 5], "the rows at the cut: {got}");
        assert_eq!(app.order_count().await, 3, "production kept both writes");
    })
    .await;
}

#[tokio::test]
async fn before_the_cut_finishes_ctx_oltp_is_refused() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(&app, "a1", "a-1", &[]).await;

        let early = run_in(&app, ENVIRONMENT, &[IDS, ["w", "exec", &insert(9)]]).await;
        for key in ["ids", "w"] {
            let error = error_of(&early, key);
            assert!(
                error.contains("EnvironmentRefused: ctx.oltp.")
                    && error.contains(EnvBranchUnready::NotCut.fix()),
                "{key}: {early}"
            );
        }
        assert_eq!(app.order_count().await, 1, "production is untouched");
    })
    .await;
}

#[tokio::test]
async fn a_call_admitted_before_a_reset_fails_rather_than_follow() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        run(&app.t.db, &task(&app, &row, BranchOp::Cut { at: None }))
            .await
            .expect("cut");
        let (db, org) = (&app.t.db, app.t.org_id);
        let writer = oxy_oltp::schema::app_writer_name(&app.slug).expect("a writer name");

        let home = home_for(db, &row, &sandbox("a1"), OltpMode::Branch)
            .await
            .expect("admit");
        assert!(matches!(home, OltpHome::EnvironmentBranch(_)), "{home:?}");
        writer_connection(db, org, &writer, &home)
            .await
            .expect("the admitted cut serves");

        run(db, &task(&app, &row, BranchOp::Reset { at: None }))
            .await
            .expect("reset");
        assert_eq!(branch_of(&app, &row).await.status.as_str(), "ready");
        let err = writer_connection(db, org, &writer, &home)
            .await
            .expect_err("the old cut is gone");
        assert!(err.contains("reset"), "{err}");

        let fresh = home_for(db, &row, &sandbox("a1"), OltpMode::Branch)
            .await
            .expect("admit again");
        writer_connection(db, org, &writer, &fresh)
            .await
            .expect("a call admitted after the reset serves");
    })
    .await;
}

/// A branch is served in the mode it was cut in. `empty` exists so a sandbox
/// does not see a copy of production's rows, and a `branch` copy must not
/// keep serving them after the planes say `empty` (or the reverse).
#[tokio::test]
async fn a_branch_cut_in_one_mode_is_refused_when_the_sandbox_now_wants_the_other() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(&app, "a1", "a-1", &[]).await;
        super::set_planes(&app, &row, "dev-b2", json!({"oltp": "empty"})).await;
        for env in [ENVIRONMENT, "dev-b2"] {
            run(
                &app.t.db,
                &task_in(&app, &row, env, BranchOp::Cut { at: None }),
            )
            .await
            .expect("cut");
        }
        let admit = |handle: &'static str, mode| {
            let (db, row) = (&app.t.db, &row);
            async move {
                home_for(db, row, &sandbox(handle), mode)
                    .await
                    .expect("admit")
            }
        };
        assert!(matches!(
            admit("a1", OltpMode::Branch).await,
            OltpHome::EnvironmentBranch(_)
        ));
        assert!(matches!(
            admit("b2", OltpMode::Empty).await,
            OltpHome::EnvironmentBranch(_)
        ));

        // The planes change under a branch already cut.
        assert_eq!(
            admit("a1", OltpMode::Empty).await,
            OltpHome::EnvironmentBranchUnready(EnvBranchUnready::CutAsBranch)
        );
        assert_eq!(
            admit("b2", OltpMode::Branch).await,
            OltpHome::EnvironmentBranchUnready(EnvBranchUnready::CutAsEmpty)
        );

        // Through the serve route: the call is refused, not served the copy.
        super::set_planes(&app, &row, ENVIRONMENT, json!({"oltp": "empty"})).await;
        let got = run_in(&app, ENVIRONMENT, &[IDS]).await;
        let error = error_of(&got, "ids");
        assert!(
            error.contains("EnvironmentRefused: ctx.oltp.query")
                && error.contains(EnvBranchUnready::CutAsBranch.fix()),
            "{got}"
        );
    })
    .await;
}

#[tokio::test]
async fn a_statement_reaching_outside_the_branch_is_refused() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(&app, "a1", "a-1", &[]).await;
        run(&app.t.db, &task(&app, &row, BranchOp::Cut { at: None }))
            .await
            .expect("cut");

        let read_file = "select pg_read_file('postgresql.conf')";
        let got = run_in(&app, ENVIRONMENT, &[["f", "query", read_file], IDS]).await;
        let error = error_of(&got, "f");
        assert!(
            error.contains("EnvironmentRefused: ctx.") && error.contains("This statement"),
            "{got}"
        );
        assert_eq!(ids(&got, "ids"), [1], "an ordinary read is sent");
    })
    .await;
}

#[tokio::test]
async fn production_admitted_calls_still_write_production() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(&app, "a1", "a-1", &[]).await;
        run(&app.t.db, &task(&app, &row, BranchOp::Cut { at: None }))
            .await
            .expect("cut");

        let write = insert(400);
        let got = run_in(&app, "production", &[["w", "exec", &write], IDS]).await;
        assert_eq!(ids(&got, "ids"), [1, 400], "{got}");
        assert_eq!(app.order_count().await, 2);
        let branch = run_in(&app, ENVIRONMENT, &[IDS]).await;
        assert_eq!(ids(&branch, "ids"), [1], "the branch kept its own copy");
    })
    .await;
}
