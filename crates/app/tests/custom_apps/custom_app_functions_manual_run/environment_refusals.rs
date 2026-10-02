//! Every refusal around a check run in a named app environment.
//!
//! - refused by the route, each with its code and with nothing queued: a
//!   function the environment's build does not mark a check, a name that is
//!   not an environment, an environment with no build, an unknown function, a
//!   caller without reach, and a publish token;
//! - refused by the mount: a staff grant scoped to another org is answered
//!   not-found on the routes added for environments, as on every admin route;
//! - refused by the runner: a queued task that names staging for a function
//!   that is not a check, whoever queued it — the flag is read from the build
//!   the environment serves, not from production's.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use oxy_app::server::api::admin::apps::environment_scope::EnvironmentQuery;
use oxy_app::server::api::admin::apps::{functions as admin_functions, handlers};
use oxy_app::server::app_function_executor::{APP_FUNCTION_KIND, AppFunctionTaskExecutor};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use agentic_pipeline::scheduler::enqueue_app_function_job;

use super::callers::{environment, outsider, publish_token, staff, user};
use super::environment_checks::two_builds;
use super::{answer, get_admin_as, post_admin};
use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, invocations, seeded_tenant, throwaway_org};

/// How many function-job runs are recorded for `function` of `app_id`: one per
/// accepted trigger, whether or not a worker has picked it up.
async fn queued_runs(t: &Tenant, app_id: Uuid, function: &str) -> i64 {
    t.db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*) AS n FROM agentic_runs WHERE question = $1",
        [format!("fn:{app_id}/{function}").into()],
    ))
    .await
    .expect("count runs")
    .expect("a count")
    .try_get::<i64>("", "n")
    .expect("n")
}

/// `run_function_job`, called as the router would with these extractors.
async fn run_as(
    caller: AuthenticatedUserExtractor,
    token: Option<Extension<AppPublishTokenAuth>>,
    app_id: Uuid,
    function: &str,
    query: Query<EnvironmentQuery>,
) -> (StatusCode, Value) {
    let outcome = handlers::run_function_job(
        caller,
        token,
        Path((app_id, function.to_string())),
        query,
        Bytes::new(),
    )
    .await;
    match outcome {
        Ok(axum::Json(run)) => (
            StatusCode::OK,
            serde_json::to_value(run).expect("the response serializes"),
        ),
        Err(refused) => answer(refused.into_response()).await,
    }
}

#[tokio::test]
async fn a_run_outside_production_is_refused_unless_it_is_a_check_someone_with_reach_asked_for() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t).await;
    let runs =
        |function: &str, query: &str| format!("/apps/{app_id}/functions/{function}/runs{query}");

    // Not a check in the build staging serves — including one production's
    // build still marks a check.
    for function in ["plain", "was-a-check"] {
        let (status, body) = post_admin(&runs(function, "?environment=staging")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{function}: {body}");
        assert_eq!(body["error"], "not_a_check", "{function}: {body}");
        assert_eq!(
            queued_runs(&t, app_id, function).await,
            0,
            "{function} was queued"
        );
    }

    let (status, body) = post_admin(&runs("smoke", "?environment=Staging")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_environment");

    // An environment that serves nothing, and a function staging does not have.
    let (status, body) = post_admin(&runs("smoke", "?environment=dev-a1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "environment_has_no_build");
    let (status, body) = post_admin(&runs("missing", "?environment=staging")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "function_not_found");

    // A caller who may not open the app's non-production environments.
    let (status, body) = run_as(outsider(), None, app_id, "smoke", environment("staging")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "non_production_refused");

    // A publish token, even one its staff minter could have used staging with,
    // and even for a declared check.
    let token = || publish_token(app_id);
    for name in ["staging", "dev-a1"] {
        let (status, body) = run_as(staff(&t), token(), app_id, "smoke", environment(name)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{name}: {body}");
        assert_eq!(body["error"], "publish_token_refused", "{name}: {body}");
    }
    // A token scoped to another app gets one bare answer whether or not the
    // app it names exists, so it learns nothing about either.
    for target in [app_id, Uuid::new_v4()] {
        let foreign = publish_token(Uuid::new_v4());
        let (status, body) =
            run_as(staff(&t), foreign, target, "smoke", environment("staging")).await;
        assert_eq!(
            (status, body),
            (StatusCode::FORBIDDEN, Value::Null),
            "{target}"
        );
    }
    assert_eq!(
        queued_runs(&t, app_id, "smoke").await,
        0,
        "no refused request queued a run"
    );

    // The same listing rule: a non-production build's functions are not a
    // publish token's or an outsider's to read.
    let list = |caller, token| async move {
        admin_functions::list_functions(caller, token, Path(app_id), environment("staging")).await
    };
    let (status, body) = answer(
        list(staff(&t), token())
            .await
            .map(|_| ())
            .unwrap_err()
            .into_response(),
    )
    .await;
    assert_eq!(
        (status, &body["error"]),
        (StatusCode::FORBIDDEN, &json!("publish_token_refused"))
    );
    let (status, body) = answer(
        list(outsider(), None)
            .await
            .map(|_| ())
            .unwrap_err()
            .into_response(),
    )
    .await;
    assert_eq!(
        (status, &body["error"]),
        (StatusCode::FORBIDDEN, &json!("non_production_refused"))
    );

    // What a publish token could always do is unchanged: run a declared check
    // in production, and nothing that is not one.
    let (status, body) = run_as(
        staff(&t),
        token(),
        app_id,
        "smoke",
        Query(EnvironmentQuery::default()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["environment"], "production");
    let (status, _) = run_as(
        staff(&t),
        token(),
        app_id,
        "plain",
        environment("production"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a token runs checks only");
}

/// The runner applies the rule itself: a task that names staging for a
/// function staging's build does not mark a check is refused, however it got
/// on the queue — and the flag is read from staging's build, so a function
/// that is a check only in production's is refused too.
#[tokio::test]
async fn a_queued_task_naming_staging_for_a_function_that_is_not_a_check_is_refused() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t).await;
    let executor = AppFunctionTaskExecutor {
        db: t.db.clone(),
        preagg: Default::default(),
    };
    for function in ["plain", "was-a-check"] {
        let assignment = TaskAssignment {
            task_id: format!("t-{function}"),
            parent_task_id: None,
            run_id: format!("r-{function}"),
            spec: TaskSpec::Custom {
                kind: APP_FUNCTION_KIND.into(),
                payload: json!({
                    "app_id": app_id.to_string(),
                    "function_name": function,
                    "trigger": "manual",
                    "environment": "staging",
                }),
            },
            policy: None,
        };
        let mut task = executor
            .execute(assignment)
            .await
            .expect("the task is read");
        match task.outcomes.recv().await.expect("an outcome") {
            // The refusal `check_run::admit_queued` gives, not any failure
            // that happens to name the environment (no build, a lookup error).
            TaskOutcome::Failed(message) => assert!(
                message.contains("only a route call runs a function in the staging environment"),
                "{function}: refused, but not as a queued run outside production: {message}"
            ),
            other => panic!("{function}: a non-check staging task must not run: {other:?}"),
        }
        assert!(
            invocations(&t.db, app_id, function).await.is_empty(),
            "{function}: the runner refused before writing an invocation"
        );
    }
}

/// An App Operator — staff who ship custom apps and nothing else — whose
/// grant names `org` and no other.
async fn operator_scoped_to(t: &Tenant, org: Uuid) -> AuthenticatedUser {
    use entity::{app_admin_scope_orgs, app_admins};
    use sea_orm::{ActiveModelTrait, ActiveValue};
    let email = format!("operator-{}@oxy.example", Uuid::new_v4());
    let grant = Uuid::new_v4();
    app_admins::ActiveModel {
        id: ActiveValue::Set(grant),
        email: ActiveValue::Set(email.clone()),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        role: ActiveValue::Set("app_operator".to_string()),
        scope_all: ActiveValue::Set(false),
        updated_at: ActiveValue::NotSet,
    }
    .insert(&t.db)
    .await
    .expect("seed the grant");
    app_admin_scope_orgs::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_admin_id: ActiveValue::Set(grant),
        org_id: ActiveValue::Set(org),
        created_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(&t.db)
    .await
    .expect("scope the grant");
    let AuthenticatedUserExtractor(operator) = user(Uuid::new_v4(), &email);
    operator
}

/// The routes added for environments sit behind the mount's own fence like
/// every admin route: staff whose grant names another org are answered
/// not-found — the mount's, before any handler runs — whether or not they
/// name an environment. An operator whose grant names the app's org is let
/// through the same stack, so it is the scope that refused. (Two operators,
/// not one re-scoped: a grant is cached per email for a minute.)
#[tokio::test]
async fn staff_scoped_to_another_org_get_not_found_on_the_environment_routes() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t).await;
    let elsewhere = throwaway_org(&t).await;
    let out_of_scope = operator_scoped_to(&t, elsewhere.org_id).await;
    let in_scope = operator_scoped_to(&t, t.org_id).await;

    let run = enqueue_app_function_job(
        &t.db,
        &app_id.to_string(),
        "smoke",
        demo_workspace_id(),
        None,
        "manual",
        None,
        None,
    )
    .await
    .expect("queue a production run");
    let run_detail = format!("/apps/{app_id}/function-runs/{run}");
    let listing = format!("/apps/{app_id}/invocations");
    let held = format!("/apps/{app_id}/invocations/{}/held", Uuid::new_v4());
    let paths = [
        run_detail.clone(),
        listing.clone(),
        format!("{listing}?environment=staging"),
        format!("/apps/{app_id}/functions/smoke/invocations?environment=staging"),
        held.clone(),
    ];
    for path in &paths {
        let (status, body) = get_admin_as(&out_of_scope, path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(
            body,
            Value::Null,
            "{path}: the mount refused; a handler's not-found carries a code"
        );
    }

    let (status, body) = get_admin_as(&in_scope, &listing).await;
    assert_eq!(status, StatusCode::OK, "in scope: {body}");
    assert_eq!(body["invocations"], json!([]));
    let (status, body) = get_admin_as(&in_scope, &run_detail).await;
    assert_eq!(status, StatusCode::OK, "in scope: {body}");
    assert_eq!(body["run_id"], run.as_str());
    let (status, body) = get_admin_as(&in_scope, &held).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        body["error"], "invocation_not_found",
        "in scope, the handler runs and gives its own not-found"
    );
}
