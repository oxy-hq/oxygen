//! `on_claim`, without a database: which arm a claimed attempt takes is the
//! whole of the executor's idempotency, so each arm is pinned here. Then
//! admission and the refused stamp, against one: what an attempt does about a
//! run another attempt began, or one that moved while it prepared.

use chrono::{DateTime, Utc};
use sea_orm::ConnectionTrait;

use super::admission::{ClaimStep, admit, begin, on_claim, resolve};
use super::heartbeat::{INTERVAL, STALE_AFTER_SECS};
use super::*;

/// A run's row as of `now`. `begun` is how many seconds ago an attempt stamped
/// it; `beat` how many seconds ago that attempt last proved it was alive.
fn row_at(
    now: DateTime<Utc>,
    status: &str,
    cancel_requested: bool,
    begun: Option<i64>,
    beat: Option<i64>,
) -> proc_run::Model {
    let ago = |secs: i64| (now - chrono::Duration::seconds(secs)).fixed_offset();
    proc_run::Model {
        id: Uuid::new_v4(),
        workspace_id: Uuid::new_v4(),
        procedure_id: "weekly".into(),
        status: status.into(),
        params: None,
        progress_step: None,
        progress_percent: None,
        result_summary: None,
        result_outputs: None,
        error_message: None,
        error_code: None,
        cancel_requested_at: cancel_requested.then_some(now.fixed_offset()),
        started_at: ago(3600),
        execution_started_at: begun.map(ago),
        execution_heartbeat_at: beat.map(ago),
        completed_at: None,
    }
}

/// Long past any threshold: a dead attempt's stamp and last beat.
const LONG_AGO: i64 = 600;

/// Which arm a claimed attempt takes is what makes a requeued run safe to
/// claim twice, and a cancelled one safe to claim at all.
#[test]
fn an_attempt_runs_only_an_open_uncancelled_unbegun_run() {
    let now = Utc::now();
    let claim = |row: proc_run::Model| on_claim(Some(&row), now);
    assert!(matches!(
        claim(row_at(now, "running", false, None, None)),
        ClaimStep::Run
    ));
    assert!(
        matches!(
            claim(row_at(now, "running", true, None, None)),
            ClaimStep::Cancel
        ),
        "a cancel stamped before the claim must stop the run before it starts"
    );
    for closed in ["done", "failed"] {
        assert!(
            matches!(
                claim(row_at(now, closed, false, Some(LONG_AGO), Some(LONG_AGO))),
                ClaimStep::Closed(outcome) if matches!(*outcome, TaskOutcome::Done { .. })
            ),
            "a second attempt at a {closed} run must not execute it again"
        );
    }
    assert!(matches!(
        claim(row_at(now, "cancelled", true, Some(LONG_AGO), Some(LONG_AGO))),
        ClaimStep::Closed(outcome) if matches!(*outcome, TaskOutcome::Cancelled)
    ));
    assert!(matches!(
        on_claim(None, now),
        ClaimStep::Closed(outcome) if matches!(*outcome, TaskOutcome::Failed(_))
    ));
}

/// At-most-once, at the row: a `running` run some attempt already began is
/// never run again, whatever its heartbeat says — and the heartbeat decides
/// only whether it is closed. A user's cancel outranks both readings.
#[test]
fn a_run_an_earlier_attempt_began_is_never_run_again() {
    let now = Utc::now();
    let claim = |row: proc_run::Model| on_claim(Some(&row), now);
    assert!(
        matches!(
            claim(row_at(
                now,
                "running",
                false,
                Some(LONG_AGO),
                Some(LONG_AGO)
            )),
            ClaimStep::Interrupted
        ),
        "a begun run whose attempt went quiet is a dead attempt's: close it, do not run it"
    );
    assert!(
        matches!(
            claim(row_at(now, "running", false, Some(LONG_AGO), Some(1))),
            ClaimStep::Live
        ),
        "a begun run whose attempt beat a second ago is being executed: leave it alone"
    );
    for beat in [Some(LONG_AGO), Some(1)] {
        assert!(
            matches!(
                claim(row_at(now, "running", true, Some(LONG_AGO), beat)),
                ClaimStep::Cancel
            ),
            "a run its user cancelled reads as cancelled, whatever happened to its driver"
        );
    }
}

/// The threshold, at its edges. Two missed beats — the newest beat three
/// intervals old, the third just due — must still read as alive, or one slow
/// write would close a run under the attempt executing it.
#[test]
fn a_heartbeat_goes_stale_at_the_threshold_and_not_before() {
    let now = Utc::now();
    let begun = |beat: i64| {
        let row = row_at(now, "running", false, Some(LONG_AGO), Some(beat));
        on_claim(Some(&row), now)
    };
    let two_missed = 3 * INTERVAL.as_secs() as i64;
    assert!(matches!(begun(0), ClaimStep::Live));
    assert!(
        matches!(begun(two_missed), ClaimStep::Live),
        "the threshold must leave room past two missed beats"
    );
    assert!(matches!(begun(STALE_AFTER_SECS - 1), ClaimStep::Live));
    assert!(matches!(begun(STALE_AFTER_SECS), ClaimStep::Interrupted));
    assert!(matches!(
        begun(STALE_AFTER_SECS + 1),
        ClaimStep::Interrupted
    ));
    // A beat stamped by a pod whose clock runs ahead is not a dead attempt's.
    assert!(matches!(begun(-5), ClaimStep::Live));
}

/// A run begun by a binary from before the heartbeat column has a stamp and
/// no beat. It is read as last alive at the stamp: left alone while the stamp
/// is younger than the threshold, closed once it is not. A beat, when there is
/// one, is what counts — not how long ago the run began.
#[test]
fn a_begun_run_with_no_heartbeat_is_judged_by_its_stamp() {
    let now = Utc::now();
    let claim = |begun: i64, beat: Option<i64>| {
        let row = row_at(now, "running", false, Some(begun), beat);
        on_claim(Some(&row), now)
    };
    assert!(matches!(claim(1, None), ClaimStep::Live));
    assert!(matches!(claim(STALE_AFTER_SECS - 1, None), ClaimStep::Live));
    assert!(matches!(
        claim(STALE_AFTER_SECS, None),
        ClaimStep::Interrupted
    ));
    assert!(matches!(claim(LONG_AGO, None), ClaimStep::Interrupted));
    assert!(
        matches!(claim(LONG_AGO, Some(1)), ClaimStep::Live),
        "a long run is alive for as long as it beats"
    );
}

// ── Against a database ───────────────────────────────────────────────────────

async fn read(db: &DatabaseConnection, id: Uuid) -> proc_run::Model {
    proc_run::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("lookup")
        .expect("row")
}

/// A run an attempt began `begun` seconds ago and last beat `beat` seconds
/// ago (`None`: never, a row from before the column).
async fn begun_run(db: &DatabaseConnection, begun: i64, beat: Option<i64>) -> Uuid {
    let id = super::super::task::submitted_for_test(db).await;
    let beat = match beat {
        Some(secs) => format!("now() - interval '{secs} seconds'"),
        None => "NULL".to_string(),
    };
    db.execute_unprepared(&format!(
        "UPDATE customer_app_automation_runs \
         SET execution_started_at = now() - interval '{begun} seconds', \
             execution_heartbeat_at = {beat} \
         WHERE id = '{id}'"
    ))
    .await
    .expect("stamp the earlier attempt");
    id
}

/// The claim was handed on under a live driver: the run was begun long before
/// this claim, and the attempt that began it is still beating. The claimant
/// must not close the run under it — it hands the task back and writes
/// nothing, for as long as the sweep would leave the run open. The same for a
/// run with no beat at all whose stamp is younger than the threshold.
#[tokio::test]
async fn a_fresh_heartbeat_makes_a_later_claimant_step_aside_and_write_nothing() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    for (begun, beat) in [(LONG_AGO, Some(1)), (5, None)] {
        let id = begun_run(&db, begun, beat).await;
        let before = read(&db, id).await;

        let outcome = admit(&db, id)
            .await
            .expect_err("a begun run is never admitted");
        let TaskOutcome::Deferred { max_wait_secs, .. } = outcome else {
            panic!("begun {begun}s ago, beat {beat:?}: must step aside, got {outcome:?}");
        };
        assert_eq!(
            max_wait_secs,
            settle::ORPHAN_SWEEP_AFTER_SECS as u64,
            "a live attempt is waited for as long as the sweep leaves its run open"
        );
        assert_eq!(read(&db, id).await, before, "stepping aside writes nothing");
    }
}

/// The attempt that began the run went quiet: its last beat is past the
/// threshold. The claimant closes the run as interrupted and runs nothing.
/// The same for a run with no beat at all whose stamp is that old.
#[tokio::test]
async fn a_stale_heartbeat_closes_the_run_as_interrupted() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    // Well past the threshold: the row is stamped on the database's clock and
    // read against this process's.
    let quiet = STALE_AFTER_SECS + 30;
    for (begun, beat) in [(LONG_AGO, Some(quiet)), (quiet, None)] {
        let id = begun_run(&db, begun, beat).await;

        let outcome = admit(&db, id)
            .await
            .expect_err("a begun run is never admitted");
        assert!(
            matches!(&outcome, TaskOutcome::Failed(m) if m == settle::INTERRUPTED_MESSAGE),
            "begun {begun}s ago, beat {beat:?}: {outcome:?}"
        );
        let row = read(&db, id).await;
        assert_eq!(row.status, "failed");
        assert_eq!(row.error_code.as_deref(), Some(settle::INTERRUPTED_CODE));
        assert_eq!(row.result_outputs, None, "nothing ran");
    }
}

/// Two attempts at one run: the second was admitted, and while it prepared the
/// first took the run. The first wrote its heartbeat with the stamp, so it
/// reads as alive — and the second must neither close the run under it nor
/// report a terminal outcome the poll would read as the run's end. It hands
/// the task back unrun.
#[tokio::test]
async fn an_attempt_refused_because_another_began_steps_aside() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let id = super::super::task::submitted_for_test(&db).await;
    admit(&db, id).await.expect("an unbegun run is admitted");
    assert!(
        settle::begin_execution(&db, id).await.expect("stamp"),
        "the other attempt takes the run"
    );
    let taken = read(&db, id).await;
    assert_eq!(
        taken.execution_heartbeat_at, taken.execution_started_at,
        "taking the run is its first heartbeat"
    );

    let outcome = begin(&db, id)
        .await
        .expect_err("this attempt's stamp is refused");
    assert!(
        matches!(outcome, TaskOutcome::Deferred { .. }),
        "a refused attempt must step aside, not settle a run another drives: {outcome:?}"
    );
    assert_eq!(read(&db, id).await, taken, "the run stays the other's");
}

/// The other ways a stamp is refused are states, not a live rival, and are
/// settled as admission would: a cancel that landed while this attempt
/// prepared closes the run as cancelled; a run another attempt finished
/// reports how it closed and is left as it is.
#[tokio::test]
async fn an_attempt_refused_by_a_cancel_or_a_close_settles_what_it_reads() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let cancelled = super::super::task::submitted_for_test(&db).await;
    admit(&db, cancelled).await.expect("admitted");
    db.execute_unprepared(&format!(
        "UPDATE customer_app_procedure_runs SET cancel_requested_at = now() \
         WHERE id = '{cancelled}'"
    ))
    .await
    .expect("stamp the cancel");
    let outcome = begin(&db, cancelled).await.expect_err("refused");
    assert!(matches!(outcome, TaskOutcome::Cancelled), "{outcome:?}");
    assert_eq!(read(&db, cancelled).await.status, "cancelled");

    let finished = super::super::task::submitted_for_test(&db).await;
    admit(&db, finished).await.expect("admitted");
    assert!(settle::begin_execution(&db, finished).await.expect("stamp"));
    assert!(
        settle::close_running(&db, finished, settle::done(&HashMap::new()), Guard::Running)
            .await
            .expect("the other attempt closes it")
    );
    let outcome = begin(&db, finished).await.expect_err("refused");
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(read(&db, finished).await.status, "done");
}

/// `Run` after a refused stamp is a state no writer of the table produces. If
/// it is ever read, the attempt neither runs nor settles: it writes nothing
/// and hands the task back, rather than failing a run someone may own.
#[tokio::test]
async fn a_runnable_read_after_a_refused_stamp_writes_nothing() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let id = super::super::task::submitted_for_test(&db).await;
    let outcome = resolve(&db, id, ClaimStep::Run).await;
    assert!(
        matches!(outcome, TaskOutcome::Deferred { .. }),
        "{outcome:?}"
    );
    let row = read(&db, id).await;
    assert_eq!(row.status, "running");
    assert!(row.execution_started_at.is_none());
}
