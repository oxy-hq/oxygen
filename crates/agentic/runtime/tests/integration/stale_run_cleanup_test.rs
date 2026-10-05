//! `cleanup_stale_runs` against runs somebody may still hold.
//!
//! The cleanup runs unscoped at every `oxy serve` boot, so in a split fleet it
//! reads rows a worker on another pod is driving. For a root with no events
//! the run's own queue entry is the evidence of whether anything holds it:
//!
//! | queue entry | the run after the cleanup |
//! | --- | --- |
//! | none | `failed` |
//! | `queued` | untouched |
//! | `claimed`, whatever its heartbeat's age | untouched |
//! | `completed` / `failed` / `cancelled` / `dead` | `failed` |
//!
//! The `claimed` row is the one that was wrong: the cleanup spared only
//! `queued`, so a serve pod booting while a driver was still preparing a run
//! reported it `failed` ("server restarted: run never started") under that
//! driver.
//!
//! The last test pins the other arm's safety argument rather than a change:
//! stamping `needs_resume` on a run a live peer drives hands it to no recovery
//! pass that would not have taken it anyway.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(stale_run_cleanup_test)'

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use crate::stuck_run_sweeper_test::{age_run, seed_run, test_db};

const WORKER: &str = "stale-run-cleanup-test";

/// Neither `workflow` nor `airway`, the two kinds the periodic stranded tick
/// re-drives: for every other kind the boot cleanup was the only thing that
/// touched the run, so nothing later corrected a wrong `failed`.
const KIND: &str = "preagg_cycle";

const NEVER_STARTED: &str = "server restarted: run never started";

/// `agentic_pipeline::recovery::STRANDED_GRACE_SECS`, spelled out: this crate
/// sits below the pipeline and cannot name it.
const GRACE: u64 = 30;

/// A root run of `kind` with no events and one `queued` entry of its own.
async fn seed_queued(db: &DatabaseConnection, kind: &str, scope: crud::TaskScope) -> String {
    let run_id = seed_run(db, kind).await;
    crud::enqueue_task(
        db,
        &run_id,
        &run_id,
        None,
        &TaskSpec::Custom {
            kind: kind.into(),
            payload: serde_json::json!({}),
        },
        None,
        scope,
    )
    .await
    .unwrap();
    run_id
}

/// The same run once a driver has claimed it — and written nothing yet.
async fn seed_claimed(db: &DatabaseConnection, kind: &str, scope: crud::TaskScope) -> String {
    let run_id = seed_queued(db, kind, scope).await;
    crud::claim_task_under_root(db, WORKER, &run_id)
        .await
        .unwrap()
        .expect("a freshly queued root is claimable");
    run_id
}

/// Make the claim look abandoned: a heartbeat far past any visibility timeout.
/// `spend_retries` also uses up `max_claims`, so the reaper dead-letters the
/// entry instead of re-queueing it.
async fn abandon_claim(db: &DatabaseConnection, run_id: &str, spend_retries: bool) {
    let claim_count = if spend_retries {
        "max_claims"
    } else {
        "claim_count"
    };
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "UPDATE agentic_task_queue \
             SET last_heartbeat = now() - interval '1 hour', claim_count = {claim_count} \
             WHERE task_id = $1"
        ),
        [run_id.into()],
    ))
    .await
    .unwrap();
}

async fn queue_status(db: &DatabaseConnection, run_id: &str) -> Option<String> {
    crud::get_queue_entry(db, run_id)
        .await
        .unwrap()
        .map(|q| q.queue_status)
}

/// `(task_status, error_message)` of the run.
async fn run_state(db: &DatabaseConnection, run_id: &str) -> (Option<String>, Option<String>) {
    let run = crud::get_run(db, run_id).await.unwrap().expect("run row");
    (run.task_status, run.error_message)
}

fn untouched() -> (Option<String>, Option<String>) {
    (Some("running".to_string()), None)
}

fn failed_as_never_started() -> (Option<String>, Option<String>) {
    (Some("failed".to_string()), Some(NEVER_STARTED.to_string()))
}

/// The defect: a driver has claimed the run and is still preparing it (no
/// first event yet) when a serve pod boots. The run is that driver's, and the
/// boot must not report it failed.
#[tokio::test(flavor = "multi_thread")]
async fn a_claimed_root_with_no_events_survives_the_cleanup() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = seed_claimed(&db, KIND, crud::TaskScope::Global).await;
    assert_eq!(queue_status(&db, &run_id).await.as_deref(), Some("claimed"));

    crud::cleanup_stale_runs(&db).await.unwrap();

    assert_eq!(
        run_state(&db, &run_id).await,
        untouched(),
        "a run whose entry a driver has claimed must not be failed by a boot"
    );
    assert_eq!(
        queue_status(&db, &run_id).await.as_deref(),
        Some("claimed"),
        "and the claim is not the cleanup's to touch"
    );
}

/// Why every `claimed` entry is spared and not only one with a live heartbeat:
/// a dead claim belongs to the reaper, and what the reaper does with it is
/// retry it. Failing the run first would pre-empt that retry and leave a
/// re-queued task behind a run already reported `failed`.
#[tokio::test(flavor = "multi_thread")]
async fn an_abandoned_claim_is_left_for_the_reaper_which_retries_the_run() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = seed_claimed(&db, KIND, crud::TaskScope::Global).await;
    abandon_claim(&db, &run_id, false).await;

    crud::cleanup_stale_runs(&db).await.unwrap();
    assert_eq!(
        run_state(&db, &run_id).await,
        untouched(),
        "the cleanup must not decide a claim is dead — that is the reaper's rule"
    );

    crud::reap_stale_tasks(&db).await.unwrap();
    assert_eq!(queue_status(&db, &run_id).await.as_deref(), Some("queued"));
    let pending = crud::find_pending_global_runs(&db, Some(uuid::Uuid::nil()))
        .await
        .unwrap();
    assert!(
        pending.iter().any(|r| r.run_id == run_id),
        "the re-queued run must be the latency worker's to take; a run the \
         cleanup had failed is not in its selection"
    );
}

/// The orphan the cleanup exists for: a placeholder from a request that died
/// before it enqueued anything.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_with_no_events_and_no_queue_entry_is_failed() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = seed_run(&db, KIND).await;
    assert_eq!(queue_status(&db, &run_id).await, None);

    crud::cleanup_stale_runs(&db).await.unwrap();

    assert_eq!(run_state(&db, &run_id).await, failed_as_never_started());
}

/// A terminal entry holds nothing: whoever had the task has let go of it, and
/// no one will claim it again. Each terminal status, reached the way
/// production reaches it — `dead` by way of the reaper, which closes the loop
/// with the test above: a claim with its retries spent is an orphan after all.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_with_no_events_and_a_terminal_entry_is_failed() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let mut runs = Vec::new();
    for terminal in ["completed", "failed", "cancelled", "dead"] {
        let run_id = seed_claimed(&db, KIND, crud::TaskScope::Global).await;
        match terminal {
            "completed" => {
                crud::complete_queue_task(&db, &run_id, WORKER)
                    .await
                    .unwrap();
            }
            "failed" => {
                crud::fail_queue_task(&db, &run_id, WORKER).await.unwrap();
            }
            "cancelled" => crud::cancel_queued_task(&db, &run_id).await.unwrap(),
            _ => {
                abandon_claim(&db, &run_id, true).await;
                crud::reap_stale_tasks(&db).await.unwrap();
            }
        }
        assert_eq!(queue_status(&db, &run_id).await.as_deref(), Some(terminal));
        runs.push((terminal, run_id));
    }

    crud::cleanup_stale_runs(&db).await.unwrap();

    for (terminal, run_id) in runs {
        assert_eq!(
            run_state(&db, &run_id).await,
            failed_as_never_started(),
            "a zero-event root whose entry is `{terminal}` is an orphan"
        );
    }
}

/// Existing behaviour, kept here so the table in this module's header is
/// pinned in one place: an unclaimed entry is pending work. Both scopes — the
/// row a schedule tick seeds and the one an interactive submit is about to
/// claim.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_root_with_no_events_survives_the_cleanup() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let global = seed_queued(&db, KIND, crud::TaskScope::Global).await;
    let scoped = seed_queued(&db, KIND, crud::TaskScope::Scoped).await;

    crud::cleanup_stale_runs(&db).await.unwrap();

    assert_eq!(run_state(&db, &global).await, untouched());
    assert_eq!(run_state(&db, &scoped).await, untouched());
}

/// Which of the three recovery selections return `run_id`, in the order
/// startup pass, periodic stranded tick, latency worker.
async fn selected_by(db: &DatabaseConnection, run_id: &str) -> [bool; 3] {
    let ws = Some(uuid::Uuid::nil());
    let startup = crud::get_resumable_root_runs(db, ws).await.unwrap();
    let stranded = crud::find_stuck_runs(db, GRACE, ws).await.unwrap();
    let pending = crud::find_pending_global_runs(db, ws).await.unwrap();
    [
        startup.iter().any(|r| r.id == run_id),
        stranded.iter().any(|r| r.run_id == run_id),
        pending.iter().any(|r| r.run_id == run_id),
    ]
}

/// The other arm, which this module does not change: a run WITH events is
/// stamped `needs_resume` at every boot, live driver or not. What makes that
/// tolerable is that the stamp changes no recovery selection — a run a live
/// peer drives is offered to exactly the passes it was offered to before.
///
/// Two shapes, because two different things keep a second driver off:
/// a queue-driven run holds the driver lease, and no pass selects it at all;
/// a direct-driven run holds only its claimed, scope-owned entry, which the
/// two queue-gated passes honour (the startup pass does not, stamp or no
/// stamp — the reason only `oxy serve` runs it).
#[tokio::test(flavor = "multi_thread")]
async fn stamping_needs_resume_on_a_live_peers_run_changes_no_selection() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    // `airway`: a kind the periodic tick does select, so its exclusion below
    // is the lease or the queue predicate at work and not the kind filter.
    let queue_driven = seed_claimed(&db, "airway", crud::TaskScope::Global).await;
    assert!(
        crud::try_acquire_driver(&db, &queue_driven, "a-live-worker")
            .await
            .unwrap()
    );
    let direct_driven = seed_claimed(&db, "airway", crud::TaskScope::Scoped).await;

    let mut before = Vec::new();
    for run_id in [&queue_driven, &direct_driven] {
        crud::insert_event(&db, run_id, 0, "step_start", &serde_json::json!({}), 0)
            .await
            .unwrap();
        // Past the stranded grace, so the tick's answer is not the grace's.
        age_run(&db, run_id, 120).await;
        before.push(selected_by(&db, run_id).await);
    }
    assert_eq!(
        before[0],
        [false, false, false],
        "a run under a live driver lease is no pass's to take"
    );
    assert_eq!(
        before[1],
        [true, false, false],
        "a direct-driven run is excluded by the two queue-gated passes only"
    );

    crud::cleanup_stale_runs(&db).await.unwrap();

    for (run_id, before) in [&queue_driven, &direct_driven].into_iter().zip(before) {
        let (status, _) = run_state(&db, run_id).await;
        assert_eq!(status.as_deref(), Some("needs_resume"));
        // The stamp moved `updated_at`, which would hide the run from the
        // stranded tick behind its grace window; age it back out.
        age_run(&db, run_id, 120).await;
        assert_eq!(
            selected_by(&db, run_id).await,
            before,
            "`needs_resume` must not make a run selectable that was not already"
        );
    }
}
