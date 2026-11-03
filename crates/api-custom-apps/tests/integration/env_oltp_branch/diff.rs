//! `GET …/environments/{name}/diff` (Task 10) through the staff console's
//! guards, against a branch `LocalProvider` copied for real: a sandbox
//! migration shows as exactly the column it added, a branch not yet usable is
//! refused with the fix `ctx.oltp` names, and an environment with no branch of
//! its own has no plane to show. Production is read, never written.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{ENVIRONMENT, HANDLE, app_with_a_branch_sandbox, run_queued};
use crate::sandbox_oltp_isolation::{functions, publish};
use crate::sandbox_routes::send;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

const ADD_NOTE: &[u8] = b"alter table orders add column note text default 'n';";

/// `GET …/environments/{name}/diff` as Oxy staff.
async fn diff(app: Uuid, name: &str) -> (StatusCode, Value) {
    // SAFETY: nextest runs each test in its own process; set before any
    // console request.
    unsafe {
        std::env::set_var("OXY_OWNER", oxy_auth::user::LOCAL_GUEST_EMAIL);
    }
    send("GET", &format!("{app}/environments/{name}/diff"), None).await
}

fn all_empty_but(oltp: &Value, list: &str) -> bool {
    [
        "tables_added",
        "tables_removed",
        "columns_added",
        "columns_removed",
        "columns_changed",
    ]
    .into_iter()
    .filter(|key| *key != list)
    .all(|key| oltp[key] == json!([]))
}

#[tokio::test]
async fn env_diff_reports_a_branch_migration() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        publish(
            &app,
            HANDLE,
            "a-1",
            &[("migrations/0001_note.sql", ADD_NOTE)],
        )
        .await;
        run_queued(&app.t.db, row.id).await;

        let (status, body) = diff(row.id, ENVIRONMENT).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["environment"], ENVIRONMENT);
        let oltp = &body["planes"]["oltp"];
        assert_eq!(oltp["columns_added"], json!(["orders.note text"]), "{body}");
        assert!(all_empty_but(oltp, "columns_added"), "{body}");
        assert!(
            oltp["schema"].as_str().is_some_and(|s| !s.is_empty()),
            "{body}"
        );
        assert_eq!(app.order_count().await, 1, "production is untouched");

        // Production has no such column: the diff did not write to it.
        let (_, again) = diff(row.id, ENVIRONMENT).await;
        assert_eq!(again, body, "reading twice changes nothing");
    })
    .await;
}

#[tokio::test]
async fn env_diff_of_a_branch_not_cut_is_refused_with_the_fix() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        let (status, body) = diff(row.id, ENVIRONMENT).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "oltp_branch_unready");
        let message = body["message"].as_str().unwrap_or_default();
        assert!(message.contains("has not been cut"), "{body}");
    })
    .await;
}

#[tokio::test]
async fn env_diff_of_an_environment_with_no_branch_has_no_planes() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app_with_a_branch_sandbox(&app, json!({"oltp": "branch"})).await;
        // `dev-b2` keeps the default, `schema`; staging and production have
        // no branch of their own.
        for name in ["dev-b2", "staging", "production"] {
            let (status, body) = diff(row.id, name).await;
            assert_eq!(status, StatusCode::OK, "{name}: {body}");
            assert_eq!(body, json!({"environment": name, "planes": {}}));
        }
        let (status, body) = diff(row.id, "dev-zz").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    })
    .await;
}
