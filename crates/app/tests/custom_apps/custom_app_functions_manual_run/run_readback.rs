//! Reading one function-job run back: `GET …/function-runs/{run_id}`.
//!
//! A run queued in a non-production environment carries the staging or
//! sandbox build's answer, error and log lines. It is read only by a caller
//! who may open the app's non-production environments:
//!
//! - anyone else holding the run id — a publish token included, whoever
//!   minted it — is answered not-found, as for an id that does not exist;
//! - a production run answers all three exactly as it always has.
//!
//! The handler is called directly with the extractors the router would build,
//! for the reason `callers` gives.

use agentic_pipeline::scheduler::{enqueue_app_function_job, enqueue_app_function_job_in};
use axum::Extension;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use oxy_app::server::api::admin::apps::functions as admin_functions;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AppPublishTokenAuth;
use serde_json::Value;
use uuid::Uuid;

use super::answer;
use super::callers::{outsider, publish_token, staff};
use super::environment_checks::two_builds;
use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};

/// A queued run of `smoke` for `app_id`, in `environment` (production when
/// `None`), exactly as the trigger route seeds one. Nothing drives it: the
/// read-back of a queued run is what is under test.
async fn queued_run(t: &Tenant, app_id: Uuid, environment: Option<&str>) -> String {
    let (app, ws) = (app_id.to_string(), demo_workspace_id());
    let queued = match environment {
        None => {
            enqueue_app_function_job(&t.db, &app, "smoke", ws, None, "manual", None, None).await
        }
        Some(name) => {
            enqueue_app_function_job_in(
                &t.db,
                &app,
                "smoke",
                ws,
                None,
                "manual",
                None,
                None,
                Some(name),
            )
            .await
        }
    };
    queued.expect("queue the run")
}

/// `get_function_run`, called as the router would with these extractors.
async fn read_as(
    caller: AuthenticatedUserExtractor,
    token: Option<Extension<AppPublishTokenAuth>>,
    app_id: Uuid,
    run_id: &str,
) -> (StatusCode, Value) {
    let path = Path((app_id, run_id.to_string()));
    match admin_functions::get_function_run(caller, token, path).await {
        Ok(axum::Json(run)) => (
            StatusCode::OK,
            serde_json::to_value(run).expect("the run serializes"),
        ),
        Err(refused) => answer(refused.into_response()).await,
    }
}

#[tokio::test]
async fn a_run_outside_production_is_read_only_by_a_caller_with_reach() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t).await;

    for environment in ["staging", "dev-a1"] {
        let run_id = queued_run(&t, app_id, Some(environment)).await;

        // The tenant's own app admin, holding the id: the run does not exist.
        let (status, body) = read_as(outsider(), None, app_id, &run_id).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{environment}: {body}");
        assert_eq!(
            body,
            Value::Null,
            "{environment}: the same bare not-found an unknown run id gets"
        );

        // A publish token gets the same answer, though it rides staff's own
        // login: it named a run id, not an environment, and a coded refusal
        // would confirm the run and say where it ran.
        let token = publish_token(app_id);
        let (status, body) = read_as(staff(&t), token, app_id, &run_id).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{environment}: {body}");
        assert_eq!(body, Value::Null, "{environment}: a bare not-found");

        // Staff with reach, on their own login, read it.
        let (status, body) = read_as(staff(&t), None, app_id, &run_id).await;
        assert_eq!(status, StatusCode::OK, "{environment}: {body}");
        assert_eq!(body["run_id"], run_id.as_str());
        assert_eq!(body["environment"], environment);
    }

    let (status, body) = read_as(outsider(), None, app_id, &Uuid::new_v4().to_string()).await;
    assert_eq!(
        (status, body),
        (StatusCode::NOT_FOUND, Value::Null),
        "an unknown run id is the answer a hidden run gives"
    );
}

/// A production run is not a non-production row: no second door. Every caller
/// the mount let through reads it, and reads the same thing.
#[tokio::test]
async fn a_production_run_reads_back_for_every_caller_as_before() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t).await;
    let run_id = queued_run(&t, app_id, None).await;

    let (status, as_staff) = read_as(staff(&t), None, app_id, &run_id).await;
    assert_eq!(status, StatusCode::OK, "{as_staff}");
    assert_eq!(as_staff["environment"], "production");
    assert_eq!(as_staff["run_id"], run_id.as_str());

    let token = publish_token(app_id);
    for (who, caller, token) in [
        ("no reach", outsider(), None),
        ("a publish token", staff(&t), token),
    ] {
        let (status, body) = read_as(caller, token, app_id, &run_id).await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
        assert_eq!(body, as_staff, "{who}");
    }
}
