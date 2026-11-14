//! Schedules outside production
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §4.5,
//! ruling 1), on the database and through the production executor:
//!
//! - an environment has schedule rows only while its switch is on, and its
//!   own publishes and flips keep them to its build; production's promote
//!   leaves them, and production keeps its own two-segment row;
//! - a row fires through the scheduler's tick into its environment, as the
//!   system, and the fire is not the sandbox's activity;
//! - the runner refuses a fire with the switch off or past a sandbox's idle
//!   expiry, writing no invocation;
//! - the teardown retires a sandbox's rows.
//!
//! What the tenant's schedules API does with an environment's row is
//! `env_schedules_tenant`'s.

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_pipeline::WorkflowWorkspaceContext;
use agentic_runtime::worker::TaskExecutor;
use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use entity::apps;
use oxy_api_custom_apps::app_function_executor::{APP_FUNCTION_KIND, AppFunctionTaskExecutor};
use oxy_api_custom_apps::custom_apps_env_schedules::handlers::{
    self as switch, EnvironmentSchedules, SwitchRequest,
};
use oxy_api_custom_apps::custom_apps_publish::{PublishTarget, publish_to};
use oxy_api_custom_apps::custom_apps_sandboxes::{TeardownReason, activity, ops};
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, invocations, publish_app, publish_build, seeded_tenant,
};
use crate::sandbox_publish::{app_row, bundle, input, sandbox};
use crate::staging_functions::make_guest_staff;

const APP: &str = "env-sched";
const TICK_JS: &str = "export default async (req, ctx) => Response.json({ channel: ctx.channel });";

/// A function on an hourly schedule.
pub(crate) fn tick() -> FunctionSpec {
    FunctionSpec {
        name: "tick",
        manifest: json!({ "schedule": "0 * * * *" }),
        js: TICK_JS,
    }
}

/// A route-only function: no schedule.
pub(crate) fn plain() -> FunctionSpec {
    FunctionSpec {
        name: "plain",
        manifest: json!({ "route": true }),
        js: TICK_JS,
    }
}

/// Publish `functions` as build `build_id` to the sandbox `handle`.
pub(crate) async fn publish_sandbox(
    t: &Tenant,
    slug: &str,
    build_id: &str,
    handle: &str,
    functions: &[FunctionSpec],
) {
    let tarball = bundle(slug, functions, json!({}), &[]);
    publish_to(
        input(t, slug, build_id, tarball),
        PublishTarget::Sandbox(sandbox(handle)),
    )
    .await
    .expect("publish to the sandbox");
}

/// An app whose production and staging builds schedule `tick`, the guest made
/// staff, and the sandbox `dev-a1` serving a build that schedules it too.
pub(crate) async fn scheduled_app(t: &Tenant, slug: &str) -> apps::Model {
    let app_id = publish_app(t, slug, demo_workspace_id(), &[tick(), plain()])
        .await
        .app_id;
    make_guest_staff();
    let app = app_row(&t.db, app_id).await;
    ops::create(&t.db, &app, &sandbox("a1"), &t.guest())
        .await
        .expect("create the sandbox");
    publish_sandbox(t, slug, "sbx-1", "a1", &[tick(), plain()]).await;
    app
}

/// The app's function schedule refs, sorted.
pub(crate) async fn rows(db: &DatabaseConnection, app: Uuid) -> Vec<String> {
    let mut refs: Vec<String> =
        agentic_pipeline::scheduler::list_schedules(db, demo_workspace_id())
            .await
            .expect("list the schedules")
            .into_iter()
            .filter(|s| s.target_kind == "function" && s.target_ref.starts_with(&app.to_string()))
            .map(|s| s.target_ref)
            .collect();
    refs.sort();
    refs
}

/// `PUT …/environments/{name}/schedules`, as the guest — staff.
pub(crate) async fn flip(
    t: &Tenant,
    app: Uuid,
    name: &str,
    enabled: bool,
) -> Result<EnvironmentSchedules, (StatusCode, Value)> {
    let path = Path((app, name.to_string()));
    let body = Json(SwitchRequest { enabled });
    match switch::set(RefuseSandboxAgent, t.guest(), None, path, body).await {
        Ok(Json(shown)) => Ok(shown),
        Err(refused) => {
            let response = refused.into_response();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("body");
            Err((
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            ))
        }
    }
}

/// One `app_function` task for `function` of `app`, run through the
/// production executor; the outcome it reports.
pub(crate) async fn run_task(
    db: &DatabaseConnection,
    app: Uuid,
    function: &str,
    environment: &str,
    trigger: &str,
) -> TaskOutcome {
    let spec = TaskSpec::Custom {
        kind: APP_FUNCTION_KIND.into(),
        payload: json!({
            "app_id": app.to_string(),
            "function_name": function,
            "trigger": trigger,
            "environment": environment,
        }),
    };
    run_spec(db, spec).await
}

pub(crate) async fn run_spec(db: &DatabaseConnection, spec: TaskSpec) -> TaskOutcome {
    let executor = AppFunctionTaskExecutor {
        db: db.clone(),
        preagg: Default::default(),
        executor: std::sync::Arc::new(
            oxy_api_custom_apps::data_plane::query::DataPlaneQueryExecutor,
        ),
    };
    let id = Uuid::new_v4().to_string();
    let assignment = TaskAssignment {
        task_id: id.clone(),
        parent_task_id: None,
        run_id: id,
        spec,
        policy: None,
        claim_epoch: agentic_core::NO_CLAIM,
    };
    let mut task = executor
        .execute(assignment)
        .await
        .expect("the task is read");
    task.outcomes.recv().await.expect("an outcome")
}

async fn execute(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect("execute");
}

pub(crate) fn sandbox_ref(app: Uuid) -> String {
    format!("{app}/dev-a1/tick")
}

/// Off by default; on, one row per scheduled function of the build the
/// environment serves, kept to it by the environment's own publishes; a
/// promote leaves it; off again, none.
#[tokio::test]
async fn a_sandbox_has_schedule_rows_only_while_its_switch_is_on() {
    let t = seeded_tenant().await;
    let app = scheduled_app(&t, APP).await;
    let production = format!("{}/tick", app.id);
    assert_eq!(
        rows(&t.db, app.id).await,
        vec![production.clone()],
        "off by default"
    );

    let shown = flip(&t, app.id, "dev-a1", true).await.expect("turn on");
    assert!(shown.enabled);
    let functions: Vec<&str> = shown
        .schedules
        .iter()
        .map(|s| s.function.as_str())
        .collect();
    assert_eq!(functions, vec!["tick"]);
    // Sorted: `<app>/dev-a1/tick` sorts before `<app>/tick`.
    let both = vec![sandbox_ref(app.id), production.clone()];
    assert_eq!(rows(&t.db, app.id).await, both);

    // Production's promote retires only production's rows.
    publish_build(&t, APP, demo_workspace_id(), "prod-2", true, &[tick()]).await;
    assert_eq!(
        rows(&t.db, app.id).await,
        both,
        "a promote leaves the sandbox's row"
    );

    // The sandbox's own publish keeps its rows to its build.
    publish_sandbox(&t, APP, "sbx-2", "a1", &[plain()]).await;
    assert_eq!(rows(&t.db, app.id).await, vec![production.clone()]);
    publish_sandbox(&t, APP, "sbx-3", "a1", &[tick()]).await;
    assert_eq!(rows(&t.db, app.id).await, both);

    let shown = flip(&t, app.id, "dev-a1", false).await.expect("turn off");
    assert!(!shown.enabled && shown.schedules.is_empty());
    assert_eq!(rows(&t.db, app.id).await, vec![production]);

    let (status, body) = flip(&t, app.id, "production", true)
        .await
        .expect_err("no switch");
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("production_has_no_switch"))
    );
    let (status, body) = flip(&t, app.id, "dev-zz", true)
        .await
        .expect_err("no sandbox");
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("environment_not_found"))
    );
}

/// Staging has a switch of its own; its rows follow staging's build, which
/// every publish moves.
#[tokio::test]
async fn staging_has_a_switch_of_its_own() {
    let t = seeded_tenant().await;
    let app = scheduled_app(&t, "env-sched-stg").await;
    flip(&t, app.id, "staging", true)
        .await
        .expect("turn staging on");
    let production = format!("{}/tick", app.id);
    let staging = format!("{}/staging/tick", app.id);
    assert_eq!(
        rows(&t.db, app.id).await,
        vec![staging.clone(), production.clone()]
    );
    // A draft that drops the schedule takes staging's row with it, and leaves
    // production's alone.
    publish_build(
        &t,
        "env-sched-stg",
        demo_workspace_id(),
        "stg-2",
        false,
        &[plain()],
    )
    .await;
    assert_eq!(rows(&t.db, app.id).await, vec![production]);
}

/// A due row fires through the scheduler's tick into its environment: the
/// sandbox's build runs as the sandbox, the run names the environment, and
/// the fire is not the sandbox's activity.
#[tokio::test]
async fn a_sandbox_row_fires_into_the_sandbox_and_is_not_activity() {
    let t = seeded_tenant().await;
    let app = scheduled_app(&t, "env-sched-fire").await;
    flip(&t, app.id, "dev-a1", true).await.expect("turn on");
    execute(
        &t.db,
        "UPDATE agentic_schedules SET next_run_at = now() - interval '1 minute' \
          WHERE target_ref = $1",
        vec![sandbox_ref(app.id).into()],
    )
    .await;

    let (platform, _dir) = crate::custom_app_functions_manual_run::platform().await;
    let workspace: Arc<dyn WorkflowWorkspaceContext> = platform.clone();
    let fired =
        agentic_pipeline::scheduler::tick_schedules(&t.db, demo_workspace_id(), workspace.as_ref())
            .await;
    assert!(fired >= 1, "the due row fired");
    let run_id = fired_run(&t.db, &sandbox_ref(app.id)).await;
    let metadata = run_metadata(&t.db, &run_id).await;
    assert_eq!(metadata["environment"], "dev-a1", "{metadata}");
    assert_eq!(metadata["trigger"], "scheduled");

    let spec = crate::sandbox_teardown_task::queued(&t.db, &run_id).await;
    let TaskOutcome::Done { answer, .. } = run_spec(&t.db, spec).await else {
        panic!("the fire ran");
    };
    let answer: Value = serde_json::from_str(&answer).expect("JSON");
    assert_eq!(answer["channel"], "dev-a1");
    let rows = invocations(&t.db, app.id, "tick").await;
    let seen: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.environment.as_str(), r.mode.as_str()))
        .collect();
    assert_eq!(seen, vec![("dev-a1", "schedule")], "in the sandbox only");
    assert_eq!(
        activity::last_invocation(&t.db, app.id, "dev-a1")
            .await
            .expect("read"),
        None,
        "a scheduled fire is not activity"
    );
}

/// The runner decides again: with the switch off, or past a sandbox's idle
/// expiry, a fire is refused and writes no invocation.
#[tokio::test]
async fn the_runner_refuses_a_fire_with_the_switch_off_or_past_expiry() {
    let t = seeded_tenant().await;
    let app = scheduled_app(&t, "env-sched-refuse").await;
    let failed = |outcome: TaskOutcome| match outcome {
        TaskOutcome::Failed(message) => message,
        other => panic!("a refused fire must not run: {other:?}"),
    };

    let off = failed(run_task(&t.db, app.id, "tick", "dev-a1", "scheduled").await);
    assert!(
        off.contains("schedules are off in the dev-a1 environment"),
        "{off}"
    );

    flip(&t, app.id, "dev-a1", true).await.expect("turn on");
    let not_scheduled = failed(run_task(&t.db, app.id, "plain", "dev-a1", "scheduled").await);
    assert!(
        not_scheduled.contains("declares no `schedule`"),
        "{not_scheduled}"
    );

    execute(
        &t.db,
        "UPDATE app_environments SET updated_at = now() - interval '30 days' \
          WHERE app_id = $1 AND name = 'dev-a1'",
        vec![app.id.into()],
    )
    .await;
    let expired = failed(run_task(&t.db, app.id, "tick", "dev-a1", "scheduled").await);
    assert!(expired.contains("past its idle expiry"), "{expired}");

    assert!(invocations(&t.db, app.id, "tick").await.is_empty());
    assert!(invocations(&t.db, app.id, "plain").await.is_empty());
}

/// Deleting a sandbox retires its rows with the rest of its homes;
/// production's stays.
#[tokio::test]
async fn the_teardown_retires_a_sandboxs_rows() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let t = seeded_tenant().await;
    let app = scheduled_app(&t, "env-sched-down").await;
    flip(&t, app.id, "dev-a1", true).await.expect("turn on");
    assert_eq!(rows(&t.db, app.id).await.len(), 2);

    let run_id = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1");
    let outcome = crate::sandbox_teardown_task::run_queued(&t.db, &run_id).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("the teardown failed: {outcome:?}");
    };
    assert!(answer.contains("1 schedules retired"), "{answer}");
    assert_eq!(rows(&t.db, app.id).await, vec![format!("{}/tick", app.id)]);
}

/// The run a tick seeded for `target_ref`.
async fn fired_run(db: &DatabaseConnection, target_ref: &str) -> String {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT last_run_id FROM agentic_schedules WHERE target_ref = $1",
        [target_ref.into()],
    ))
    .await
    .expect("read the row")
    .expect("the row")
    .try_get::<Option<String>>("", "last_run_id")
    .expect("last_run_id")
    .expect("the tick recorded its run")
}

async fn run_metadata(db: &DatabaseConnection, run_id: &str) -> Value {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT metadata FROM agentic_runs WHERE id = $1",
        [run_id.into()],
    ))
    .await
    .expect("read the run")
    .expect("the run")
    .try_get::<Option<Value>>("", "metadata")
    .expect("metadata")
    .unwrap_or(Value::Null)
}
