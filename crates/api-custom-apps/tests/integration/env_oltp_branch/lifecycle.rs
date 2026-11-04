//! What a change of planes, a failed run and the sweep leave behind:
//!
//! - **turning on an Airhouse mode that copies** starts the sandbox's 30 days
//!   now, unless it already held a copy, whose clock stands;
//! - **entering `snapshot`** pins the snapshot at now, not at a pin left from
//!   an earlier stretch on it;
//! - **`env show`** says when a sandbox is deleted for its copies
//!   (`copies_expire_at`), leftover overlay tables included;
//! - **`env show` says a re-cut is queued** from the queue itself, not from
//!   the fork points disagreeing, which an Airhouse change also makes them;
//! - **a reset whose run fails** puts the fork point of the branch the sandbox
//!   still has back;
//! - **a switch back to `schema`** queues the schema task for the build the
//!   sandbox serves;
//! - **a branch no sandbox asks for** — its own `Drop` failed — is dropped by
//!   the sweep, and one whose planes cannot be read is left alone.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::apps;
use oxy_api_custom_apps::custom_apps_env_branch::task::run;
use oxy_api_custom_apps::custom_apps_sandboxes::maintenance::sweep;
use oxy_api_custom_apps::custom_apps_sandboxes::oltp_task::SANDBOX_OLTP_KIND;
use oxy_oltp::env_branches::find_for_app;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::planes::{SIBLING, close, put_planes, queued, reset, sandbox_row};
use super::teardown::{is_deleting, overlay_row, set_fork_at};
use super::{
    CUT, ENVIRONMENT, HANDLE, app_with_a_branch_sandbox, branch_of, run_queued, set_planes, task_in,
};
use crate::sandbox_oltp_isolation::{functions, publish};
use crate::sandbox_routes::send;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

fn fork_at(row: &entity::app_environments::Model) -> DateTime<Utc> {
    row.fork_at.expect("a fork point").to_utc()
}

/// `airhouse_pinned_at` of sandbox `name`.
async fn pinned_at(app: &OltpApp, app_id: Uuid, name: &str) -> Option<DateTime<Utc>> {
    let row = app
        .t
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT airhouse_pinned_at FROM app_environments WHERE app_id = $1 AND name = $2",
            [app_id.into(), name.into()],
        ))
        .await
        .expect("read the pin")
        .expect("the sandbox's row");
    row.try_get("", "airhouse_pinned_at")
        .expect("the pin column")
}

async fn set_pin(app: &OltpApp, app_id: Uuid, name: &str, at: DateTime<Utc>) {
    app.t
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_environments SET airhouse_pinned_at = $3 WHERE app_id = $1 AND name = $2",
            [app_id.into(), name.into(), at.fixed_offset().into()],
        ))
        .await
        .expect("set the pin");
}

/// Sandbox `name` as `GET …/environments` lists it.
async fn listed(app_id: Uuid, name: &str) -> Value {
    let (status, body) = send("GET", &format!("{app_id}/environments"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["environments"]
        .as_array()
        .expect("environments")
        .iter()
        .find(|e| e["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("{name} is listed: {body}"))
}

fn instant(value: &Value) -> DateTime<Utc> {
    value
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .unwrap_or_else(|| panic!("an RFC 3339 instant, not {value}"))
        .to_utc()
}

/// An old sandbox that copied nothing starts its 30 days when it turns on
/// `airhouse=branch`; before this the next sweep deleted it. One that already
/// held a copy keeps the clock of that copy, and is deleted for it.
#[tokio::test]
async fn turning_on_an_airhouse_copy_starts_its_clock_now() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({})).await;
        let long_ago = Utc::now() - Duration::days(40);
        for name in [ENVIRONMENT, SIBLING] {
            set_fork_at(&app, row.id, name, long_ago).await;
        }
        overlay_row(&app, &row, SIBLING, "shadow").await;
        assert!(listed(row.id, ENVIRONMENT).await["copies_expire_at"].is_null());

        for name in [ENVIRONMENT, SIBLING] {
            let (status, put) = put_planes(row.id, name, json!({"airhouse": "branch"})).await;
            assert_eq!(status, StatusCode::OK, "{put}");
        }

        let now = Utc::now();
        assert!(close(
            fork_at(&sandbox_row(&app, row.id, ENVIRONMENT).await),
            now
        ));
        let kept = fork_at(&sandbox_row(&app, row.id, SIBLING).await);
        assert!(close(kept, long_ago), "the older copy's clock stands");
        let shown = instant(&listed(row.id, ENVIRONMENT).await["copies_expire_at"]);
        assert!(close(shown, now + Duration::days(30)), "{shown}");

        sweep(&app.t.db, now).await.expect("sweep");
        assert!(
            !is_deleting(&app, row.id, ENVIRONMENT).await,
            "a fresh copy"
        );
        assert!(is_deleting(&app, row.id, SIBLING).await, "the control");
    })
    .await;
}

/// `env show` says when leftover overlay tables get a write-only sandbox
/// deleted — the console showed it nothing.
#[tokio::test]
async fn env_show_dates_leftover_copies() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({})).await;
        let forked = Utc::now() - Duration::days(3);
        set_fork_at(&app, row.id, SIBLING, forked).await;
        overlay_row(&app, &row, SIBLING, "partial").await;
        overlay_row(&app, &row, ENVIRONMENT, "dropped").await;

        let shown = instant(&listed(row.id, SIBLING).await["copies_expire_at"]);
        assert!(close(shown, forked + Duration::days(30)), "{shown}");
        assert!(listed(row.id, ENVIRONMENT).await["copies_expire_at"].is_null());
    })
    .await;
}

/// A pin left from an earlier stretch on `snapshot` would read production
/// days back and be refused; entering `snapshot` pins now.
#[tokio::test]
async fn entering_snapshot_pins_now() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({})).await;
        let stale = Utc::now() - Duration::days(2);
        set_pin(&app, row.id, ENVIRONMENT, stale).await;
        set_pin(&app, row.id, SIBLING, stale).await;
        set_planes(&app, &row, SIBLING, json!({"airhouse": "snapshot"})).await;

        for name in [ENVIRONMENT, SIBLING] {
            let (status, put) = put_planes(row.id, name, json!({"airhouse": "snapshot"})).await;
            assert_eq!(status, StatusCode::OK, "{put}");
        }

        let pinned = pinned_at(&app, row.id, ENVIRONMENT).await.expect("a pin");
        assert!(close(pinned, Utc::now()), "{pinned}");
        let kept = pinned_at(&app, row.id, SIBLING).await.expect("a pin");
        assert!(close(kept, stale), "already on snapshot: its pin stands");
    })
    .await;
}

/// A reset moves the fork point when it queues. A run that fails — here the
/// provisioner is gone — puts the branch's own point back, so the old copy
/// is not given 30 more days.
#[tokio::test]
async fn a_failed_reset_puts_the_fork_point_back() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        run(&app.t.db, &task_in(&app, &row, ENVIRONMENT, CUT))
            .await
            .expect("cut");
        let cut_at = branch_of(&app, &row).await.fork_at.to_utc();
        let long_ago = cut_at - Duration::days(20);
        set_fork_at(&app, row.id, ENVIRONMENT, long_ago).await;
        age_branch_fork(&app, &row, long_ago).await;
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::remove_var("OXY_OLTP_PROVIDER") };

        let (status, body) = reset(row.id, ENVIRONMENT, json!({"planes": ["oltp"]})).await;
        assert!(status.is_success(), "{status}: {body}");
        let moved = fork_at(&sandbox_row(&app, row.id, ENVIRONMENT).await);
        assert!(
            close(moved, Utc::now()),
            "the reset moved it when it queued"
        );

        let outcomes = run_queued(&app.t.db, row.id).await;
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        assert!(
            matches!(
                outcomes[0],
                agentic_core::delegation::TaskOutcome::Failed { .. }
            ),
            "{outcomes:?}"
        );
        let back = fork_at(&sandbox_row(&app, row.id, ENVIRONMENT).await);
        assert!(close(back, long_ago), "{back} is the branch's point again");
    })
    .await;
}

/// A queued re-cut is read off the queue. Turning on `airhouse=branch` for a
/// sandbox on an `empty` branch moves its fork point and queues no cut, so the
/// console's old reading (the two fork points differ) said "re-cut queued"
/// forever.
#[tokio::test]
async fn env_show_reports_a_queued_recut_from_the_queue() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "empty"})).await;
        run(&app.t.db, &task_in(&app, &row, ENVIRONMENT, CUT))
            .await
            .expect("cut");
        let recut = |env: &Value| env["oltp_branch"]["recut_queued"].clone();

        let (status, put) = put_planes(
            row.id,
            ENVIRONMENT,
            json!({"oltp": "empty", "airhouse": "branch"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{put}");
        let env = listed(row.id, ENVIRONMENT).await;
        assert_eq!(env["oltp_branch"]["status"], "ready", "{env}");
        assert_ne!(
            instant(&env["fork_at"]),
            instant(&env["oltp_branch"]["fork_at"]),
            "the case: the fork points differ with nothing queued"
        );
        assert_eq!(recut(&env), json!(false), "{env}");

        let (status, body) = reset(row.id, ENVIRONMENT, json!({"planes": ["oltp"]})).await;
        assert!(status.is_success(), "{status}: {body}");
        assert_eq!(recut(&listed(row.id, ENVIRONMENT).await), json!(true));

        run_queued(&app.t.db, row.id).await;
        assert_eq!(recut(&listed(row.id, ENVIRONMENT).await), json!(false));
    })
    .await;
}

/// The branch row's fork point, as an older cut left it.
async fn age_branch_fork(app: &OltpApp, row: &apps::Model, at: DateTime<Utc>) {
    app.t
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE oltp_env_branches SET fork_at = $3 WHERE app_id = $1 AND env_name = $2",
            [row.id.into(), ENVIRONMENT.into(), at.fixed_offset().into()],
        ))
        .await
        .expect("age the branch");
}

/// Builds queued for sandbox `name`'s own schema.
async fn schema_tasks(app: &OltpApp, app_id: Uuid, name: &str) -> usize {
    let row = app
        .t
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM agentic_task_queue \
              WHERE queue_status = 'queued' AND spec->>'kind' = $1 \
                AND spec->'payload'->>'app_id' = $2 AND spec->'payload'->>'environment' = $3",
            [
                SANDBOX_OLTP_KIND.into(),
                app_id.to_string().into(),
                name.into(),
            ],
        ))
        .await
        .expect("read the queue")
        .expect("a count");
    row.try_get::<i64>("", "n").expect("n") as usize
}

/// While on its own branch no publish migrated the sandbox's schema, so a
/// switch back to `schema` queues it for the build the sandbox serves.
#[tokio::test]
async fn a_switch_back_to_schema_queues_the_schema() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        // The org's staging branch, which a sandbox's own schema lives in.
        app.store.provision_branch().await;
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(&app, HANDLE, "a-1", &[]).await;
        run_queued(&app.t.db, row.id).await;
        assert_eq!(schema_tasks(&app, row.id, ENVIRONMENT).await, 0);

        let (status, put) = put_planes(row.id, ENVIRONMENT, json!({"oltp": "empty"})).await;
        assert_eq!(status, StatusCode::OK, "{put}");
        assert_eq!(
            schema_tasks(&app, row.id, ENVIRONMENT).await,
            0,
            "still a branch"
        );

        let (status, put) = put_planes(row.id, ENVIRONMENT, json!({"oltp": "schema"})).await;
        assert_eq!(status, StatusCode::OK, "{put}");
        assert_eq!(schema_tasks(&app, row.id, ENVIRONMENT).await, 1);
    })
    .await;
}

async fn age_branch(app: &OltpApp, row: &apps::Model, name: &str, by: Duration) {
    app.t
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE oltp_env_branches SET updated_at = now() - make_interval(secs => $3) \
              WHERE app_id = $1 AND env_name = $2",
            [row.id.into(), name.into(), (by.num_seconds() as f64).into()],
        ))
        .await
        .expect("age the branch row");
}

/// A sandbox moved to `schema` whose own `Drop` failed keeps its branch; the
/// sweep queues the drop again once the branch has sat still. One whose
/// planes cannot be read is left alone, and so is one that only just moved.
#[tokio::test]
async fn the_sweep_drops_a_branch_no_sandbox_asks_for() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        set_planes(&app, &row, SIBLING, json!({"oltp": "branch"})).await;
        for name in [ENVIRONMENT, SIBLING] {
            run(&app.t.db, &task_in(&app, &row, name, CUT))
                .await
                .expect("cut");
        }
        // As a write whose drop failed leaves them.
        set_planes(&app, &row, ENVIRONMENT, json!({"oltp": "schema"})).await;
        set_planes(&app, &row, SIBLING, json!({"oltp": "bogus"})).await;
        age_branch(&app, &row, SIBLING, Duration::hours(7)).await;

        sweep(&app.t.db, Utc::now()).await.expect("sweep");
        assert!(
            queued(&app, row.id).await.is_empty(),
            "nothing has sat still yet"
        );

        age_branch(&app, &row, ENVIRONMENT, Duration::hours(7)).await;
        sweep(&app.t.db, Utc::now()).await.expect("sweep");
        assert_eq!(
            queued(&app, row.id).await,
            [(ENVIRONMENT.to_string(), json!({"op": "drop"}))]
        );
        run_queued(&app.t.db, row.id).await;
        let gone = find_for_app(&app.t.db, row.id, ENVIRONMENT).await.unwrap();
        assert!(gone.is_none(), "dropped");
        let kept = find_for_app(&app.t.db, row.id, SIBLING).await.unwrap();
        assert!(kept.is_some(), "unreadable planes keep theirs");
    })
    .await;
}
