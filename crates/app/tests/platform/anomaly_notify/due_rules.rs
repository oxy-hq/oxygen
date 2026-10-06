//! The ledger's own rules: which buckets make an event due, and who may hold
//! the claim on it.

use chrono::{Duration, Utc};
use oxy_metric_monitoring::Severity;
use oxy_metric_monitoring::notify::ledger;
use uuid::Uuid;

use super::{CHANNEL, Row, central_db, due, due_at, seed};

/// One workspace per clause, each holding a single bucket that only that
/// clause keeps out — and a control beside them that must come through, or
/// every assertion here passes on an empty table.
#[tokio::test]
async fn what_the_block_did_not_ask_for_is_never_claimed() {
    let db = central_db().await;
    let control = Uuid::new_v4();
    seed(&db, Row::due(control, Uuid::new_v4())).await;
    assert!(ledger::anything_due(&db, due(control)).await.unwrap());

    let cases: [(&str, fn(&mut Row)); 6] = [
        ("below min_severity", |r| r.severity = "medium"),
        ("already acknowledged", |r| r.status = "acknowledged"),
        ("dismissed", |r| r.status = "dismissed"),
        ("first detected before the window", |r| {
            r.detected_ago = Duration::days(ledger::DETECTION_WINDOW_DAYS + 1)
        }),
        ("about a period long past", |r| {
            r.period_ended_ago = Duration::days(ledger::PERIOD_WINDOW_DAYS + 1)
        }),
        ("not grouped by a scan", |r| r.event = None),
    ];
    for (name, change) in cases {
        let workspace = Uuid::new_v4();
        let mut row = Row::due(workspace, Uuid::new_v4());
        change(&mut row);
        seed(&db, row).await;

        assert!(
            !ledger::anything_due(&db, due(workspace)).await.unwrap(),
            "{name}: must not be due"
        );
        let taken = ledger::claim(&db, due(workspace), CHANNEL, Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(taken, 0, "{name}: must not be claimed");
    }
}

#[tokio::test]
async fn min_severity_is_the_files_to_lower() {
    let db = central_db().await;
    let workspace = Uuid::new_v4();
    seed(
        &db,
        Row {
            severity: "medium",
            ..Row::due(workspace, Uuid::new_v4())
        },
    )
    .await;
    let now = Utc::now();

    assert!(
        !ledger::anything_due(&db, due_at(workspace, Severity::High, now))
            .await
            .unwrap()
    );
    for bar in [Severity::Medium, Severity::Low] {
        assert!(
            ledger::anything_due(&db, due_at(workspace, bar, now))
                .await
                .unwrap(),
            "{bar:?}"
        );
    }
}

/// Two tasks for one workspace — a scheduled scan and a "Scan now" finishing
/// together. The second must find nothing to take while the first is mid-post,
/// and must be able to take over if the first never finishes.
#[tokio::test]
async fn a_claim_in_flight_is_honoured_until_it_goes_stale() {
    let db = central_db().await;
    let workspace = Uuid::new_v4();
    seed(&db, Row::due(workspace, Uuid::new_v4())).await;
    let now = Utc::now();
    let at = |minutes: i64| due_at(workspace, Severity::High, now + Duration::minutes(minutes));
    let (first, second, third) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());

    assert_eq!(ledger::claim(&db, at(0), CHANNEL, first).await.unwrap(), 1);
    assert!(!ledger::anything_due(&db, at(1)).await.unwrap());
    assert_eq!(ledger::claim(&db, at(1), CHANNEL, second).await.unwrap(), 0);

    let stale = ledger::RECLAIM_GRACE_MINUTES + 1;
    assert!(ledger::anything_due(&db, at(stale)).await.unwrap());
    assert_eq!(
        ledger::claim(&db, at(stale), CHANNEL, third).await.unwrap(),
        1
    );
}

/// The other half of a takeover: the task whose claim went stale is still
/// running, and is about to read back what it holds. It must find nothing, or
/// both tasks post.
#[tokio::test]
async fn a_task_that_lost_its_claim_reads_nothing_back() {
    let db = central_db().await;
    let workspace = Uuid::new_v4();
    seed(&db, Row::due(workspace, Uuid::new_v4())).await;
    let now = Utc::now();
    let later = now + Duration::minutes(ledger::RECLAIM_GRACE_MINUTES + 1);
    let (slow, taker) = (Uuid::new_v4(), Uuid::new_v4());
    for (claim_id, at) in [(slow, now), (taker, later)] {
        let taken = ledger::claim(
            &db,
            due_at(workspace, Severity::High, at),
            CHANNEL,
            claim_id,
        )
        .await
        .unwrap();
        assert_eq!(taken, 1);
    }

    let held_by = |claim| ledger::claimed_buckets(&db, workspace, claim);
    assert_eq!(held_by(taker).await.unwrap().len(), 1);
    assert!(held_by(slow).await.unwrap().is_empty());
}

/// A delivered claim is never stale, however old: the grace period reopens
/// only what was never sent.
#[tokio::test]
async fn a_delivered_event_is_not_reclaimed_after_the_grace_period() {
    let db = central_db().await;
    let workspace = Uuid::new_v4();
    seed(&db, Row::due(workspace, Uuid::new_v4())).await;
    let now = Utc::now();
    let claim_id = Uuid::new_v4();
    ledger::claim(
        &db,
        due_at(workspace, Severity::High, now),
        CHANNEL,
        claim_id,
    )
    .await
    .unwrap();
    ledger::mark_delivered(&db, workspace, claim_id, now)
        .await
        .unwrap();

    let later = due_at(workspace, Severity::High, now + Duration::hours(6));
    assert!(!ledger::anything_due(&db, later).await.unwrap());
    assert_eq!(
        ledger::claim(&db, later, CHANNEL, Uuid::new_v4())
            .await
            .unwrap(),
        0
    );
}

/// The message describes the event as its inbox row does: every live bucket,
/// the quiet continuation days included, and nothing someone dismissed.
#[tokio::test]
async fn the_message_reads_every_live_bucket_of_the_event() {
    let db = central_db().await;
    let workspace = Uuid::new_v4();
    let event = Uuid::new_v4();
    seed(&db, Row::due(workspace, event)).await;
    seed(
        &db,
        Row {
            severity: "low",
            ..Row::due(workspace, event)
        },
    )
    .await;
    seed(
        &db,
        Row {
            status: "dismissed",
            ..Row::due(workspace, event)
        },
    )
    .await;

    let claim_id = Uuid::new_v4();
    ledger::claim(&db, due(workspace), CHANNEL, claim_id)
        .await
        .unwrap();
    let buckets = ledger::claimed_buckets(&db, workspace, claim_id)
        .await
        .unwrap();

    assert_eq!(buckets.len(), 2, "{buckets:?}");
    assert!(buckets.iter().all(|b| b.event_id == event));
    let mut severities: Vec<&str> = buckets.iter().map(|b| b.severity.as_str()).collect();
    severities.sort_unstable();
    assert_eq!(severities, ["high", "low"]);
}
