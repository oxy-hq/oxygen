//! After production's pointer moves: a production attempt of the build it
//! moved to, and the runner started for it with `environment: production`.

use agentic_core::delegation::{TaskOutcome, TaskSpec};
use oxy_api_custom_apps::custom_apps_build_checks::BUILD_CHECKS_KIND;
use oxy_api_custom_apps::custom_apps_build_checks::executor::run_guarded;
use oxy_api_custom_apps::custom_apps_build_checks::task::BuildChecksTask;
use oxy_app_core::config::AppChecksConfig;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::promote_rollback::{fixture_builds, pointers, rollback};

/// The app's queued production attempts, oldest first, as `(build, attempt)`.
async fn production_tasks(agent: &Agent) -> Vec<BuildChecksTask> {
    let rows = agent
        .t
        .db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec FROM agentic_task_queue \
             WHERE spec->>'kind' = $1 AND spec->'payload'->>'app_id' = $2 \
               AND spec->'payload'->>'environment' = 'production' \
             ORDER BY created_at, task_id",
            [BUILD_CHECKS_KIND.into(), agent.app.id.to_string().into()],
        ))
        .await
        .expect("read the queue");
    rows.iter()
        .map(|row| {
            let spec: Value = row.try_get("", "spec").expect("spec");
            let spec: TaskSpec = serde_json::from_value(spec).expect("a TaskSpec");
            BuildChecksTask::from_spec(&spec).expect("a build checks task")
        })
        .collect()
}

fn attempts(tasks: &[BuildChecksTask]) -> Vec<(Uuid, i32)> {
    tasks
        .iter()
        .map(|task| (task.build_pk, task.attempt))
        .collect()
}

/// `POST /customer-apps/{id}/publish`: the console's Make Live of the draft.
async fn make_live(agent: &Agent) -> StatusCode {
    let uri = format!("/customer-apps/{}/publish", agent.app.id);
    person(agent, "POST", &uri, None).await.0
}

/// Every writer of production's pointer: the promote route, the rollback
/// route, Make Live, the console's rollback and a promoting publish. Each
/// move to another build queues that build's next production attempt; a
/// re-stamp of the build production already serves queues nothing.
#[tokio::test]
async fn every_move_of_production_to_another_build_queues_a_production_attempt() {
    let agent = gated().await;
    let (prod, stg) = fixture_builds(&agent).await;
    // Nothing is required, so no move below is held by a check.
    settings(&agent, json!({ "required": [] })).await;
    assert!(production_tasks(&agent).await.is_empty());

    promote(&agent).await;
    assert_eq!(attempts(&production_tasks(&agent).await), [(stg, 1)]);
    assert_eq!(
        statuses(&agent, &key(stg, "production", 1)).await,
        [("production_smoke".to_string(), "queued".to_string())]
    );

    // A rollback is a move like any other.
    let (status, plan) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(
        attempts(&production_tasks(&agent).await),
        [(stg, 1), (prod, 1)]
    );

    // Make Live moves production to the draft again: its next attempt.
    assert_eq!(make_live(&agent).await, StatusCode::OK);
    assert_eq!(pointers(&agent).await.0, Some(stg));
    let queued = attempts(&production_tasks(&agent).await);
    assert_eq!(queued, [(stg, 1), (prod, 1), (stg, 2)]);
    // Make Live again re-stamps the build production serves: nothing new.
    assert_eq!(make_live(&agent).await, StatusCode::OK);
    assert_eq!(attempts(&production_tasks(&agent).await), queued);

    // The console's rollback to a retained build.
    let uri = format!("/customer-apps/{}/rollback", agent.app.id);
    let (status, body) = person(&agent, "POST", &uri, Some(json!({ "build_id": prod }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let queued = attempts(&production_tasks(&agent).await);
    assert_eq!(queued, [(stg, 1), (prod, 1), (stg, 2), (prod, 2)]);

    // A promoting publish moves production through its own transaction.
    let slug = agent.app.slug.as_str();
    let mut promoting = input(
        &agent.t,
        slug,
        "pm-1",
        bundle_with_screens(slug, "pm-1", None),
    );
    promoting.promote = true;
    publish(promoting).await.expect("a promoting publish");
    let published = build_pk(&agent.t.db, agent.app.id, "pm-1").await.unwrap();
    assert_eq!(pointers(&agent).await.0, Some(published));
    let queued = production_tasks(&agent).await;
    assert_eq!(attempts(&queued).last(), Some(&(published, 1)));
    assert_eq!(queued.len(), 5);
    // Each has its one row, and none of staging's kinds.
    for task in &queued {
        let rows = statuses(&agent, &task.key()).await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].0, "production_smoke");
    }
}

/// The task of a production attempt starts the runner with `environment:
/// production`; a staging attempt's with `staging`.
#[tokio::test]
async fn a_production_attempts_task_dispatches_the_workflow_for_production() {
    let agent = gated().await;
    let (_, stg) = fixture_builds(&agent).await;
    settings(&agent, json!({ "required": [] })).await;
    promote(&agent).await;
    let task = production_tasks(&agent).await.remove(0);

    let github = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&github)
        .await;
    let api = github.uri();
    let config = AppChecksConfig::parse(&move |name| match name {
        "OXY_APP_CHECKS_GITHUB_API" => Some(api.clone()),
        "OXY_APP_CHECKS_GITHUB_TOKEN" => Some("ghp_fake_for_tests".to_string()),
        "OXY_APP_CHECKS_DEPLOYMENT" => Some("dev".to_string()),
        _ => None,
    });
    let outcome = run_guarded(&agent.t.db, &task, &config, &[]).await;
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
    let staging = BuildChecksTask {
        environment: "staging".to_string(),
        attempt: 1,
        ..task.clone()
    };
    let outcome = run_guarded(&agent.t.db, &staging, &config, &[]).await;
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");

    let requests = github.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "one dispatch per attempt");
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        sent,
        json!({
            "ref": "main",
            "inputs": {
                "deployment": "dev",
                "app_id": agent.app.id.to_string(),
                "app": format!("{ORG_SLUG}/{}", agent.app.slug),
                "build": stg.to_string(),
                "attempt": "1",
                "environment": "production",
            },
        })
    );
    let sent: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(sent["inputs"]["environment"], "staging");
    // Started, not judged: the check waits for its runner.
    assert_eq!(
        statuses(&agent, &task.key()).await,
        [("production_smoke".to_string(), "queued".to_string())]
    );

    // A runner that never claims it: the rules of a stalled attempt apply to a
    // production attempt as to any. Read as failed after ten minutes, the
    // stored row untouched.
    let read = format!("{}?environment=production", checks(&agent, stg));
    let (_, waiting) = person(&agent, "GET", &read, None).await;
    assert_eq!(
        (&waiting["state"], &waiting["stalled"]),
        (&json!("running"), &json!(null))
    );
    agent
        .t
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_build_checks SET created_at = created_at - interval '11 minutes' \
             WHERE build_id = $1 AND environment = 'production'",
            [stg.into()],
        ))
        .await
        .expect("age the attempt");
    let (_, late) = person(&agent, "GET", &read, None).await;
    assert_eq!(
        (&late["state"], &late["stalled"]),
        (&json!("failed"), &json!("no_runner"))
    );
    assert_eq!(late["checks"][0]["status"], "error");
    assert_eq!(
        statuses(&agent, &task.key()).await,
        [("production_smoke".to_string(), "queued".to_string())]
    );
}

/// Off unless the org's flag is on, and off for an app that does not check
/// production: the move stands and nothing is queued.
#[tokio::test]
async fn with_the_flag_off_or_the_setting_off_a_production_move_queues_nothing() {
    // The flag off: phase 1 exactly.
    let agent = agent().await;
    let (_, stg) = fixture_builds(&agent).await;
    promote(&agent).await;
    assert_eq!(pointers(&agent).await.0, Some(stg));
    assert!(production_tasks(&agent).await.is_empty());
    assert!(rows(&agent, &key(stg, "production", 1)).await.is_empty());

    // The flag on, and the app's `production_smoke` off.
    enable_gate(&agent).await;
    settings(&agent, json!({ "required": [], "production_smoke": false })).await;
    let (status, plan) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    promote(&agent).await;
    assert!(production_tasks(&agent).await.is_empty());

    // Turned on, the next move is checked.
    settings(&agent, json!({ "required": [], "production_smoke": true })).await;
    let (status, plan) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(production_tasks(&agent).await.len(), 1);
}
