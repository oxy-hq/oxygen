//! The runner routes with an environment: a staging claim's job, a production
//! attempt from claim to verdict, and the seam a settled staging attempt is
//! handed to.

use super::*;
use crate::common::demo_workspace_id;
use crate::promote_rollback::{fixture_builds, rollback};

/// The claim's fields that are the same for every attempt of the fixture.
fn job(agent: &Agent, build: Uuid, build_id: &str) -> Value {
    json!({
        "app": { "id": agent.app.id, "workspace_id": demo_workspace_id(),
            "org_slug": ORG_SLUG, "slug": agent.app.slug },
        "build": { "id": build, "build_id": build_id, "sdk_version": null },
        "serving": true,
        "url": format!("https://app-dev.oxygen-hq.com/customer-apps/{ORG_SLUG}/{}/", agent.app.slug),
        "identity": "app_checker",
        "sdk_floor": "2.30.0",
        "limits": { "screens": 30, "file_bytes": 2_000_000, "files": 200 },
    })
}

/// `job` with the fields in `more` set.
fn with(mut job: Value, more: Value) -> Value {
    for (name, value) in more.as_object().expect("an object") {
        job[name.as_str()] = value.clone();
    }
    job
}

/// The contract's staging claim, and the attempt handed on once (its notes
/// task queued, by the attempt's own run id): when `changes`, its last row,
/// has its verdict, not when the last check that decides its state has.
#[tokio::test]
async fn a_staging_claim_answers_the_job_and_the_attempt_settles_once() {
    let agent = gated().await;
    let (production, _) = fixture_builds(&agent).await;
    let declared = json!([
        "",
        "stores",
        "/abs",
        "stores",
        "x?week=47",
        "x#top",
        "x?",
        "reorder/plans",
        "../up"
    ]);
    let built = draft(&agent, "pc-1", Some(declared)).await;
    let base = checks(&agent, built);
    let staging = key(built, "staging", 1);

    let (status, claimed) = checker(
        "POST",
        &format!("{base}/claim"),
        Some(claim_body("staging")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(
        claimed,
        with(
            job(&agent, built, "pc-1"),
            json!({
                "environment": "staging",
                "attempt": 1,
                "open": ["loads", "design_system", "smoke", "changes"],
                // A query is the screen's own and is kept; a hash and an empty query are not.
                "screens": ["", "stores", "x?week=47", "reorder/plans"],
                "production": { "id": production, "build_id": "agent-prod" },
                "baseline": null,
            })
        )
    );

    // Every check that decides the state has its verdict; `changes` has not.
    let three = json!({ "attempt": 1, "environment": "staging", "checks": [
        passed("loads", json!({})), passed("design_system", json!({})),
        passed("smoke", json!({ "screens": [] })),
    ] });
    let (status, reported) = checker("POST", &format!("{base}/report"), Some(three)).await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    assert_eq!(reported, json!({ "state": "passed", "open": ["changes"] }));
    assert_eq!(
        notes_queued(&agent, &staging).await,
        0,
        "changes is still open"
    );

    // The last verdict, sent twice at once and racing a settle: one of them
    // settles the attempt, and its notes are queued once.
    let last = json!({ "attempt": 1, "environment": "staging",
        "checks": [passed("changes", changes_detail(json!([])))] });
    let settle = json!({ "attempt": 1, "environment": "staging" });
    let (report_uri, settle_uri) = (format!("{base}/report"), format!("{base}/functions/settle"));
    let (one, two, settled) = tokio::join!(
        checker("POST", &report_uri, Some(last.clone())),
        checker("POST", &report_uri, Some(last.clone())),
        checker("POST", &settle_uri, Some(settle)),
    );
    for (status, reported) in [one, two] {
        assert_eq!(status, StatusCode::OK, "{reported}");
        assert_eq!(reported, json!({ "state": "passed", "open": [] }));
    }
    assert_eq!(settled.0, StatusCode::OK, "{}", settled.1);
    assert_eq!(
        notes_queued(&agent, &staging).await,
        1,
        "handed on when it settled, once"
    );

    // The same report again, a settle and a claim change nothing and hand
    // nothing on again.
    let (status, _) = checker("POST", &format!("{base}/report"), Some(last)).await;
    assert_eq!(status, StatusCode::OK);
    let settle = json!({ "attempt": 1, "environment": "staging" });
    let (status, settled) =
        checker("POST", &format!("{base}/functions/settle"), Some(settle)).await;
    assert_eq!(status, StatusCode::OK, "{settled}");
    assert_eq!(settled["status"], "skipped", "the build declares no checks");
    let (status, closed) = checker(
        "POST",
        &format!("{base}/claim"),
        Some(claim_body("staging")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{closed}");
    assert_eq!(closed["code"], "attempt_closed");
    assert_eq!(notes_queued(&agent, &staging).await, 1, "once per attempt");
    assert_eq!(notes_queued(&agent, &key(built, "staging", 2)).await, 0);
}

/// Off: the attempt has four kinds, the claim still says what it can, and a
/// settled attempt is handed nowhere.
#[tokio::test]
async fn with_the_flag_off_a_settled_attempt_is_handed_nowhere() {
    let agent = agent().await;
    with_checker(&agent.t).await;
    let built = draft(&agent, "pc-off", None).await;
    let base = checks(&agent, built);
    let (status, claimed) = checker(
        "POST",
        &format!("{base}/claim"),
        Some(claim_body("staging")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["open"], json!(["loads", "design_system"]));
    assert_eq!(
        claimed["screens"],
        json!(null),
        "the manifest declares none"
    );
    assert_eq!(claimed["production"]["build_id"], "agent-prod");
    let both = json!({ "attempt": 1, "checks": [
        passed("loads", json!({})), passed("design_system", json!({})),
    ] });
    let (_, reported) = checker("POST", &format!("{base}/report"), Some(both)).await;
    assert_eq!(reported, json!({ "state": "passed", "open": [] }));
    assert_eq!(
        notes_queued(&agent, &key(built, "staging", 1)).await,
        0,
        "the gate is off"
    );
}

/// A production attempt, from its claim to its verdict, in the contract's
/// shapes: one kind, judged against what the build's own `changes` run saw
/// on production, with no functions to settle.
#[tokio::test]
async fn a_production_attempt_is_claimed_reported_and_read_back() {
    let agent = gated().await;
    let built = draft(&agent, "pc-2", None).await;
    let before = json!(["TypeError: old", "ReferenceError: older"]);
    pass_staging(&agent, built, before.clone()).await;
    promote(&agent).await;
    let production = key(built, "production", 1);
    assert_eq!(
        statuses(&agent, &production).await,
        [("production_smoke".to_string(), "queued".to_string())]
    );
    let compared = rows(&agent, &key(built, "staging", 1))
        .await
        .into_iter()
        .find(|row| row.kind == "changes")
        .expect("the changes row");

    let base = checks(&agent, built);
    let (status, claimed) = checker(
        "POST",
        &format!("{base}/claim"),
        Some(claim_body("production")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(
        claimed,
        with(
            job(&agent, built, "pc-2"),
            json!({
                "environment": "production",
                "attempt": 1,
                "open": ["production_smoke"],
                "screens": null,
                "production": null,
                "baseline": {
                    "at": compared.finished_at,
                    "screens": [
                        { "path": "stores", "viewport": "desk", "ready": true, "errors": before },
                        { "path": "", "viewport": "desk", "ready": true, "errors": [] },
                    ],
                },
            })
        )
    );

    // No function is started for it, and there is none to settle.
    let settle = json!({ "attempt": 1, "environment": "production" });
    let (status, _) = checker("POST", &format!("{base}/functions/settle"), Some(settle)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A staging kind reported for it has no row and is written nowhere.
    let detail = json!({ "findings": [{ "severity": "error", "at": null, "rule": null,
            "message": "/stores at desk: TypeError: new" }],
        "baseline_at": compared.finished_at, "discovered": true, "screens_omitted": 0,
        "screens": [{ "path": "stores", "viewports": [] }],
        "new_errors": [{ "path": "stores", "viewport": "desk", "error": "TypeError: new" }] });
    let report = json!({ "attempt": 1, "environment": "production", "checks": [
        passed("loads", json!({})),
        { "kind": "production_smoke", "status": "failed",
          "summary": "1 new error on 1 of 2 screens", "detail": detail.clone() },
    ] });
    let (status, reported) = checker("POST", &format!("{base}/report"), Some(report)).await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    assert_eq!(reported, json!({ "state": "failed", "open": [] }));
    assert_eq!(
        statuses(&agent, &production).await,
        [("production_smoke".to_string(), "failed".to_string())]
    );

    // The console reads it by environment; with none named it reads staging.
    let (status, read) = person(
        &agent,
        "GET",
        &format!("{base}?environment=production"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(
        (&read["environment"], &read["state"], &read["attempt"]),
        (&json!("production"), &json!("failed"), &json!(1))
    );
    let shown = &read["checks"][0];
    assert_eq!(
        (&shown["kind"], &shown["label"], &shown["advisory"]),
        (
            &json!("production_smoke"),
            &json!("Production opens after promote"),
            &json!(false)
        )
    );
    assert_eq!(shown["summary"], "1 new error on 1 of 2 screens");
    assert_eq!(shown["detail"], detail, "the runner's evidence, as sent");
    assert_eq!(read["counts"]["total"], 1);
    let (_, staging) = person(&agent, "GET", &base, None).await;
    assert_eq!(staging["environment"], "staging");
    assert_eq!(staging["counts"]["total"], 6);
    let (status, refused) =
        person(&agent, "GET", &format!("{base}?environment=dev-a1"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "invalid_environment");
    // The checker reads none of it back: the console is not its door.
    let (status, _) = checker("GET", &format!("{base}?environment=production"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A runner route names its environment the same way.
    let odd = json!({ "environment": "dev-a1", "runner": { "label": "run" } });
    let (status, refused) = checker("POST", &format!("{base}/claim"), Some(odd)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "invalid_environment");
}

/// A claim after the environment moved off the build closes what would judge
/// another build: staging's `functions`, `loads`, `smoke` and `changes`,
/// production's one check.
#[tokio::test]
async fn a_claim_after_the_environment_moved_skips_what_would_judge_another_build() {
    let agent = gated().await;
    let first = draft(&agent, "pc-3a", None).await;
    draft(&agent, "pc-3b", None).await;
    let base = checks(&agent, first);
    let (status, claimed) = checker(
        "POST",
        &format!("{base}/claim"),
        Some(claim_body("staging")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["serving"], false);
    assert_eq!(claimed["open"], json!(["design_system"]));
    let closed: Vec<(String, String)> = statuses(&agent, &key(first, "staging", 1))
        .await
        .into_iter()
        .filter(|(_, status)| status == "skipped")
        .collect();
    let kinds: Vec<&str> = closed.iter().map(|(kind, _)| kind.as_str()).collect();
    assert_eq!(kinds, ["changes", "functions", "loads", "smoke"]);

    // Production: promote staging's build past its checks, then roll back.
    settings(&agent, json!({ "required": [] })).await;
    let plan = promote(&agent).await;
    let live: Uuid = serde_json::from_value(plan["build"]["id"].clone()).unwrap();
    let (status, rolled) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{rolled}");
    let base = checks(&agent, live);
    let (status, claimed) = checker(
        "POST",
        &format!("{base}/claim"),
        Some(claim_body("production")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(
        (&claimed["serving"], &claimed["open"]),
        (&json!(false), &json!([]))
    );
    let row = rows(&agent, &key(live, "production", 1)).await.remove(0);
    assert_eq!(row.status, "skipped");
    assert_eq!(
        row.summary.as_deref(),
        Some("production no longer serves build pc-3b; it is checked again when it does")
    );
    // Nobody judged it, and the row says so: the attempt never reads `passed`.
    assert_eq!(row.detail.as_ref().unwrap()["not_checked"], true);
    let now = chrono::Utc::now();
    let state = oxy_api_custom_apps::custom_apps_build_checks::dto::attempt_state(&[row], now);
    assert_eq!(state, "none");
}
