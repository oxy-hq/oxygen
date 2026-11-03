//! The OLTP plane's reset and re-cut paths through the environment routes
//! (Task 11): the reset route, a fork point outside the history window, a
//! switch between `branch` and `empty`, a pending drop, and the expiry clock a
//! reset restarts. The helpers are `planes`'.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::app_environments;
use oxy_api_custom_apps::custom_apps_env_branch::home::home_for;
use oxy_api_custom_apps::custom_apps_env_branch::task::{ENV_OLTP_BRANCH_KIND, run};
use oxy_api_custom_apps::custom_apps_functions::env_policy::{EnvBranchUnready, OltpHome};
use oxy_api_custom_apps::custom_apps_sandboxes::maintenance::sweep;
use oxy_custom_apps::planes::OltpMode;
use oxy_oltp::entity::env_branches::{EnvBranchMode, EnvBranchStatus};
use oxy_oltp::env_branches::find_for_app;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;

use super::planes::{
    CUT, SIBLING, close, create, done, orders_on_the_branch, put_planes, queued, reset, sandbox_row,
};
use super::{ENVIRONMENT, app_with_a_branch_sandbox, branch_of, run_queued, set_planes, task_in};
use crate::sandbox_oltp_isolation::functions;
use crate::sandbox_publish::sandbox;
use crate::sandbox_routes::send;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

#[tokio::test]
async fn reset_plane_oltp_queues_a_reset_with_at() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        run(&app.t.db, &task_in(&app, &row, ENVIRONMENT, CUT))
            .await
            .expect("cut");
        let at = Utc::now() - Duration::hours(1);

        let body = json!({"planes": ["oltp"], "at": at.to_rfc3339()});
        let (status, answer) = reset(row.id, ENVIRONMENT, body).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        let plane = &answer["planes"][0];
        assert_eq!(
            (plane["plane"].as_str(), plane["status"].as_str()),
            (Some("oltp"), Some("queued"))
        );
        assert!(
            plane["run_id"]
                .as_str()
                .is_some_and(|id| id.contains(ENV_OLTP_BRANCH_KIND)),
            "{answer}"
        );
        let ops = queued(&app, row.id).await;
        assert_eq!(ops.len(), 1, "{ops:?}");
        assert_eq!(ops[0].1["op"], "reset");
        let queued_at: DateTime<Utc> = ops[0].1["at"].as_str().expect("an at").parse().unwrap();
        assert!(close(queued_at, at));
        assert!(close(
            sandbox_row(&app, row.id, ENVIRONMENT)
                .await
                .fork_at
                .unwrap()
                .into(),
            at
        ));

        done(&run_queued(&app.t.db, row.id).await);
        let after = branch_of(&app, &row).await;
        assert_eq!(after.status, EnvBranchStatus::Ready);
        assert!(close(after.fork_at.into(), at), "re-cut where --at said");

        // `schema` mode has no branch to reset: the schema is re-seeded by
        // publishing to the sandbox again.
        let (status, refused) = reset(row.id, SIBLING, json!({"planes": ["oltp"]})).await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert_eq!(refused["error"], "plane_not_resettable");
        let message = refused["message"].as_str().expect("a message");
        assert!(
            message.contains("publish to the sandbox again"),
            "{message}"
        );
        assert!(message.contains("--plane oltp=branch"), "{message}");
    })
    .await;
}

#[tokio::test]
async fn an_at_outside_the_history_window_is_refused() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        let old = (Utc::now() - Duration::hours(48)).to_rfc3339();
        let body = json!({"name": "dev-c3", "preset": "full", "at": old});
        let (status, refused) = create(row.id, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["error"], "invalid_planes");
        assert!(
            refused["message"]
                .as_str()
                .unwrap()
                .contains("hours of history"),
            "{refused}"
        );
        let (_, shown) = send("GET", &format!("{}/environments/dev-c3", row.id), None).await;
        assert_eq!(
            shown["error"], "environment_not_found",
            "nothing was created"
        );

        let (status, refused) =
            reset(row.id, ENVIRONMENT, json!({"planes": ["oltp"], "at": old})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["error"], "invalid_fork_at");
        assert!(queued(&app, row.id).await.is_empty());
    })
    .await;
}

/// `home_for` refuses a branch cut in the other mode, so a PUT between
/// `branch` and `empty` must leave a branch it serves: the queued op re-cuts
/// in the new mode and records it.
#[tokio::test]
async fn a_switch_between_branch_and_empty_recuts_in_the_new_mode() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        run(&app.t.db, &task_in(&app, &row, ENVIRONMENT, CUT))
            .await
            .expect("cut");
        assert_eq!(
            orders_on_the_branch(&app, &row, ENVIRONMENT).await,
            1,
            "production's row"
        );

        let (status, put) = put_planes(row.id, ENVIRONMENT, json!({"oltp": "empty"})).await;
        assert_eq!(status, StatusCode::OK, "{put}");
        let env = sandbox("a1");
        let before = home_for(&app.t.db, &row, &env, OltpMode::Empty)
            .await
            .expect("admit");
        assert_eq!(
            before,
            OltpHome::EnvironmentBranchUnready(EnvBranchUnready::CutAsBranch)
        );
        assert_eq!(
            queued(&app, row.id).await,
            [(ENVIRONMENT.to_string(), json!({"op": "reset", "at": null}))]
        );

        done(&run_queued(&app.t.db, row.id).await);
        let after = branch_of(&app, &row).await;
        assert_eq!(
            (after.mode, after.status),
            (EnvBranchMode::Empty, EnvBranchStatus::Ready)
        );
        assert_eq!(
            orders_on_the_branch(&app, &row, ENVIRONMENT).await,
            0,
            "no production rows"
        );
        let home = home_for(&app.t.db, &row, &env, OltpMode::Empty)
            .await
            .expect("admit");
        assert!(matches!(home, OltpHome::EnvironmentBranch(_)), "{home:?}");

        // And back: a branch again, with production's rows.
        put_planes(row.id, ENVIRONMENT, json!({"oltp": "branch"})).await;
        done(&run_queued(&app.t.db, row.id).await);
        assert_eq!(branch_of(&app, &row).await.mode, EnvBranchMode::Branch);
        assert_eq!(orders_on_the_branch(&app, &row, ENVIRONMENT).await, 1);
    })
    .await;
}

/// The expiry pass reads the fork from `app_environments.fork_at`; a reset
/// writes it, so a sandbox 29 days old that was reset is not torn down on day
/// 31 — and one that was not, is.
#[tokio::test]
async fn a_reset_restarts_the_forks_expiry_clock() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        set_planes(&app, &row, SIBLING, json!({"oltp": "branch"})).await;
        let now = Utc::now();
        for env in [ENVIRONMENT, SIBLING] {
            app_environments::Entity::update_many()
                .col_expr(
                    app_environments::Column::CreatedAt,
                    Expr::value((now - Duration::days(29)).fixed_offset()),
                )
                .filter(app_environments::Column::Name.eq(env))
                .filter(app_environments::Column::AppId.eq(row.id))
                .exec(&app.t.db)
                .await
                .expect("age the sandbox");
        }
        let (status, answer) = reset(row.id, ENVIRONMENT, json!({"planes": ["oltp"]})).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert!(close(
            sandbox_row(&app, row.id, ENVIRONMENT)
                .await
                .fork_at
                .unwrap()
                .into(),
            now
        ));

        // Day 31 from creation.
        sweep(&app.t.db, now + Duration::days(2))
            .await
            .expect("sweep");

        let deleting = |name: &'static str| {
            let (app, id) = (&app, row.id);
            async move { sandbox_row(app, id, name).await.deleting_at.is_some() }
        };
        assert!(!deleting(ENVIRONMENT).await, "the reset sandbox stays");
        assert!(deleting(SIBLING).await, "the control, never reset, goes");
    })
    .await;
}

/// A drop queued by a switch to `schema` that has not run when the planes ask
/// for a branch again must not delete the branch: it no-ops, and the re-cut
/// queued behind it runs.
#[tokio::test]
async fn a_pending_drop_no_ops_when_the_planes_want_a_branch_again() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        run(&app.t.db, &task_in(&app, &row, ENVIRONMENT, CUT))
            .await
            .expect("cut");
        put_planes(row.id, ENVIRONMENT, json!({"oltp": "schema"})).await;
        put_planes(row.id, ENVIRONMENT, json!({"oltp": "branch"})).await;
        let ops: Vec<_> = queued(&app, row.id)
            .await
            .into_iter()
            .map(|(_, op)| op["op"].clone())
            .collect();
        assert_eq!(ops, [json!("drop"), json!("reset")]);

        let answers = done(&run_queued(&app.t.db, row.id).await);
        assert!(
            answers[0].contains("ask for an OLTP branch again"),
            "{answers:?}"
        );
        assert!(answers[1].contains("branch reset"), "{answers:?}");
        let kept = find_for_app(&app.t.db, row.id, ENVIRONMENT).await.unwrap();
        assert_eq!(
            kept.expect("the branch stands").status,
            EnvBranchStatus::Ready
        );
    })
    .await;
}

async fn fork_at_of(app: &OltpApp, app_id: uuid::Uuid) -> Option<DateTime<Utc>> {
    let row = sandbox_row(app, app_id, ENVIRONMENT).await;
    row.fork_at.map(Into::into)
}

async fn set_fork_at(app: &OltpApp, app_id: uuid::Uuid, at: Option<DateTime<Utc>>) {
    app_environments::Entity::update_many()
        .col_expr(
            app_environments::Column::ForkAt,
            Expr::value(at.map(|a| a.fixed_offset())),
        )
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(ENVIRONMENT))
        .exec(&app.t.db)
        .await
        .expect("set the fork point");
}

/// The expiry pass reads `fork_at`, so a PUT that queues a cut or a re-cut
/// writes it in the same transaction — not when (or if) the task finishes.
/// The recorded point is kept while the database still has it; an older one
/// cannot be cut at, and the sandbox says it forked now.
#[tokio::test]
async fn a_put_that_queues_a_cut_records_the_fork_point_with_it() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({})).await;
        let now = Utc::now();
        let hour_ago = now - Duration::hours(1);

        // No recorded point: now. A recorded one inside the window: kept.
        set_fork_at(&app, row.id, None).await;
        put_planes(row.id, ENVIRONMENT, json!({"oltp": "branch"})).await;
        assert!(close(
            fork_at_of(&app, row.id).await.expect("recorded"),
            now
        ));
        put_planes(row.id, ENVIRONMENT, json!({})).await;
        set_fork_at(&app, row.id, Some(hour_ago)).await;
        put_planes(row.id, ENVIRONMENT, json!({"oltp": "branch"})).await;
        assert!(close(
            fork_at_of(&app, row.id).await.expect("kept"),
            hour_ago
        ));
        let cuts: Vec<_> = queued(&app, row.id)
            .await
            .into_iter()
            .map(|(_, op)| op["op"].clone())
            .collect();
        assert_eq!(
            cuts,
            [json!("cut"), json!("cut")],
            "no branch row, so no drop between"
        );

        // Out of the window: cut now, and say so.
        let (id, handle) = (row.id, ENVIRONMENT);
        app_environments::Entity::update_many()
            .col_expr(app_environments::Column::Planes, Expr::value(json!({})))
            .filter(app_environments::Column::AppId.eq(id))
            .filter(app_environments::Column::Name.eq(handle))
            .exec(&app.t.db)
            .await
            .expect("back to schema");
        set_fork_at(&app, row.id, Some(now - Duration::days(3))).await;
        put_planes(row.id, ENVIRONMENT, json!({"oltp": "empty"})).await;
        assert!(close(
            fork_at_of(&app, row.id).await.expect("rewritten"),
            now
        ));
        let last = queued(&app, row.id).await.pop().expect("a cut").1;
        assert_eq!(last, json!({"op": "cut", "at": null}));
    })
    .await;
}
