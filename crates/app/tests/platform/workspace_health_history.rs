//! The trail of workspace status changes, against a real Postgres.
//!
//! Three things here are SQL and nothing else: that a row is written only on
//! a change, that one workspace's history is only its own, and that the
//! migration's backfill turns each existing state row into the start of the
//! state it is in. The handler's scope guard is the one the eval trigger
//! beside it uses and is tested with it (`admin_staff_scope`).
//!
//! Own database per test through `common::fresh_db`.

use chrono::{DateTime, Duration, SubsecRound, Utc};
use oxy_app::server::api::admin::workspace_health_history::{
    MAX_TRANSITIONS, RETENTION_DAYS, history_of, record,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

async fn db() -> DatabaseConnection {
    crate::common::fresh_db(crate::common::Schema::Central)
        .await
        .0
}

fn failing(dimensions: &[(&str, &str)]) -> Value {
    Value::Array(
        dimensions
            .iter()
            .map(|(dimension, status)| json!({ "dimension": dimension, "status": status }))
            .collect(),
    )
}

/// A transition written directly, at a chosen time.
async fn seed(db: &DatabaseConnection, workspace: Uuid, at: DateTime<Utc>, to: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_health_transitions (workspace_id, at, from_status, to_status) \
         VALUES ($1, $2, NULL, $3)",
        [workspace.into(), at.fixed_offset().into(), to.into()],
    ))
    .await
    .expect("seed transition");
}

#[tokio::test]
async fn a_row_is_written_when_the_status_changes_and_only_then() {
    let db = db().await;
    let workspace = Uuid::new_v4();
    // Whole seconds. Postgres keeps microseconds and a Linux clock reads
    // nanoseconds, so an untruncated time does not come back equal — except on
    // macOS, whose clock stops at microseconds, which is where this passed.
    let t0 = (Utc::now() - Duration::hours(6)).trunc_subsecs(0);
    let at = |hours: i64| t0 + Duration::hours(hours);

    // The first evaluation is where the history starts, healthy or not.
    assert!(
        record(&db, workspace, None, "healthy", json!([]), at(0))
            .await
            .unwrap()
    );
    // Checked again and again with nothing different: nothing to say.
    assert!(
        !record(&db, workspace, Some("healthy"), "healthy", json!([]), at(1))
            .await
            .unwrap()
    );
    let pipeline = failing(&[("pipeline", "unhealthy")]);
    assert!(
        record(
            &db,
            workspace,
            Some("healthy"),
            "unhealthy",
            pipeline.clone(),
            at(2)
        )
        .await
        .unwrap()
    );
    assert!(
        !record(
            &db,
            workspace,
            Some("unhealthy"),
            "unhealthy",
            pipeline.clone(),
            at(3)
        )
        .await
        .unwrap()
    );
    assert!(
        record(
            &db,
            workspace,
            Some("unhealthy"),
            "healthy",
            json!([]),
            at(5)
        )
        .await
        .unwrap()
    );

    let history = history_of(&db, workspace, 30, Utc::now()).await.unwrap();
    let seen: Vec<(Option<&str>, &str)> = history
        .transitions
        .iter()
        .map(|t| (t.from_status.as_deref(), t.to_status.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            (Some("unhealthy"), "healthy"),
            (Some("healthy"), "unhealthy"),
            (None, "healthy"),
        ],
        "newest first, one row per change"
    );
    assert_eq!(history.transitions[1].failures, pipeline);
    assert_eq!(history.transitions[1].at.with_timezone(&Utc), at(2));
    assert_eq!(history.opening, None);
    assert!(!history.truncated);
}

/// The state row and the trail are written by two statements, and either can
/// fail alone. `prev` is what the state row says; the trail has to go by what
/// it last wrote, or it repeats a change and strands another.
#[tokio::test]
async fn a_change_is_measured_against_the_trail_not_the_state_row() {
    let db = db().await;
    let workspace = Uuid::new_v4();
    let now = Utc::now();
    // The trail recorded "unhealthy", then the state write failed: the state
    // row still says healthy.
    seed(&db, workspace, now - Duration::hours(4), "unhealthy").await;

    // The next pass finds the same change again. It is already written.
    assert!(
        !record(
            &db,
            workspace,
            Some("healthy"),
            "unhealthy",
            json!([]),
            now - Duration::hours(3)
        )
        .await
        .unwrap(),
        "the change the trail already holds is not written twice"
    );
    // It recovers. To the state row nothing changed; to the trail it did.
    assert!(
        record(
            &db,
            workspace,
            Some("healthy"),
            "healthy",
            json!([]),
            now - Duration::hours(1)
        )
        .await
        .unwrap(),
        "a recovery the state row cannot see still closes the stretch"
    );

    let history = history_of(&db, workspace, 30, now).await.unwrap();
    let seen: Vec<(Option<&str>, &str)> = history
        .transitions
        .iter()
        .map(|t| (t.from_status.as_deref(), t.to_status.as_str()))
        .collect();
    assert_eq!(seen, [(Some("unhealthy"), "healthy"), (None, "unhealthy")]);

    // With no trail at all there is only the state row to go by, and a
    // workspace it says has not moved has nothing to record.
    let untracked = Uuid::new_v4();
    assert!(
        !record(&db, untracked, Some("healthy"), "healthy", json!([]), now)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_workspaces_history_is_only_its_own() {
    let db = db().await;
    let (mine, theirs) = (Uuid::new_v4(), Uuid::new_v4());
    let now = Utc::now();
    record(
        &db,
        mine,
        None,
        "degraded",
        json!([]),
        now - Duration::hours(2),
    )
    .await
    .unwrap();
    record(
        &db,
        theirs,
        None,
        "unhealthy",
        json!([]),
        now - Duration::hours(1),
    )
    .await
    .unwrap();

    let history = history_of(&db, mine, 30, now).await.unwrap();
    assert_eq!(history.transitions.len(), 1);
    assert_eq!(history.transitions[0].to_status, "degraded");
}

/// A workspace that went unhealthy before the window opened and has not
/// changed since has no transition inside it. Without the row from before,
/// the window would read as "nothing happened" for a workspace that was down
/// throughout.
#[tokio::test]
async fn the_state_the_window_opened_in_comes_with_it() {
    let db = db().await;
    let workspace = Uuid::new_v4();
    let now = Utc::now();
    seed(&db, workspace, now - Duration::days(40), "healthy").await;
    seed(&db, workspace, now - Duration::days(12), "unhealthy").await;
    seed(&db, workspace, now - Duration::days(2), "healthy").await;

    let week = history_of(&db, workspace, 7, now).await.unwrap();
    assert_eq!(week.window_days, 7);
    assert_eq!(week.transitions.len(), 1);
    let opening = week.opening.expect("the last change before the window");
    assert_eq!(opening.to_status, "unhealthy");

    let month = history_of(&db, workspace, 30, now).await.unwrap();
    assert_eq!(month.transitions.len(), 2);
    assert_eq!(
        month.opening.expect("the change before those").to_status,
        "healthy"
    );
}

/// A workspace unhealthy for four months recovers today. The rows past
/// retention go — all but the newest, which is the only record that it was
/// unhealthy until now. Delete that too and its history starts at the recovery
/// and reads as healthy all along.
#[tokio::test]
async fn a_change_prunes_past_retention_but_keeps_what_the_kept_period_opened_in() {
    let db = db().await;
    let (mine, theirs) = (Uuid::new_v4(), Uuid::new_v4());
    let now = Utc::now();
    let ancient = now - Duration::days(RETENTION_DAYS + 60);
    seed(&db, mine, ancient, "healthy").await;
    seed(
        &db,
        mine,
        now - Duration::days(RETENTION_DAYS + 30),
        "unhealthy",
    )
    .await;
    seed(&db, theirs, ancient, "unhealthy").await;

    record(&db, mine, Some("unhealthy"), "healthy", json!([]), now)
        .await
        .unwrap();

    let count = |workspace: Uuid| {
        let db = &db;
        async move {
            db.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*)::int8 AS n FROM workspace_health_transitions WHERE workspace_id = $1",
                [workspace.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap()
        }
    };
    assert_eq!(
        count(mine).await,
        2,
        "the older row past retention is gone; the newer one and the new change stay"
    );
    assert_eq!(
        count(theirs).await,
        1,
        "another workspace's old rows are not this write's"
    );

    let history = history_of(&db, mine, 30, now + Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(history.transitions.len(), 1);
    assert_eq!(
        history.opening.map(|o| o.to_status).as_deref(),
        Some("unhealthy"),
        "the month before the recovery still reads as unhealthy"
    );
}

/// A workspace flapping hourly makes more rows in a month than one read
/// returns. The cut has to be said, or the oldest stretch reads as quiet.
#[tokio::test]
async fn a_cut_history_says_it_was_cut() {
    let db = db().await;
    let workspace = Uuid::new_v4();
    let now = Utc::now();
    seed(&db, workspace, now - Duration::days(40), "degraded").await;
    for i in 0..(MAX_TRANSITIONS as i64 + 3) {
        let to = if i % 2 == 0 { "unhealthy" } else { "healthy" };
        seed(&db, workspace, now - Duration::minutes(10 * (i + 1)), to).await;
    }

    let history = history_of(&db, workspace, 30, now).await.unwrap();
    assert_eq!(history.transitions.len(), MAX_TRANSITIONS);
    assert!(history.truncated);
    assert_eq!(
        history.opening, None,
        "what the window opened in did not lead to the oldest change listed; \
         changes between them were cut"
    );
    assert_eq!(
        history.transitions[0].at.with_timezone(&Utc).timestamp(),
        (now - Duration::minutes(10)).timestamp(),
        "what is kept is the newest"
    );
}

/// The migration ran against an empty state table when this database was made,
/// so its backfill is run again here over rows that look like production's: a
/// failing workspace whose payload names its dimensions, and an old row with no
/// payload at all.
#[tokio::test]
async fn the_backfill_starts_each_workspaces_history_at_its_current_state() {
    let db = db().await;
    let (broken, old, already) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let since = Utc::now() - Duration::days(3);
    let state = |workspace: Uuid, status: &'static str, payload: Option<Value>| {
        let db = &db;
        async move {
            db.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO workspace_health_state \
                   (workspace_id, status, reasons, changed_at, updated_at, payload) \
                 VALUES ($1, $2, '[]'::jsonb, $3, now(), $4)",
                [
                    workspace.into(),
                    status.into(),
                    since.fixed_offset().into(),
                    payload.into(),
                ],
            ))
            .await
            .expect("seed state row");
        }
    };
    state(
        broken,
        "unhealthy",
        Some(json!({ "dimensions": [
            { "dimension": "queue", "status": "degraded", "reason": "3 dead-letter task(s)" },
            { "dimension": "pipeline", "status": "unhealthy", "reason": "last run failed" },
            { "dimension": "job_liveness", "status": "healthy", "reason": null },
        ]})),
    )
    .await;
    state(old, "healthy", None).await;
    state(already, "degraded", None).await;
    seed(&db, already, since, "degraded").await;

    for _ in 0..2 {
        db.execute_unprepared(migration::WORKSPACE_HEALTH_TRANSITIONS_BACKFILL_SQL)
            .await
            .expect("backfill");
    }

    let now = Utc::now();
    let of_broken = history_of(&db, broken, 30, now).await.unwrap();
    assert_eq!(of_broken.transitions.len(), 1, "run twice, written once");
    let first = &of_broken.transitions[0];
    assert_eq!(
        (first.from_status.as_deref(), first.to_status.as_str()),
        (None, "unhealthy")
    );
    assert_eq!(first.at.with_timezone(&Utc).timestamp(), since.timestamp());
    assert_eq!(
        first.failures,
        failing(&[("pipeline", "unhealthy"), ("queue", "degraded")]),
        "failing dimensions only, without their reason text"
    );

    let of_old = history_of(&db, old, 30, now).await.unwrap();
    assert_eq!(of_old.transitions.len(), 1);
    assert_eq!(of_old.transitions[0].failures, json!([]));
    assert_eq!(
        history_of(&db, already, 30, now)
            .await
            .unwrap()
            .transitions
            .len(),
        1
    );
}
