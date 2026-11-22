//! The self-heal backoff's tests, moved with it out of
//! `api::middlewares::workspace_context` (2026-11-21).

#[test]
fn self_heal_backoff_doubles_per_consecutive_failure_up_to_six_hours() {
    use super::lazy_compile_backoff_secs as b;
    assert_eq!(b(0, false), 0, "nothing failed, nothing to wait for");
    assert_eq!(b(1, false), 300);
    assert_eq!(b(2, false), 600);
    assert_eq!(b(4, false), 2400);
    assert_eq!(b(8, false), 6 * 60 * 60);
    assert_eq!(
        b(u32::MAX, false),
        6 * 60 * 60,
        "no overflow at the extreme"
    );
}

/// The storm this second guard was added for.
///
/// oxy-staging, 2026-09-24: workspace 64ac780e re-enqueued a compile every
/// 30 seconds for hours — 246 failures in two hours, 246 DISTINCT task ids,
/// so a fresh task each pass rather than one retrying. Every one of them
/// was rejected by `OxyCompileDispatcher::dispatch` at the
/// `!workspace_path.is_dir()` check, which fires BEFORE
/// `insert_compiling_revision` runs. Zero revision rows were written, so
/// the revisions-based count below saw a clean history and waived the
/// backoff, forever.
#[test]
fn a_compile_that_dies_before_it_starts_still_counts_as_a_failure() {
    use super::{compile_task_status_is_failure, leading_failure_count};

    // What the revisions table held during the storm: nothing.
    assert_eq!(
        leading_failure_count(std::iter::empty(), |s| s == "failed"),
        0,
        "the revisions guard could not see the storm — this is the hole"
    );

    // What the task queue held: the failures, newest first.
    let queue = ["failed", "failed", "failed", "failed"];
    let n = leading_failure_count(queue, compile_task_status_is_failure);
    assert_eq!(n, 4);
    assert!(
        super::lazy_compile_backoff_secs(n, false) >= 2400,
        "four dispatch failures must buy at least the 40-minute step, \
         not the zero wait that produced a 30-second cadence"
    );
}

/// The window must fetch EVERY terminal status, or a success can never
/// truncate the run and old failures accumulate into a permanent backoff.
///
/// The first version of this guard fetched only `('failed','dead','cancelled')`.
/// `completed` rows were therefore unreachable, which made
/// `only_the_leading_run_of_real_failures_counts`'s "a success in between"
/// case assert behaviour production could not produce — and left the real
/// hazard open: `failed`/`dead` are retained 30 days while
/// `completed`/`cancelled` are purged at 7, so a workspace that broke once,
/// was fixed, and ran clean for three weeks would have counted eight stale
/// failures as consecutive with one fresh one — six hours of suppressed
/// self-heal from a single transient failure.
#[test]
fn the_backoff_window_reads_every_terminal_status() {
    let in_list = super::RECENT_COMPILE_TASKS_SQL
        .split("queue_status IN (")
        .nth(1)
        .expect("the window must filter on queue_status")
        .split(')')
        .next()
        .expect("unterminated IN list");
    let listed: Vec<&str> = in_list
        .split(',')
        .map(|s| s.trim().trim_matches('\''))
        .collect();

    assert_eq!(
        listed,
        super::COMPILE_TASK_TERMINAL_STATUSES,
        "the SQL and the status list have drifted"
    );
    assert!(
        super::COMPILE_TASK_TERMINAL_STATUSES
            .iter()
            .any(|s| !super::compile_task_status_is_failure(s)),
        "at least one FETCHED status must be a non-failure, or the leading run \
         can never be broken and every old failure counts forever"
    );
}

/// A query whose shape no longer matches its index degrades to a seq scan
/// plus a sort, silently — a planner regression, not an error. This runs on
/// the request hot path inside an advisory-lock transaction, so pin it.
#[test]
fn the_backoff_window_matches_the_index_that_serves_it() {
    // idx_task_queue_compile_terminal:
    //   ON agentic_task_queue ((spec->>'workspace_id'), updated_at DESC)
    //   WHERE spec->>'type' = 'compile'
    let sql = super::RECENT_COMPILE_TASKS_SQL;
    for fragment in [
        "spec->>'type' = 'compile'",  // the index PREDICATE
        "spec->>'workspace_id' = $1", // the leading index KEY
        "ORDER BY updated_at DESC",   // the trailing key, so no sort is needed
    ] {
        assert!(
            sql.contains(fragment),
            "the window dropped {fragment:?}, which idx_task_queue_compile_terminal \
             is built around — add a NEW migration reshaping the index, do not \
             edit the shipped one"
        );
    }
}

/// A staging compile is a branch, not the workspace's main revision:
/// neither a queued one nor a failed one may hold back the self-heal
/// compile of main. (A main compile's spec may omit `kind`.)
#[test]
fn staging_compiles_never_gate_the_main_self_heal() {
    for sql in [
        super::RECENT_COMPILE_TASKS_SQL,
        super::IN_FLIGHT_MAIN_COMPILE_SQL,
    ] {
        assert!(
            sql.contains("COALESCE(spec->>'kind', 'main') = 'main'"),
            "{sql}"
        );
    }
}

#[test]
fn only_the_leading_run_of_real_failures_counts() {
    use super::{compile_task_status_is_failure as f, leading_failure_count as n};

    assert_eq!(n(["failed", "dead", "failed"], f), 3, "dead is a failure");
    assert_eq!(
        n(["failed", "completed", "failed"], f),
        1,
        "a success in between means the workspace recovered once"
    );
    assert_eq!(
        n(["cancelled", "failed", "failed"], f),
        0,
        "a cancel is someone stopping the work, not the work failing"
    );
    assert_eq!(
        n(["failed", "cancelled", "failed"], f),
        1,
        "and it breaks the run rather than being skipped over"
    );
    assert_eq!(n(["claimed", "failed"], f), 0, "in-flight is not failed");
}

#[test]
fn a_content_change_never_waits_longer_than_the_flat_window() {
    use super::lazy_compile_backoff_secs as b;
    assert_eq!(b(0, true), 0);
    assert_eq!(b(1, true), 300);
    assert_eq!(
        b(8, true),
        300,
        "a pushed fix must not wait hours behind the failures"
    );
}
