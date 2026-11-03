//! The OLTP plane's modes through the environment routes (Task 11): what
//! `POST …/environments`, `PUT …/planes` and `POST …/reset` refuse and queue,
//! and what the queued task leaves — through the console's guards, with the
//! real provisioner on the test cluster (`LocalProvider`).

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::{app_environments, apps};
use oxy_api_custom_apps::custom_apps_env_branch::task::{BranchOp, ENV_OLTP_BRANCH_KIND, run};
use oxy_oltp::entity::env_branches::{EnvBranchMode, EnvBranchStatus};
use oxy_oltp::env_branches::{count_for_org, find_for_app, mark_env_branch};
use oxy_oltp::resolver::{EnvBranchAccept, resolve_env_branch_writer_for_org};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::teardown::database_exists;
use super::{ENVIRONMENT, app_with_a_branch_sandbox, branch_of, run_queued, set_planes, task_in};
use crate::sandbox_oltp_isolation::functions;
use crate::sandbox_publish::sandbox;
use crate::sandbox_routes::send;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};
use agentic_core::delegation::TaskOutcome;

pub(super) const SIBLING: &str = "dev-b2";
pub(super) const CUT: BranchOp = BranchOp::Cut { at: None };

pub(super) async fn put_planes(app: Uuid, name: &str, planes: Value) -> (StatusCode, Value) {
    let path = format!("{app}/environments/{name}/planes");
    send("PUT", &path, Some(json!({ "planes": planes }))).await
}

pub(super) async fn create(app: Uuid, body: Value) -> (StatusCode, Value) {
    send("POST", &format!("{app}/environments"), Some(body)).await
}

pub(super) async fn reset(app: Uuid, name: &str, body: Value) -> (StatusCode, Value) {
    send(
        "POST",
        &format!("{app}/environments/{name}/reset"),
        Some(body),
    )
    .await
}

/// `(environment, op)` of every queued environment-branch task of the app.
pub(super) async fn queued(app: &OltpApp, app_id: Uuid) -> Vec<(String, Value)> {
    let rows = app
        .t
        .db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec->'payload'->>'environment' AS env, spec->'payload'->'op' AS op \
               FROM agentic_task_queue \
              WHERE queue_status = 'queued' AND spec->>'kind' = $1 \
                AND spec->'payload'->>'app_id' = $2 ORDER BY created_at, task_id",
            [ENV_OLTP_BRANCH_KIND.into(), app_id.to_string().into()],
        ))
        .await
        .expect("read the queue");
    rows.iter()
        .map(|r| {
            (
                r.try_get("", "env").expect("env"),
                r.try_get("", "op").expect("op"),
            )
        })
        .collect()
}

pub(super) fn done(outcomes: &[TaskOutcome]) -> Vec<String> {
    outcomes
        .iter()
        .map(|outcome| match outcome {
            TaskOutcome::Done { answer, .. } => answer.clone(),
            other => panic!("every queued task must succeed: {other:?}"),
        })
        .collect()
}

pub(super) async fn sandbox_row(
    app: &OltpApp,
    app_id: Uuid,
    name: &str,
) -> app_environments::Model {
    app_environments::Entity::find_by_id((app_id, name.to_string()))
        .one(&app.t.db)
        .await
        .expect("read the sandbox")
        .expect("the sandbox's row")
}

pub(super) fn close(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    (a - b).num_seconds().abs() <= 1
}

/// `select count(*) from orders` on the environment's branch.
pub(super) async fn orders_on_the_branch(app: &OltpApp, row: &apps::Model, env: &str) -> i64 {
    let writer = resolve_env_branch_writer_for_org(
        &app.t.db,
        app.t.org_id,
        row.id,
        env,
        &app.store.writer,
        EnvBranchAccept::Ready,
    )
    .await
    .expect("resolve the branch")
    .expect("the environment has a branch row");
    let client = oxy_oltp::connect::connect(&writer.connection.dsn, "env branch planes test")
        .await
        .expect("connect to the branch");
    let got = client.query_one("select count(*) from orders", &[]).await;
    got.expect("count").get(0)
}

#[tokio::test]
async fn the_cap_counts_rows_in_every_status() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var("OXY_APP_ENV_OLTP_BRANCHES_MAX", "2") };
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        set_planes(&app, &row, SIBLING, json!({"oltp": "branch"})).await;
        for env in [ENVIRONMENT, SIBLING] {
            run(&app.t.db, &task_in(&app, &row, env, CUT))
                .await
                .expect("cut");
        }
        // One ready, one failed: a failed row still holds a provider branch.
        let failed = find_for_app(&app.t.db, row.id, SIBLING)
            .await
            .unwrap()
            .unwrap();
        let why = Some("broken".to_string());
        mark_env_branch(&app.t.db, failed.id, EnvBranchStatus::Failed, why)
            .await
            .unwrap();
        assert_eq!(count_for_org(&app.t.db, app.t.org_id).await.unwrap(), 2);

        let c3 = sandbox("c3");
        let created = oxy_api_custom_apps::custom_apps_sandboxes::ops::create(
            &app.t.db,
            &row,
            &c3,
            &app.t.guest(),
        )
        .await;
        created.expect("create dev-c3");
        let (status, refused) = put_planes(row.id, "dev-c3", json!({"oltp": "branch"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["error"], "invalid_planes");
        let message = refused["message"].as_str().expect("a message");
        assert!(message.contains("has 2 sandbox OLTP branches"), "{message}");
        assert!(
            message.contains("allows (2)") && message.contains("oltp=schema"),
            "{message}"
        );
        let (status, refused) = create(
            row.id,
            json!({"name": "dev-d4", "planes": {"oltp": "empty"}}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

        // The environment's own row is not counted against it, and the cheap
        // mode is never refused.
        let (status, kept) = put_planes(row.id, ENVIRONMENT, json!({"oltp": "empty"})).await;
        assert_eq!(status, StatusCode::OK, "{kept}");
        let (status, cheap) = put_planes(row.id, "dev-c3", json!({"oltp": "schema"})).await;
        assert_eq!(status, StatusCode::OK, "{cheap}");
    })
    .await;
}

#[tokio::test]
async fn staging_refuses_oltp_branch() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({})).await;
        for mode in ["branch", "empty"] {
            let (status, refused) = put_planes(row.id, "staging", json!({"oltp": mode})).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{mode}: {refused}");
            assert_eq!(refused["error"], "invalid_planes");
            assert_eq!(
                refused["message"],
                "staging uses the org's OLTP staging branch; oltp modes branch and empty are \
                 for sandboxes"
            );
        }
    })
    .await;
}

#[tokio::test]
async fn a_create_with_preset_full_queues_a_cut() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({})).await;
        let at = Utc::now() - Duration::hours(2);
        let (status, plain) = create(row.id, json!({"name": "dev-c3", "preset": "full"})).await;
        assert_eq!(status, StatusCode::CREATED, "{plain}");
        assert_eq!(
            plain["planes_set"],
            json!({
                "oltp": "branch",
                "secrets": "empty",
                "airhouse": "branch",
                "messages": "capture",
                "workspace-actions": "preview"
            }),
            "the whole of PRESET_FULL"
        );
        let body = json!({"name": "dev-d4", "preset": "full", "at": at.to_rfc3339()});
        let (status, forked) = create(row.id, body).await;
        assert_eq!(status, StatusCode::CREATED, "{forked}");

        let ops = queued(&app, row.id).await;
        assert_eq!(ops.len(), 2, "{ops:?}");
        assert_eq!(
            ops[0],
            ("dev-c3".to_string(), json!({"op": "cut", "at": null}))
        );
        assert_eq!(ops[1].0, "dev-d4");
        let queued_at: DateTime<Utc> = ops[1].1["at"].as_str().expect("an at").parse().unwrap();
        assert!(close(queued_at, at), "{ops:?}");
        assert!(close(
            sandbox_row(&app, row.id, "dev-d4")
                .await
                .fork_at
                .unwrap()
                .into(),
            at
        ));

        let answers = done(&run_queued(&app.t.db, row.id).await);
        assert_eq!(answers.len(), 2, "{answers:?}");
        for env in ["dev-c3", "dev-d4"] {
            let cut = find_for_app(&app.t.db, row.id, env)
                .await
                .unwrap()
                .expect("a branch");
            assert_eq!(
                (cut.mode, cut.status),
                (EnvBranchMode::Branch, EnvBranchStatus::Ready)
            );
        }
        let cut = find_for_app(&app.t.db, row.id, "dev-d4")
            .await
            .unwrap()
            .unwrap();
        assert!(close(cut.fork_at.into(), at), "forked where --at said");
    })
    .await;
}

#[tokio::test]
pub(super) async fn put_planes_back_to_schema_queues_the_drop() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({})).await;
        let (status, put) = put_planes(row.id, ENVIRONMENT, json!({"oltp": "branch"})).await;
        assert_eq!(status, StatusCode::OK, "{put}");
        assert_eq!(
            queued(&app, row.id).await,
            [(ENVIRONMENT.to_string(), json!({"op": "cut", "at": null}))]
        );
        done(&run_queued(&app.t.db, row.id).await);
        let database = branch_of(&app, &row)
            .await
            .database_name
            .expect("a database");
        assert!(database_exists(&database).await);

        let (status, put) = put_planes(row.id, ENVIRONMENT, json!({"oltp": "schema"})).await;
        assert_eq!(status, StatusCode::OK, "{put}");
        assert_eq!(
            queued(&app, row.id).await,
            [(ENVIRONMENT.to_string(), json!({"op": "drop"}))]
        );
        let answers = done(&run_queued(&app.t.db, row.id).await);
        assert!(answers[0].contains("was deleted"), "{answers:?}");

        let gone = find_for_app(&app.t.db, row.id, ENVIRONMENT).await.unwrap();
        assert!(gone.is_none(), "the row goes with the provider branch");
        assert!(!database_exists(&database).await);
        // Nothing to drop the second time: nothing is queued.
        put_planes(row.id, ENVIRONMENT, json!({})).await;
        assert!(queued(&app, row.id).await.is_empty());
    })
    .await;
}
