//! The tenant boundary on the bulk anomaly-status write.
//!
//! `POST /semantic/anomalies/status` takes a caller-supplied id list, and the
//! only thing standing between that list and another tenant's rows is the
//! `workspace_id` predicate — on **both** statements the write runs: the
//! candidate count that decides what the toast reports, and the `UPDATE`
//! itself. That predicate is exactly the kind of clause a later refactor drops
//! while every unit test keeps passing, and naming one statement when there are
//! two is how the other one's copy goes unmissed — so both are asserted here
//! against a real database rather than reasoned about.
//!
//! Drives `apply_status_bulk` directly: the id normalisation and the HTTP shape
//! have their own unit tests, and going through the router would pull in
//! `api_router`, which reaches the *shared* database (see
//! `authz::shared_db_registry`). This uses `common::fresh_db`, so it gets its
//! own database and stays in `db-per-test`.

use chrono::{Duration, Utc};
use entity::metric_anomalies;
use oxy_api_metric_monitoring::metric_anomalies::{
    AnomalyError, apply_status_bulk, apply_status_bulk_capped,
};
use sea_orm::ActiveValue::Set;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{DatabaseConnection, EntityTrait};
use std::sync::atomic::{AtomicI64, Ordering};
use uuid::Uuid;

/// One anomaly row, `new`, in the given workspace, optionally part of an event.
/// Only the columns the write touches or filters on matter; the rest are filler
/// the NOT NULLs demand.
async fn seed_anomaly(db: &DatabaseConnection, workspace_id: Uuid) -> Uuid {
    seed_bucket(db, workspace_id, None).await
}

/// Each seeded bucket gets its own `period_start`, one day apart.
///
/// `uq_metric_anomalies_workspace_measure_period_dim_grain` covers
/// (workspace, measure, period_start, dimension_key, granularity), and every
/// seed here shares the first, second, fourth and fifth — so `period_start` is
/// the only thing keeping two buckets of one event from colliding. A bare
/// `Utc::now()` makes that hinge on two calls landing in different
/// microseconds; a counter states what the rows actually are, which is
/// consecutive buckets of a chain.
///
/// Process-global while the databases are per-test, so it only ever spaces
/// further apart than a single test needs — the safe direction, and one no
/// assertion here reads.
static SEEDED_BUCKETS: AtomicI64 = AtomicI64::new(0);

async fn seed_bucket(db: &DatabaseConnection, workspace_id: Uuid, event_id: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    let nth = SEEDED_BUCKETS.fetch_add(1, Ordering::Relaxed);
    let period: DateTimeWithTimeZone = (Utc::now() - Duration::days(nth)).into();
    let now = Utc::now().into();
    metric_anomalies::Entity::insert(metric_anomalies::ActiveModel {
        id: Set(id),
        workspace_id: Set(workspace_id),
        measure: Set("sales".into()),
        time_dimension: Set("order_date".into()),
        granularity: Set("day".into()),
        period_start: Set(period),
        period_end: Set(period),
        observed: Set(10.0),
        expected: Set(20.0),
        lower_bound: Set(15.0),
        upper_bound: Set(25.0),
        z_score: Set(-3.0),
        severity: Set("high".into()),
        status: Set("new".into()),
        dimension_key: Set(String::new()),
        event_id: Set(event_id),
        detected_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .exec(db)
    .await
    .expect("seed anomaly");
    id
}

async fn status_of(db: &DatabaseConnection, id: Uuid) -> String {
    metric_anomalies::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("load anomaly")
        .expect("anomaly row still exists")
        .status
}

async fn setup_db() -> DatabaseConnection {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    db
}

#[tokio::test]
async fn writes_only_rows_of_the_calling_workspace() {
    let db = setup_db().await;
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let a = seed_anomaly(&db, mine).await;
    let b = seed_anomaly(&db, mine).await;
    let foreign = seed_anomaly(&db, theirs).await;

    // The caller names a row it has no business naming — the id of another
    // tenant's anomaly, which it could only have guessed, but guessing is the
    // whole threat model for a caller-supplied id list.
    let updated = apply_status_bulk(&db, mine, &[a, b, foreign], &[], &[], "acknowledged")
        .await
        .expect("bulk update");

    assert_eq!(
        updated.rows, 2,
        "only the caller's own rows may be written, and the count must report that honestly"
    );
    assert_eq!(updated.events, 2, "two standalone rows are two anomalies");
    assert_eq!(status_of(&db, a).await, "acknowledged");
    assert_eq!(status_of(&db, b).await, "acknowledged");
    assert_eq!(
        status_of(&db, foreign).await,
        "new",
        "an id from another workspace must be untouched, not merely uncounted"
    );
}

#[tokio::test]
async fn duplicate_and_missing_ids_are_counted_as_rows_not_entries() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();
    let a = seed_anomaly(&db, workspace_id).await;
    let deleted = Uuid::new_v4();

    // `updated` counts rows. A repeated id is one row (the handler dedupes
    // before it gets here); an id with no row is zero.
    let updated = apply_status_bulk(&db, workspace_id, &[a, deleted], &[], &[], "dismissed")
        .await
        .expect("bulk update");
    assert_eq!(updated.rows, 1);
    assert_eq!(status_of(&db, a).await, "dismissed");
}

#[tokio::test]
async fn an_empty_selection_is_a_no_op() {
    let db = setup_db().await;
    // Not an error, and it must not reach Postgres — `IN ()` is a syntax error.
    let updated = apply_status_bulk(&db, Uuid::new_v4(), &[], &[], &[], "acknowledged")
        .await
        .expect("empty bulk update");
    assert_eq!(updated.rows, 0);
    assert_eq!(updated.events, 0);
}

#[tokio::test]
async fn an_event_id_writes_buckets_the_caller_never_saw() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();
    let event = Uuid::new_v4();
    let seen = seed_bucket(&db, workspace_id, Some(event)).await;
    let unseen = seed_bucket(&db, workspace_id, Some(event)).await;
    let unrelated = seed_bucket(&db, workspace_id, None).await;

    // This is the whole reason `event_ids` exists: a list response caps the
    // buckets it returns per event, so a client that acked only what it
    // received would leave the rest of the chain `new` while reporting
    // success. It names the event and the server resolves the full set.
    let updated = apply_status_bulk(&db, workspace_id, &[], &[event], &[], "acknowledged")
        .await
        .expect("bulk update by event");

    assert_eq!(
        updated.rows, 2,
        "every bucket of the event, not just the seen one"
    );
    assert_eq!(
        updated.events, 1,
        "two buckets of one event are one anomaly"
    );
    assert_eq!(status_of(&db, seen).await, "acknowledged");
    assert_eq!(status_of(&db, unseen).await, "acknowledged");
    assert_eq!(
        status_of(&db, unrelated).await,
        "new",
        "an event id must not sweep up rows outside that event"
    );
}

#[tokio::test]
async fn an_event_id_from_another_workspace_writes_nothing() {
    let db = setup_db().await;
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let their_event = Uuid::new_v4();
    let theirs_row = seed_bucket(&db, theirs, Some(their_event)).await;

    // The tenant boundary has to hold for the event list too — it is the same
    // caller-supplied-identifier threat as `ids`, one indirection further out.
    let updated = apply_status_bulk(&db, mine, &[], &[their_event], &[], "dismissed")
        .await
        .expect("bulk update");

    assert_eq!(updated.rows, 0);
    assert_eq!(status_of(&db, theirs_row).await, "new");
}

#[tokio::test]
async fn a_status_bound_spares_buckets_the_caller_never_saw() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();
    let event = Uuid::new_v4();
    let fresh = seed_bucket(&db, workspace_id, Some(event)).await;
    let retired = seed_bucket(&db, workspace_id, Some(event)).await;
    apply_status_bulk(&db, workspace_id, &[retired], &[], &[], "dismissed")
        .await
        .expect("retire one bucket");

    // The row on screen is a `new` one. The inbox sends the live statuses, so
    // the write must not resurrect the bucket dismissed earlier — a deliberate
    // decision, invisible from here.
    let live = ["new".to_string(), "acknowledged".to_string()];
    let updated = apply_status_bulk(&db, workspace_id, &[], &[event], &live, "acknowledged")
        .await
        .expect("bulk update bounded to live statuses");

    assert_eq!(updated.rows, 1);
    assert_eq!(status_of(&db, fresh).await, "acknowledged");
    assert_eq!(
        status_of(&db, retired).await,
        "dismissed",
        "a dismissed bucket must survive a live-status write"
    );
}

#[tokio::test]
async fn a_live_status_bound_still_sweeps_a_chained_bucket() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();
    let event = Uuid::new_v4();
    let older = seed_bucket(&db, workspace_id, Some(event)).await;
    apply_status_bulk(&db, workspace_id, &[older], &[], &[], "acknowledged")
        .await
        .expect("acknowledge the first bucket");
    // A later scan chains a fresh bucket onto the same, already-acknowledged
    // event — the case a single-status bound would strand in the New tab under
    // a toast claiming the anomaly was handled.
    let chained = seed_bucket(&db, workspace_id, Some(event)).await;

    let live = ["new".to_string(), "acknowledged".to_string()];
    let updated = apply_status_bulk(&db, workspace_id, &[], &[event], &live, "dismissed")
        .await
        .expect("bulk update");

    assert_eq!(
        updated.rows, 2,
        "both live buckets, not just the visible one"
    );
    assert_eq!(updated.events, 1);
    assert_eq!(status_of(&db, older).await, "dismissed");
    assert_eq!(status_of(&db, chained).await, "dismissed");
}

#[tokio::test]
async fn the_anomaly_count_describes_the_same_set_the_write_did() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();
    let event = Uuid::new_v4();
    let already_new = seed_bucket(&db, workspace_id, Some(event)).await;

    // The write is bounded to `acknowledged`, and this event holds nothing in
    // that status — so it must report zero on BOTH counts. Counting after the
    // write, keyed on the new status, would have found the untouched `new`
    // bucket and claimed one anomaly moved.
    let updated = apply_status_bulk(
        &db,
        workspace_id,
        &[],
        &[event],
        &["acknowledged".to_string()],
        "new",
    )
    .await
    .expect("bulk update");

    assert_eq!(updated.rows, 0);
    assert_eq!(
        updated.events, 0,
        "an event with nothing in scope was not written and must not be counted"
    );
    assert_eq!(status_of(&db, already_new).await, "new");
}

#[tokio::test]
async fn a_write_that_changes_nothing_reports_nothing() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();
    let event = Uuid::new_v4();
    let bucket = seed_bucket(&db, workspace_id, Some(event)).await;
    let live = ["new".to_string(), "acknowledged".to_string()];
    apply_status_bulk(&db, workspace_id, &[], &[event], &live, "acknowledged")
        .await
        .expect("first acknowledge");

    // "Ack selected" over an already-acknowledged selection. Postgres reports
    // rows *matched*, not changed, so without excluding the target status this
    // came back as a clean success over a no-op — and rewrote `updated_at` on
    // every untouched row.
    let again = apply_status_bulk(&db, workspace_id, &[], &[event], &live, "acknowledged")
        .await
        .expect("second acknowledge");

    assert_eq!(again.rows, 0, "nothing changed, so nothing is reported");
    assert_eq!(again.events, 0);
    assert_eq!(status_of(&db, bucket).await, "acknowledged");
}

#[tokio::test]
async fn an_oversized_selection_is_refused_before_it_writes() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();
    let event = Uuid::new_v4();
    let bucket = seed_bucket(&db, workspace_id, Some(event)).await;

    // `MAX_BULK_ROWS` is well above anything the UI can select, so this test
    // drives the guard directly rather than seeding 20k rows: with the ceiling
    // at zero, one bucket is already too many, and the refusal has to happen
    // before the write rather than after it.
    let refused = apply_status_bulk_capped(&db, workspace_id, &[], &[event], &[], "dismissed", 0)
        .await
        .expect_err("an oversized selection must be refused");

    assert!(matches!(
        refused,
        AnomalyError::TooManyRows { rows: 1, limit: 0 }
    ));
    assert_eq!(
        status_of(&db, bucket).await,
        "new",
        "a refused selection must not have written anything"
    );
}
