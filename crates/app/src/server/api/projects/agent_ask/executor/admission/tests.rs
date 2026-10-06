//! Which arm a claimed attempt takes, for every state a run's rows can be in.
//! No database: the mapping is the whole of the executor's idempotency, and it
//! is a pure function of what was read.

use chrono::TimeZone;

use super::super::super::task::run_for_test;
use super::*;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap()
}

fn ago(secs: i64) -> Option<DateTime<Utc>> {
    Some(now() - chrono::Duration::seconds(secs))
}

fn unbegun() -> Option<RunExecution> {
    Some(RunExecution {
        started_at: None,
        heartbeat_at: None,
    })
}

/// Begun `began` seconds ago, last beat `beat` seconds ago.
fn begun(began: i64, beat: i64) -> Option<RunExecution> {
    Some(RunExecution {
        started_at: ago(began),
        heartbeat_at: ago(beat),
    })
}

fn open() -> run::Model {
    run_for_test(None)
}

fn with_status(status: &str) -> run::Model {
    run::Model {
        task_status: Some(status.into()),
        ..open()
    }
}

/// The one way into the pipeline: open, never begun, never suspended, not
/// cancelled.
#[test]
fn a_fresh_ask_is_run() {
    assert!(matches!(
        on_claim(Some(&open()), unbegun(), false, now()),
        ClaimStep::Run
    ));
}

/// The rule's third clause. An attempt began the ask and went quiet: it is
/// never started again.
#[test]
fn an_ask_whose_attempt_went_quiet_is_interrupted_not_rerun() {
    for (began, beat) in [(300, 60), (300, 61), (7200, 3600)] {
        assert!(
            matches!(
                on_claim(Some(&open()), begun(began, beat), false, now()),
                ClaimStep::Interrupted
            ),
            "began {began}s ago, last beat {beat}s ago"
        );
    }
    // A stamp with no beat reads as last alive at the stamp.
    let stamped_only = Some(RunExecution {
        started_at: ago(61),
        heartbeat_at: None,
    });
    assert!(matches!(
        on_claim(Some(&open()), stamped_only, false, now()),
        ClaimStep::Interrupted
    ));
}

/// The stamp reads the same whether its attempt is dead or running; the beat
/// is what keeps a live one from being closed under itself.
#[test]
fn an_ask_another_attempt_is_executing_is_left_to_it() {
    for (began, beat) in [(0, 0), (300, 15), (300, 59)] {
        assert!(
            matches!(
                on_claim(Some(&open()), begun(began, beat), false, now()),
                ClaimStep::Live
            ),
            "began {began}s ago, last beat {beat}s ago"
        );
    }
}

/// The rule's first clause. A run with a suspension continues from it — so an
/// attempt holding the start spec neither starts it nor closes it, however
/// stale the heartbeat of the attempt that got it there.
#[test]
fn an_ask_at_a_suspension_is_neither_run_nor_closed() {
    for status in ["awaiting_input", "delegating", "running", "needs_resume"] {
        for execution in [begun(7200, 3600), begun(10, 5), unbegun()] {
            assert!(
                matches!(
                    on_claim(Some(&with_status(status)), execution, true, now()),
                    ClaimStep::Parked
                ),
                "{status} {execution:?}"
            );
        }
    }
}

/// A cancel outranks every reading of an open run: parked, live, quiet or
/// fresh, an ask its user stopped is closed as stopped.
#[test]
fn a_cancel_outranks_everything_but_an_ended_run() {
    let cancelled = run::Model {
        cancel_requested_at: Some(agentic_runtime::crud::now()),
        ..open()
    };
    for (execution, suspended) in [
        (unbegun(), false),
        (begun(300, 5), false),
        (begun(300, 600), false),
        (begun(300, 600), true),
    ] {
        assert!(
            matches!(
                on_claim(Some(&cancelled), execution, suspended, now()),
                ClaimStep::Cancel
            ),
            "{execution:?} suspended={suspended}"
        );
    }
}

/// An ended run is reported as it ended — its own answer, its own error. The
/// coordinator writes whatever outcome it is handed onto the run's row, so a
/// generic "already closed" here would overwrite the answer the user got.
#[test]
fn an_ended_ask_is_reported_as_it_ended() {
    let done = run::Model {
        answer: Some("revenue was up 4%".into()),
        ..with_status("done")
    };
    match on_claim(Some(&done), begun(300, 200), false, now()) {
        ClaimStep::Closed(outcome) => match *outcome {
            TaskOutcome::Done { answer, metadata } => {
                assert_eq!(answer, "revenue was up 4%");
                assert!(metadata.is_none());
            }
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }

    for status in ["failed", "timed_out"] {
        let failed = run::Model {
            error_message: Some("cancelled by user".into()),
            // A cancel flag on an ended run changes nothing.
            cancel_requested_at: Some(agentic_runtime::crud::now()),
            ..with_status(status)
        };
        match on_claim(Some(&failed), unbegun(), true, now()) {
            ClaimStep::Closed(outcome) => match *outcome {
                TaskOutcome::Failed(message) => assert_eq!(message, "cancelled by user"),
                other => panic!("{status}: {other:?}"),
            },
            other => panic!("{status}: {other:?}"),
        }
    }

    assert!(matches!(
        on_claim(Some(&with_status("cancelled")), unbegun(), false, now()),
        ClaimStep::Closed(outcome) if matches!(*outcome, TaskOutcome::Cancelled)
    ));
}

/// Nothing to take the stamp on means nothing may start: a missing run, or a
/// run with no extension row, is a failure and never a run.
#[test]
fn an_ask_with_nowhere_to_take_the_stamp_is_never_run() {
    for step in [
        on_claim(None, unbegun(), false, now()),
        on_claim(Some(&open()), None, false, now()),
    ] {
        assert!(
            matches!(&step, ClaimStep::Closed(outcome) if matches!(**outcome, TaskOutcome::Failed(_))),
            "{step:?}"
        );
    }
}

/// What each refusal reports. Stepping aside must be the one outcome that
/// writes nothing onto the run; an interruption must end the bundle's stream.
#[test]
fn a_refusal_reports_only_what_its_reason_allows() {
    for step in [ClaimStep::Parked, ClaimStep::Live] {
        let settled = resolve("run-1", step);
        assert!(settled.event.is_none());
        assert!(
            matches!(
                settled.outcome,
                TaskOutcome::Deferred { delay_secs: 60, max_wait_secs, .. }
                    if max_wait_secs == 24 * 60 * 60
            ),
            "{settled:?}"
        );
    }

    let interrupted = resolve("run-1", ClaimStep::Interrupted);
    let (event_type, payload) = interrupted.event.expect("an event for the stream");
    assert_eq!(event_type, "error");
    assert_eq!(payload["message"], INTERRUPTED_MESSAGE);
    assert!(
        matches!(&interrupted.outcome, TaskOutcome::Failed(m) if m == INTERRUPTED_MESSAGE),
        "{:?}",
        interrupted.outcome
    );

    let cancelled = resolve("run-1", ClaimStep::Cancel);
    assert!(cancelled.event.is_none());
    assert!(matches!(cancelled.outcome, TaskOutcome::Cancelled));
}
