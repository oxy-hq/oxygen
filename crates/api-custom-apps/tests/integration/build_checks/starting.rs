//! The task only starts the runner: one workflow dispatch to a fake GitHub,
//! and what each outcome leaves in the rows.

use super::*;

/// A fake GitHub answering every request with `status`.
async fn github(status: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn the_registered_executor_starts_the_runner_and_writes_no_verdict() {
    let t = seeded_tenant().await;
    let server = github(204).await;
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OXY_APP_CHECKS_GITHUB_TOKEN", "ghp_fake_for_tests");
        std::env::set_var("OXY_APP_CHECKS_GITHUB_API", server.uri());
        std::env::set_var("OXY_APP_CHECKS_DEPLOYMENT", "dev");
    }
    let published = publish_staging(&t, "chk-run", "cr-1").await;
    let (app, build) = models(&t, &published).await;
    let (task_id, spec) = queued(&t.db, app.id).await.remove(0);

    let outcome = run_one(&t.db, &task_id, spec).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("starting the runner is the task's whole job: {outcome:?}");
    };
    assert_eq!(
        answer,
        "runner started: custom-app-build-checks.yaml on oxy-hq/oxygen-internal@main"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "one dispatch per attempt");
    assert_eq!(
        requests[0].url.path(),
        "/repos/oxy-hq/oxygen-internal/actions/workflows/custom-app-build-checks.yaml/dispatches"
    );
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        sent,
        json!({
            "ref": "main",
            "inputs": {
                "deployment": "dev",
                "app_id": app.id.to_string(),
                "app": format!("{ORG_SLUG}/chk-run"),
                "build": build.id.to_string(),
                "attempt": "1",
                "environment": "staging",
            },
        })
    );
    // Nothing is decided on the worker: every check waits for the runner.
    assert_eq!(statuses(&t.db, build.id, 1).await, pairs(&ALL_QUEUED));
    let written = rows(&t.db, build.id).await;
    assert!(
        written
            .iter()
            .all(|r| r.summary.is_none() && r.detail.is_none())
    );
}

/// A build published on a deployment that starts no runner, its task run:
/// the app, the build, and what its checks then say.
async fn unconfigured(
    t: &Tenant,
    slug: &str,
    build_id: &str,
) -> (entity::apps::Model, entity::app_builds::Model, String) {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::remove_var("OXY_APP_CHECKS_GITHUB_TOKEN");
        std::env::set_var("OXY_APP_CHECKS_DEPLOYMENT", "dev");
    }
    let published = publish_staging(t, slug, build_id).await;
    let (app, build) = models(t, &published).await;
    let (task_id, spec) = queued(&t.db, app.id).await.remove(0);
    let outcome = run_one(&t.db, &task_id, spec).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("not being configured is not a failed task: {outcome:?}");
    };
    assert!(
        answer.starts_with("no runner started (OXY_APP_CHECKS_GITHUB_TOKEN is not set)"),
        "{answer}"
    );
    let says = format!(
        "no runner is configured on this deployment (OXY_APP_CHECKS_GITHUB_TOKEN is not set); \
         run the checks by hand: oxyc checks run {} --build {} --report",
        app.id, build.id
    );
    (app, build, says)
}

#[tokio::test]
async fn a_deployment_that_starts_no_runner_marks_the_attempt_and_its_checks_read_skipped() {
    let t = seeded_tenant().await;
    let (app, build, says) = unconfigured(&t, "chk-none", "cn-1").await;

    // Stored: every check still open, and the attempt says nothing was started.
    assert_eq!(statuses(&t.db, build.id, 1).await, pairs(&ALL_QUEUED));
    for row in rows(&t.db, build.id).await {
        let marker = row.runner.expect("the record that nothing was started");
        let at = marker["at"].as_str().expect("when");
        assert_eq!(
            marker,
            json!({ "not_started": "not_configured",
                "reason": "OXY_APP_CHECKS_GITHUB_TOKEN is not set", "at": at }),
            "{}",
            row.kind
        );
    }

    // Read: skipped at once, and never the stall of a runner that is late.
    for minutes in [0, 11, 45] {
        age_queue(&t.db, build.id, 1, minutes).await;
        let read = shown(&t, app.id, "cn-1").await;
        assert_eq!(
            (&read["state"], &read["stalled"], &read["runner"]),
            (&json!("none"), &json!(null), &json!(null)),
            "after {minutes} more minutes: a build nobody checked never reads as passed"
        );
        assert_eq!(read["attempts"][0]["state"], "none");
        assert_eq!(read["counts"]["skipped"], 4);
        for check in read["checks"].as_array().unwrap() {
            assert_eq!(check["status"], "skipped", "{check}");
            assert_eq!(check["summary"], says.as_str());
            assert_eq!(check["finished_at"], json!(null), "answered, not written");
        }
    }

    // By hand: a claim takes the open checks, and its runner replaces the record.
    let person = Runner::of(staff(&t), app.id, &build.build_id);
    let claimed = person.claim(by_hand(), HeaderMap::new()).await.unwrap();
    assert_eq!(claimed["open"], json!(["loads", "design_system"]));
    let read = shown(&t, app.id, "cn-1").await;
    assert_eq!(read["state"], "running");
    assert_eq!(read["runner"]["label"], hand_label());
    assert_eq!(read["checks"][0]["summary"], "bundle valid");
    // The record never replaces a runner.
    let key = store::AttemptKey {
        build_pk: build.id,
        environment: "staging".to_string(),
        attempt: 1,
    };
    let late = NotStarted::not_configured("late", chrono::Utc::now());
    store::mark_not_started(&t.db, &key, &late).await.unwrap();
    let runner = row_of(&t.db, build.id, "loads").await.runner.unwrap();
    assert_eq!(runner["label"], hand_label());
}

#[tokio::test]
async fn an_attempt_nothing_was_started_for_can_be_run_again_at_once() {
    let t = seeded_tenant().await;
    let (app, build, says) = unconfigured(&t, "chk-none-again", "cna-1").await;

    let next = queue::rerun(&t.db, &app, &build)
        .await
        .expect("an attempt nothing was started for is not still running");
    assert_eq!(next.attempt, 2);
    // The rerun closed the first attempt as it read.
    for row in rows(&t.db, build.id)
        .await
        .iter()
        .filter(|r| r.attempt == 1)
    {
        assert_eq!(row.status, "skipped", "{}", row.kind);
        assert_eq!(row.summary.as_deref(), Some(says.as_str()));
        assert!(row.finished_at.is_some());
    }
    // Stored as skipped now, it still rolls up as a build nobody checked.
    let read = shown(&t, app.id, "cna-1").await;
    assert_eq!(
        (
            &read["attempts"][1]["attempt"],
            &read["attempts"][1]["state"]
        ),
        (&json!(1), &json!("none"))
    );
    let person = Runner::of(staff(&t), app.id, &build.build_id);
    let late = json!({ "attempt": 1, "runner": { "label": "late" } });
    let closed = person.claim(late, HeaderMap::new()).await.unwrap_err();
    assert_eq!(
        (closed.0, &closed.1["code"]),
        (409, &json!("attempt_closed"))
    );
    // The next attempt was queued for a runner: until its task says otherwise
    // it is an attempt that may yet be answered, and a rerun waits for it.
    assert!(matches!(
        queue::rerun(&t.db, &app, &build).await,
        Err(RerunRefusal::Running { attempt: 2 })
    ));
}

#[tokio::test]
async fn a_dispatch_github_refuses_closes_the_attempt_and_fails_the_task() {
    let t = seeded_tenant().await;
    let server = github(401).await;
    let api = server.uri();
    let config = AppChecksConfig::parse(&move |name| match name {
        "OXY_APP_CHECKS_GITHUB_TOKEN" => Some("ghp_fake_for_tests".to_string()),
        "OXY_APP_CHECKS_GITHUB_API" => Some(api.clone()),
        "OXY_APP_CHECKS_DEPLOYMENT" => Some("staging".to_string()),
        _ => None,
    });
    let published = publish_staging(&t, "chk-refused", "cf-1").await;
    let (app, build) = models(&t, &published).await;
    let (_, spec) = queued(&t.db, app.id).await.remove(0);
    let task = BuildChecksTask::from_spec(&spec).expect("the queued task");

    let outcome = run_guarded(&t.db, &task, &config, &[]).await;
    assert!(
        matches!(&outcome, TaskOutcome::Failed(why) if why == "GitHub answered 401"),
        "{outcome:?}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    for row in rows(&t.db, build.id).await {
        assert_eq!(row.status, "error", "{}", row.kind);
        assert_eq!(
            row.summary.as_deref(),
            Some("the runner could not be started: GitHub answered 401")
        );
        assert!(row.finished_at.is_some());
    }
    // Closed, so the next attempt may start at once.
    let next = queue::rerun(&t.db, &app, &build).await.expect("a rerun");
    assert_eq!(next.attempt, 2);
}
