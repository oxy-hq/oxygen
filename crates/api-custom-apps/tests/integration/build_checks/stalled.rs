//! An attempt nobody answers: read as failed, closed by the rerun. And a
//! runner that claims again: measured from its own claim.

use super::*;

#[tokio::test]
async fn an_attempt_nobody_answers_reads_failed_and_can_be_run_again_at_once() {
    let t = seeded_tenant().await;
    let published = publish_staging(&t, "chk-stall", "cs-1").await;
    let (app, build) = models(&t, &published).await;
    let person = Runner::of(staff(&t), app.id, &build.build_id);

    // Fresh: it runs, and a rerun waits for it.
    let fresh = shown(&t, app.id, "cs-1").await;
    assert_eq!(
        (&fresh["state"], &fresh["stalled"]),
        (&json!("running"), &json!(null))
    );
    assert_eq!(fresh["runner"], json!(null));
    assert!(matches!(
        queue::rerun(&t.db, &app, &build).await,
        Err(RerunRefusal::Running { attempt: 1 })
    ));

    // No runner within ten minutes of being queued.
    age_queue(&t.db, build.id, 1, 11).await;
    let unclaimed = shown(&t, app.id, "cs-1").await;
    assert_eq!(unclaimed["stalled"], "no_runner");
    assert_eq!(unclaimed["state"], "failed");
    assert_eq!(unclaimed["checks"][0]["status"], "error");
    assert_eq!(
        statuses(&t.db, build.id, 1).await,
        pairs(&ALL_QUEUED),
        "answered as an error, stored as it was"
    );
    let second = queue::rerun(&t.db, &app, &build).await.expect("at once");
    assert_eq!(second.attempt, 2);
    for row in rows(&t.db, build.id)
        .await
        .iter()
        .filter(|r| r.attempt == 1)
    {
        assert_eq!(row.status, "error", "the rerun closed {}", row.kind);
        assert!(
            row.summary
                .as_deref()
                .unwrap()
                .starts_with("no runner picked the checks up within 10 minutes")
        );
    }
    // The runner that turns up late finds the attempt closed.
    let late = json!({ "attempt": 1, "runner": { "label": "late" } });
    let closed = person.claim(late, HeaderMap::new()).await.unwrap_err();
    assert_eq!(
        (closed.0, &closed.1["code"]),
        (409, &json!("attempt_closed"))
    );

    // A runner that claimed the next attempt and stopped.
    person.claim(by_hand(), HeaderMap::new()).await.unwrap();
    age_claim(&t.db, build.id, 2, 29).await;
    let working = shown(&t, app.id, "cs-1").await;
    assert_eq!(
        (&working["state"], &working["stalled"]),
        (&json!("running"), &json!(null))
    );
    assert_eq!(working["runner"]["label"], hand_label());
    age_claim(&t.db, build.id, 2, 31).await;
    let stopped = shown(&t, app.id, "cs-1").await;
    assert_eq!(stopped["stalled"], "runner_stopped");
    assert_eq!(stopped["state"], "failed");
    assert_eq!(stopped["attempt"], 2);
    let third = queue::rerun(&t.db, &app, &build).await.expect("at once");
    assert_eq!(third.attempt, 3);
    let loads = rows(&t.db, build.id)
        .await
        .into_iter()
        .find(|r| r.attempt == 2 && r.kind == "loads")
        .unwrap();
    assert_eq!(loads.status, "error");
    assert!(
        loads
            .summary
            .unwrap()
            .starts_with("the runner stopped before a verdict")
    );
}

#[tokio::test]
async fn a_runner_that_claims_again_is_measured_from_its_own_claim() {
    let t = seeded_tenant().await;
    let published = publish_staging(&t, "chk-again", "ca-1").await;
    let (app, build) = models(&t, &published).await;
    let person = Runner::of(staff(&t), app.id, &build.build_id);

    // The first runner claimed, and stopped more than thirty minutes ago.
    person.claim(by_hand(), HeaderMap::new()).await.unwrap();
    age_claim(&t.db, build.id, 1, 31).await;
    let stopped = shown(&t, app.id, "ca-1").await;
    assert_eq!(stopped["stalled"], "runner_stopped");

    // The same attempt is run again (GitHub's "Re-run jobs", or by hand).
    let again = json!({ "attempt": 1, "runner": { "label": "run #7, second try", "url": null } });
    let before = chrono::Utc::now();
    person.claim(again, HeaderMap::new()).await.unwrap();
    let working = shown(&t, app.id, "ca-1").await;
    assert_eq!(
        (&working["state"], &working["stalled"]),
        (&json!("running"), &json!(null)),
        "the runner at work is the one the thirty minutes are measured from"
    );
    assert_eq!(working["runner"]["label"], hand_label());
    let started_at: chrono::DateTime<chrono::Utc> =
        serde_json::from_value(working["runner"]["started_at"].clone()).unwrap();
    assert!(started_at >= before, "{started_at} is this claim's time");
    // So a rerun waits for it, and its checks are not closed under it.
    assert!(matches!(
        queue::rerun(&t.db, &app, &build).await,
        Err(RerunRefusal::Running { attempt: 1 })
    ));
    assert_eq!(row_of(&t.db, build.id, "loads").await.status, "running");
}
