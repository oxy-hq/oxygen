//! A runner that died after its last check that counts, during the advisory
//! `changes`: the row is not left `running` forever, and the attempt settles.

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::*;

/// Move the claim of `build`'s staging attempt 1 `minutes` into the past.
async fn age_claim(agent: &Agent, build: Uuid, minutes: i64) {
    let at = (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339();
    agent
        .t
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_build_checks \
             SET runner = jsonb_set(runner, '{started_at}', to_jsonb($2::text)) \
             WHERE build_id = $1 AND environment = 'staging' AND attempt = 1",
            [build.into(), at.into()],
        ))
        .await
        .expect("age the claim");
}

/// The answer of `GET …/checks` for `build`, and its `changes` row.
async fn shown(agent: &Agent, build: Uuid) -> (Value, Value) {
    let (status, shown) = person(agent, "GET", &checks(agent, build), None).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    let rows = shown["checks"].as_array().expect("the checks");
    let changes = rows.iter().find(|row| row["kind"] == "changes");
    (shown.clone(), changes.expect("the changes row").clone())
}

/// The runner reports `smoke` and dies during `changes`. Every check that
/// counts has its verdict, so the attempt is not stalled; past the thirty
/// minutes a claimed attempt may stay open, the advisory row reads `error`
/// like any check a runner left behind, the attempt settles, and its notes
/// are queued once.
#[tokio::test]
async fn an_advisory_check_a_dead_runner_left_open_is_closed_and_the_attempt_settles() {
    let agent = gated().await;
    let built = draft(&agent, "ab-1", None).await;
    let base = checks(&agent, built);
    let staging = key(built, "staging", 1);
    let claim = Some(claim_body("staging"));
    let (status, claimed) = checker("POST", &format!("{base}/claim"), claim.clone()).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    let report = json!({ "attempt": 1, "environment": "staging", "checks": [
        passed("loads", json!({})),
        passed("design_system", json!({})),
        passed("smoke", json!({ "screens": [] })),
    ] });
    let (status, reported) = checker("POST", &format!("{base}/report"), Some(report)).await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    assert_eq!(reported, json!({ "state": "passed", "open": ["changes"] }));

    // Inside the window the comparison may still be running: nothing is closed.
    age_claim(&agent, built, 29).await;
    let (answer, changes) = shown(&agent, built).await;
    assert_eq!(changes["status"], "running", "{changes}");
    assert_eq!(
        (&answer["state"], &answer["stalled"]),
        (&json!("passed"), &json!(null))
    );
    assert_eq!(notes_queued(&agent, &staging).await, 0, "not settled yet");

    // Past it, the runner is gone: the row is answered, and written, as the
    // check it left behind.
    age_claim(&agent, built, 31).await;
    let (answer, changes) = shown(&agent, built).await;
    assert_eq!(changes["status"], "error", "{changes}");
    let summary = changes["summary"].as_str().unwrap_or_default();
    assert!(
        summary.starts_with("the runner stopped before a verdict"),
        "{summary}"
    );
    assert_eq!(
        answer["state"], "passed",
        "an advisory row decides no state"
    );
    assert_eq!(answer["stalled"], json!(null), "{answer}");
    let stored = rows(&agent, &staging).await;
    let stored = stored.iter().find(|row| row.kind == "changes").unwrap();
    assert_eq!(stored.status, "error", "written: the attempt is settled");
    assert!(stored.finished_at.is_some());
    assert_eq!(
        notes_queued(&agent, &staging).await,
        1,
        "handed on when it settled"
    );

    // Read again, it is handed on once; and a runner that turns up finds the
    // attempt closed.
    shown(&agent, built).await;
    assert_eq!(notes_queued(&agent, &staging).await, 1);
    let (status, closed) = checker("POST", &format!("{base}/claim"), claim).await;
    assert_eq!(status, StatusCode::CONFLICT, "{closed}");
    assert_eq!(closed["code"], "attempt_closed");
}
