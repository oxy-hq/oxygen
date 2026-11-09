//! The arithmetic that decides a 400, a 429, a page, and a partial answer.
//!
//! All of it is pure — the handler only supplies counts — so none of it needs
//! the database-backed harness in `crates/app/tests/platform/simulation_routes.rs`.

use axum::http::StatusCode;
use oxy_simulation::PolicyKind;
use uuid::Uuid;

use super::limits::{
    IN_FLIGHT_MAX_AGE_HOURS, MAX_IN_FLIGHT_PER_WORKSPACE, MAX_RUNS_PER_REQUEST, RunPage,
    check_in_flight, check_request_size, in_flight_cutoff,
};
use super::queued::{EnqueuedRun, FailedArm, QueuedRuns};

fn run(replicate: u32) -> EnqueuedRun {
    EnqueuedRun {
        run_id: Uuid::new_v4(),
        simulation: "w".into(),
        policy: "hold".into(),
        replicate,
        seed: 7,
    }
}

fn failed_arm(replicate: u32, total: usize) -> FailedArm {
    FailedArm {
        policy: PolicyKind::Hold,
        replicate,
        total,
    }
}

/// The blocker: arm 3 of 5 failed, arms 0..3 are executing, and the caller
/// used to get a 500 with no ids.
#[test]
fn a_failure_after_some_runs_keeps_them_and_says_so() {
    let queued = QueuedRuns {
        runs: vec![run(0), run(1), run(2)],
        partial_failure: None,
    };
    let settled = queued
        .absorb_failure(
            failed_arm(3, 5),
            (StatusCode::INTERNAL_SERVER_ERROR, "enqueue failed".into()),
        )
        .expect("runs that queued must be returned");

    assert_eq!(settled.len(), 3, "the queued runs must survive");
    let note = settled.partial_failure.as_deref().expect("a note");
    assert!(note.contains("3 of 5"), "{note}");
    assert!(note.contains("hold #3"), "{note}");
    assert!(note.contains("enqueue failed"), "{note}");
}

/// Zero queued is still the error — there is nothing to keep and the status
/// code should still mean what it says.
#[test]
fn a_failure_before_any_run_is_the_error() {
    let err = QueuedRuns::default()
        .absorb_failure(
            failed_arm(0, 2),
            (StatusCode::BAD_REQUEST, "not a world".into()),
        )
        .expect_err("nothing queued must not read as success");
    assert_eq!(err, (StatusCode::BAD_REQUEST, "not a world".into()));
}

/// The response reads as a list to everything that predates the note.
#[test]
fn queued_runs_reads_as_its_list() {
    let queued = QueuedRuns {
        runs: vec![run(0), run(1)],
        partial_failure: None,
    };
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[1].replicate, 1);
    assert_eq!((&queued).into_iter().count(), 2);
    let json = serde_json::to_value(&queued).expect("serialise");
    assert_eq!(json["runs"].as_array().map(Vec::len), Some(2));
    assert!(json["partial_failure"].is_null());
}

#[test]
fn the_in_flight_cap_counts_the_request_against_what_is_already_there() {
    let room = MAX_IN_FLIGHT_PER_WORKSPACE - 2;
    assert!(
        check_in_flight(room, 2).is_ok(),
        "exactly at the cap is fine"
    );
    let (status, message) = check_in_flight(room + 1, 2).expect_err("one over must refuse");
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // Both numbers, so the message is actionable rather than "too many".
    assert!(message.contains(&(room + 1).to_string()), "{message}");
    assert!(message.contains("2 more"), "{message}");
    assert!(
        message.contains(&MAX_IN_FLIGHT_PER_WORKSPACE.to_string()),
        "{message}"
    );
}

/// The cap counts rows and a terminal write is best-effort, so a run that
/// never finishes would otherwise hold one of the 64 slots forever — and the
/// API has no cancel route to release it.
#[test]
fn an_abandoned_run_ages_out_of_the_in_flight_count() {
    let now = chrono::Utc::now();
    let cutoff = in_flight_cutoff(now);
    assert_eq!(
        now - cutoff,
        chrono::Duration::hours(IN_FLIGHT_MAX_AGE_HOURS),
        "the window is measured back from the caller's clock"
    );

    let just_inside =
        now - chrono::Duration::hours(IN_FLIGHT_MAX_AGE_HOURS) + chrono::Duration::minutes(1);
    assert!(
        just_inside > cutoff,
        "a run queued within the window still counts"
    );
    let long_abandoned = now - chrono::Duration::hours(IN_FLIGHT_MAX_AGE_HOURS + 1);
    assert!(
        long_abandoned < cutoff,
        "a run older than the window does not"
    );
}

/// The 429 used to tell the caller to cancel runs. There is no cancel route,
/// so it named an action the API does not offer.
#[test]
fn the_in_flight_message_names_only_actions_the_api_offers() {
    let (_, message) =
        check_in_flight(MAX_IN_FLIGHT_PER_WORKSPACE, 1).expect_err("over the cap must refuse");
    assert!(
        !message.contains("cancel"),
        "no cancel route exists: {message}"
    );
    assert!(
        message.contains(&format!("{IN_FLIGHT_MAX_AGE_HOURS}h")),
        "the way out of a stuck cap has to be in the message: {message}"
    );
}

#[test]
fn the_in_flight_cap_does_not_overflow() {
    assert!(check_in_flight(u64::MAX, 1).is_err());
}

#[test]
fn the_per_request_cap_is_arms_times_replicates() {
    assert_eq!(check_request_size(2, 32).expect("64 is the limit"), 64);
    let (status, message) = check_request_size(5, 13).expect_err("65 is over");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(message.contains("65 runs"), "{message}");
    assert!(
        message.contains(&MAX_RUNS_PER_REQUEST.to_string()),
        "{message}"
    );
}

#[test]
fn a_page_defaults_and_clamps() {
    let page = RunPage::default();
    assert_eq!(page.limit(), RunPage::DEFAULT_LIMIT);
    assert_eq!(page.offset(), 0);

    let page = RunPage::new(Some(5_000), Some(200));
    assert_eq!(
        page.limit(),
        RunPage::MAX_LIMIT,
        "over the ceiling is the ceiling"
    );
    assert_eq!(page.offset(), 200);

    assert_eq!(
        RunPage::new(Some(0), None).limit(),
        1,
        "a zero page is a page"
    );
}

/// `?policies=machine,machine` queued two identical arms against the
/// per-request cap. `lever_conflicts` (crates/semantic/src/metric_tree.rs)
/// treats a repeated id as one, not a conflict — dedupe here for the same
/// reason: a caller who typo-repeats an arm should get the arm once, not pay
/// for it twice against `MAX_RUNS_PER_REQUEST`.
#[test]
fn parse_policies_dedupes_a_repeated_arm() {
    let policies = super::parse_policies(Some("machine,machine")).expect("valid arm");
    assert_eq!(
        policies,
        vec![PolicyKind::Machine],
        "a repeated arm is one arm, not two queued runs"
    );
}

/// Order is user-visible in the queued runs, so dedupe must keep first-seen
/// order rather than incidentally sorting.
#[test]
fn parse_policies_dedupe_keeps_first_seen_order() {
    let policies = super::parse_policies(Some("machine,hold,machine")).expect("valid arms");
    assert_eq!(policies, vec![PolicyKind::Machine, PolicyKind::Hold]);
}
