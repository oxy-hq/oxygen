//! The `functions` check: started once at the claim, settled from its runs
//! and held to the build each run executed, skipped with `loads` once staging
//! has moved on.

use super::*;

/// The run ids of the app's queued function runs named `function`.
async fn function_runs(db: &DatabaseConnection, app_id: Uuid, function: &str) -> Vec<String> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id FROM agentic_runs WHERE source_type = 'app_function' AND question = $1 \
             ORDER BY created_at",
            [format!("fn:{app_id}/{function}").into()],
        ))
        .await
        .expect("read the runs");
    rows.iter()
        .map(|row| row.try_get("", "id").expect("id"))
        .collect()
}

/// How long the function itself ran, as its invocation records it: far from
/// the few milliseconds between a test's claim and its settle.
const RAN_MS: i64 = 987_654;

/// Finish the queued run `run_id` as a worker does after running `function`
/// from the build `ran`: the invocation row names the build that ran and how
/// long the function took ([`RAN_MS`]), the run's events name the invocation,
/// and the run is `done` with `answer`.
async fn worker_ran(
    db: &DatabaseConnection,
    (run_id, app_id, ran): (&str, Uuid, Uuid),
    function: &str,
    answer: &str,
) -> Uuid {
    let invocation = Uuid::new_v4();
    sql(
        db,
        "INSERT INTO app_function_invocations \
         (id, app_id, build_id, function_name, mode, status, duration_ms, created_at, environment) \
         VALUES ($1, $2, $3, $4, 'manual', 'success', $5, now(), 'staging')",
        vec![
            invocation.into(),
            app_id.into(),
            ran.into(),
            function.into(),
            RAN_MS.into(),
        ],
    )
    .await;
    let named = json!({ "invocation_id": invocation, "environment": "staging" });
    for (seq, event) in ["app_function_invocation_started", "app_function_completed"]
        .into_iter()
        .enumerate()
    {
        agentic_runtime::crud::insert_event(db, run_id, seq as i64, event, &named, 0)
            .await
            .expect("write the run's event");
    }
    sql(
        db,
        "UPDATE agentic_runs SET task_status = 'done', answer = $2 WHERE id = $1",
        vec![run_id.into(), answer.into()],
    )
    .await;
    invocation
}

fn check_functions() -> Vec<FunctionSpec> {
    vec![
        FunctionSpec {
            name: "smoke",
            manifest: json!({ "check": true }),
            js: "export default async () => Response.json({ ok: true });",
        },
        FunctionSpec {
            name: "page",
            manifest: json!({ "route": true }),
            js: "export default async () => Response.json({ ran: true });",
        },
    ]
}

#[tokio::test]
async fn a_claim_starts_the_check_functions_once_and_a_settle_writes_their_verdict() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_API_URL", "https://app.example.com/api") };
    let t = seeded_tenant().await;
    let workspace = demo_workspace_id();
    let published = publish_build(&t, "chk-fn", workspace, "fn-1", false, &check_functions()).await;
    let (app, build) = models(&t, &published).await;
    let person = Runner::of(staff(&t), app.id, &build.id.to_string());

    let first = json!({ "attempt": 1, "runner": {
        "label": "GitHub Actions run #1", "url": "https://github.com/o/r/actions/runs/1" } });
    let claimed = person.claim(first, HeaderMap::new()).await.unwrap();
    assert_eq!(
        claimed["open"],
        json!(["functions", "loads", "design_system"])
    );
    assert_eq!(
        claimed["url"],
        format!("https://app.example.com/customer-apps/{ORG_SLUG}/chk-fn/"),
        "the configured API URL's origin"
    );
    assert_eq!(claimed["build"]["sdk_version"], json!(null));
    let runs = function_runs(&t.db, app.id, "smoke").await;
    assert_eq!(runs.len(), 1, "the build's one check function was queued");
    assert!(function_runs(&t.db, app.id, "page").await.is_empty());
    let functions = row_of(&t.db, build.id, "functions").await;
    assert_eq!(functions.status, "running");
    let detail = functions.detail.clone().unwrap();
    assert_eq!(detail["pending"][0]["name"], "smoke");
    assert_eq!(detail["pending"][0]["run_id"], runs[0].as_str());
    // A person's claim is a run by hand, whatever it calls itself: the label
    // and the link of a GitHub Actions run are not kept.
    let runner = functions.runner.unwrap();
    assert_eq!(runner["label"], hand_label());
    assert_eq!(runner["url"], json!(null));
    assert_eq!(runner["by"], LOCAL_GUEST_EMAIL);
    let started_at = runner["started_at"].clone();

    // The same attempt claimed again: nothing is started twice, and the
    // runner's start is the newer claim's.
    let again = json!({ "runner": { "label": "GitHub Actions run #1 (attempt 2)", "url": null } });
    let reclaimed = person.claim(again, HeaderMap::new()).await.unwrap();
    assert_eq!(reclaimed["open"], claimed["open"]);
    assert_eq!(function_runs(&t.db, app.id, "smoke").await, runs);
    let runner = row_of(&t.db, build.id, "loads").await.runner.unwrap();
    assert_eq!(runner["label"], hand_label());
    assert_eq!(runner["url"], json!(null));
    let at = |stored: &Value| -> chrono::DateTime<chrono::Utc> {
        serde_json::from_value(stored.clone()).expect("a time")
    };
    assert!(at(&runner["started_at"]) > at(&started_at));

    let waiting = person.settle(json!({ "attempt": 1 })).await.unwrap();
    assert_eq!(
        waiting,
        json!({ "done": false, "status": "running", "summary": "0 of 1 done" })
    );
    // The worker ran the function of the build being checked.
    let ok = r#"{"ok":true}"#;
    let invocation = worker_ran(&t.db, (&runs[0], app.id, build.id), "smoke", ok).await;
    let done = json!({ "done": true, "status": "passed", "summary": "1 of 1 functions passed" });
    assert_eq!(person.settle(json!({ "attempt": 1 })).await.unwrap(), done);
    let functions = row_of(&t.db, build.id, "functions").await;
    let detail = functions.detail.unwrap();
    assert_eq!(detail["functions"][0]["run_id"], runs[0].as_str());
    assert_eq!(detail["functions"][0]["passed"], true);
    assert_eq!(detail["functions"][0]["invocation_id"], json!(invocation));
    assert_eq!(detail["functions"][0]["ran_build"], json!(build.id));
    assert_eq!(
        detail["functions"][0]["duration_ms"], RAN_MS,
        "the function's own run time, not the time the check had been open"
    );
    assert!(detail.get("pending").is_none(), "the verdict replaces it");
    // Settled: asking again reads the row, and writes nothing.
    assert_eq!(person.settle(json!({})).await.unwrap(), done);
    assert_eq!(
        person.report(json!({ "checks": [] })).await.unwrap(),
        json!({ "state": "running", "open": ["loads", "design_system"] })
    );
}

#[tokio::test]
async fn a_claim_after_staging_moved_skips_what_needs_staging() {
    let t = seeded_tenant().await;
    let workspace = demo_workspace_id();
    let first = publish_build(
        &t,
        "chk-moved",
        workspace,
        "mv-1",
        false,
        &check_functions(),
    )
    .await;
    let (app, build_a) = models(&t, &first).await;
    publish_build(
        &t,
        "chk-moved",
        workspace,
        "mv-2",
        false,
        &check_functions(),
    )
    .await;

    let person = Runner::of(staff(&t), app.id, &build_a.build_id);
    let claimed = person.claim(by_hand(), HeaderMap::new()).await.unwrap();
    assert_eq!(claimed["serving"], false);
    assert_eq!(claimed["open"], json!(["design_system"]));
    assert!(
        function_runs(&t.db, app.id, "smoke").await.is_empty(),
        "a check function would run against the build staging serves now"
    );
    for kind in ["functions", "loads"] {
        let row = row_of(&t.db, build_a.id, kind).await;
        assert_eq!(row.status, "skipped", "{kind}");
        assert_eq!(
            row.summary.as_deref(),
            Some("staging no longer serves build mv-1; run the checks again while it does")
        );
    }
    assert_eq!(
        row_of(&t.db, build_a.id, "validation").await.status,
        "passed"
    );
    let settled = person.settle(json!({})).await.unwrap();
    assert_eq!(
        (&settled["done"], &settled["status"]),
        (&json!(true), &json!("skipped"))
    );
}

/// Claimed while staging served A; B was published before the worker ran the
/// check function, so the worker ran B's. That run says nothing about A: the
/// check is closed as judged by nobody, never `passed`.
#[tokio::test]
async fn a_run_that_executed_another_build_is_no_verdict_on_the_claimed_one() {
    let t = seeded_tenant().await;
    let workspace = demo_workspace_id();
    let slug = "chk-fn-moved";
    let first = publish_build(&t, slug, workspace, "fm-1", false, &check_functions()).await;
    let (app, build_a) = models(&t, &first).await;
    let person = Runner::of(staff(&t), app.id, &build_a.id.to_string());
    let claimed = person.claim(by_hand(), HeaderMap::new()).await.unwrap();
    assert_eq!(claimed["serving"], true);
    let runs = function_runs(&t.db, app.id, "smoke").await;
    assert_eq!(runs.len(), 1, "queued while staging served A");

    let second = publish_build(&t, slug, workspace, "fm-2", false, &check_functions()).await;
    let (_, build_b) = models(&t, &second).await;
    worker_ran(
        &t.db,
        (&runs[0], app.id, build_b.id),
        "smoke",
        r#"{"ok":true}"#,
    )
    .await;

    let settled = person.settle(json!({ "attempt": 1 })).await.unwrap();
    assert_eq!(settled["done"], true, "{settled}");
    assert_eq!(settled["status"], "skipped", "never `passed`: {settled}");
    let summary = settled["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("another build"), "{summary}");
    let functions = row_of(&t.db, build_a.id, "functions").await;
    let detail = functions.detail.unwrap();
    assert_eq!(detail["not_checked"], true, "{detail}");
    assert_eq!(detail["functions"][0]["passed"], false, "{detail}");
    assert_eq!(detail["functions"][0]["ran_build"], json!(build_b.id));
    assert!(detail.get("pending").is_none(), "{detail}");
}
