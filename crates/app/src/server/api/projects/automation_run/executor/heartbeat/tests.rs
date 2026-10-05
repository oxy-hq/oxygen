//! The heartbeat against a database: that an executing run really beats, on
//! the path production runs it through, and that it goes quiet when it should.
//! What admission makes of a beat is `executor/tests.rs`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use agentic_core::delegation::TaskOutcome;
use chrono::{DateTime, FixedOffset};
use sea_orm::EntityTrait;

use super::super::settle::{self, Guard};
use super::*;
use crate::server::api::projects::automation_run::task::submitted_for_test;

/// A cadence a test can watch. Production's is [`INTERVAL`].
const FAST: Duration = Duration::from_millis(25);
/// Long enough for several beats at [`FAST`] to have landed, had any been due.
const SEVERAL_BEATS: Duration = Duration::from_millis(300);

async fn read(db: &DatabaseConnection, id: Uuid) -> proc_run::Model {
    proc_run::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("lookup")
        .expect("row")
}

/// A run an attempt has just taken, and the heartbeat that taking it wrote.
async fn begun(db: &DatabaseConnection) -> (Uuid, DateTime<FixedOffset>) {
    let id = submitted_for_test(db).await;
    assert!(settle::begin_execution(db, id).await.expect("stamp"));
    let first = read(db, id)
        .await
        .execution_heartbeat_at
        .expect("taking the run is its first heartbeat");
    (id, first)
}

/// Wait for a beat newer than `after`; panics if none lands.
async fn next_beat(
    db: &DatabaseConnection,
    id: Uuid,
    after: DateTime<FixedOffset>,
) -> DateTime<FixedOffset> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let beat = read(db, id).await.execution_heartbeat_at;
        if let Some(beat) = beat.filter(|beat| *beat > after) {
            return beat;
        }
        assert!(
            Instant::now() < deadline,
            "the heartbeat never moved past {after}: nothing is refreshing it"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The path production runs a begun run through (`execute_begun`), with steps
/// the test holds open: the row's heartbeat keeps moving for as long as the
/// steps run, and stops moving once the run has settled.
#[tokio::test]
async fn a_run_beats_while_it_executes_and_goes_quiet_once_it_settles() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let (id, taken) = begun(&db).await;

    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    let run = {
        let db = db.clone();
        tokio::spawn(async move {
            let steps = async {
                let _ = finished.await;
                Ok(HashMap::new())
            };
            super::super::execute_begun(&db, id, &CancellationToken::new(), FAST, steps).await
        })
    };

    let first = next_beat(&db, id, taken).await;
    let second = next_beat(&db, id, first).await;
    assert_eq!(
        read(&db, id).await.status,
        "running",
        "the beats land beside the steps, not after them"
    );

    finish.send(()).expect("the run is still executing");
    let outcome = run.await.expect("the run's task");
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
    let settled = read(&db, id).await;
    assert_eq!(settled.status, "done");
    assert!(settled.execution_heartbeat_at >= Some(second));

    tokio::time::sleep(SEVERAL_BEATS).await;
    assert_eq!(
        read(&db, id).await.execution_heartbeat_at,
        settled.execution_heartbeat_at,
        "a settled run must not keep beating"
    );
}

/// Stopping is the ticker's doing, not only the row guard's: on a run that is
/// still `running`, a heartbeat that was stopped — or whose task token was
/// cancelled — writes nothing more.
#[tokio::test]
async fn a_stopped_or_cancelled_heartbeat_goes_quiet_on_a_run_still_running() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };

    let (stopped, taken) = begun(&db).await;
    let beating = spawn(db.clone(), stopped, CancellationToken::new(), FAST);
    next_beat(&db, stopped, taken).await;
    beating.stop().await;

    let (cancelled, taken) = begun(&db).await;
    let cancel = CancellationToken::new();
    let (_keep, ticker) = spawn(db.clone(), cancelled, cancel.clone(), FAST).into_ticker();
    next_beat(&db, cancelled, taken).await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), ticker)
        .await
        .expect("a cancelled task's heartbeat must end")
        .expect("ticker");

    for id in [stopped, cancelled] {
        let quiet = read(&db, id).await;
        assert_eq!(quiet.status, "running");
        tokio::time::sleep(SEVERAL_BEATS).await;
        assert_eq!(
            read(&db, id).await.execution_heartbeat_at,
            quiet.execution_heartbeat_at
        );
    }
}

/// A run someone else closed — the cancel endpoint, the poll, the sweep — is
/// not kept beating by the attempt that was executing it: the next beat finds
/// nothing to stamp and the ticker ends by itself.
#[tokio::test]
async fn the_ticker_ends_on_its_own_once_the_run_is_closed() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let (id, taken) = begun(&db).await;
    let (_keep, ticker) = spawn(db.clone(), id, CancellationToken::new(), FAST).into_ticker();
    next_beat(&db, id, taken).await;

    assert!(
        settle::close_running(&db, id, settle::cancelled(), Guard::Running)
            .await
            .expect("close")
    );
    tokio::time::timeout(Duration::from_secs(5), ticker)
        .await
        .expect("the ticker must end once the run is closed")
        .expect("ticker");

    let closed = read(&db, id).await;
    assert!(
        !beat(&db, id).await.expect("beat"),
        "a beat must not land on a closed run"
    );
    assert_eq!(read(&db, id).await, closed);
}
