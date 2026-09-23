//! Tests for worker eviction safety — graceful claim release, reaper
//! accounting, and root eligibility for orphaned scoped work.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(eviction_safety_test)'

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use agentic_runtime::crud::queue::TaskScope;
use agentic_runtime::migration::RuntimeMigrator;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    std::sync::Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

async fn test_db() -> Option<DatabaseConnection> {
    let url = TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;
            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    std::sync::Arc::new(
                        Postgres::default()
                            .with_tag("18-alpine")
                            // 64 MB (Docker default) is too small: a parallel plan wants a 32 MB
                            // DSM segment and a REUSED container accumulates them.
                            // Must match at every setup site — reuse hashes the config.
                            // See internal-docs/workspace-source.md.
                            .with_shm_size(1024 * 1024 * 1024)
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("failed to start Postgres testcontainer"),
                    )
                })
                .await;
            let port = container.get_host_port_ipv4(5432_u16).await.unwrap();
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

    let mut db = None;
    for attempt in 0..10 {
        match Database::connect(&url).await {
            Ok(conn) => {
                db = Some(conn);
                break;
            }
            Err(e) if attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                eprintln!("test_db: attempt {attempt} failed: {e}");
            }
            Err(e) => panic!("failed to connect after 10 retries: {e}"),
        }
    }
    let db = db.unwrap();
    // Central then runtime (production order — see oxy_test_utils::migration).
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations failed")
        .finish()
        .await;
    Some(db)
}

/// Seed a run plus one queued task, and return `(run_id, task_id)`.
async fn seed_task(
    db: &DatabaseConnection,
    source_type: &str,
    scope: TaskScope,
) -> (String, String) {
    let run_id = format!("ev-{}", uuid::Uuid::new_v4());
    crud::insert_run(db, &run_id, "Q", None, source_type, None, uuid::Uuid::nil())
        .await
        .unwrap();
    crud::enqueue_task(
        db,
        &run_id,
        &run_id,
        None,
        &TaskSpec::Agent {
            agent_id: "test-agent".to_string(),
            question: "q".to_string(),
            extra: None,
        },
        None,
        scope,
    )
    .await
    .unwrap();
    (run_id.clone(), run_id)
}

/// Claim a task as `worker`, failing loudly if the claim doesn't land.
///
/// Every fixture claim in this file goes through here, because
/// `claim_task_under_root(..).await.unwrap()` unwraps the **`Result`** and
/// silently drops the **`Option`** — so a claim that matched no row read as
/// success and only surfaced later, as a mystifying `NotOwned` or an
/// `is_none()` precondition that "spontaneously" broke. That is the wrong end
/// of the failure to be standing at: the interesting event is the lost claim,
/// not its echo three assertions later.
///
/// A lost claim here is real and worth stopping on. These tests share one
/// table, and several operations in it are unscoped by design — the global
/// `claim_task` takes *any* queued root, `reap_stale_tasks` reaps *every*
/// expired row — so a row can genuinely be taken out from under a fixture.
/// **Reports rather than asserts, deliberately.** An `assert!` here would
/// convert a known, unfixed environmental flake into a hard CI failure: the
/// module is not hermetic (see `internal-docs/testing.md`, "`serial-db` is
/// load-bearing for the queue tests"), so under load a fixture row really can be
/// taken by another test's unscoped `reap_stale_tasks` or global `claim_task`.
/// Failing loudly on that would make CI red for something nobody can act on in
/// the moment, which buys less than it costs.
///
/// What was actually missing was the *diagnosis*, and this supplies it: nextest
/// prints captured output only for failing tests, so this line is invisible
/// while things are fine and appears directly above the real assertion when they
/// are not — naming the lost claim instead of leaving a downstream `NotOwned` to
/// be puzzled over. Make it an `assert!` once the module is hermetic.
///
/// **The report is worthless in tests that assert a NEGATIVE**, and those must
/// not rely on it. "Nothing was marked", "no row was touched" and friends hold
/// just as well when the fixture never claimed anything, so the test passes,
/// nextest discards the output, and nobody learns anything. **Such a test must
/// assert its own precondition** — that the fixture claim really landed, and
/// that the operation it is about really ran — before the negative it cares
/// about. See below for what to assert. Stated as a rule rather than a list of
/// which tests do it, because a list is read as a completeness claim and stops
/// being one the moment a test is added.
///
/// That does contradict the paragraph above, and deliberately: those few
/// preconditions DO fail hard on the same environmental lost claim this helper
/// declines to assert on. The difference is what the alternative buys. For the
/// other fixture claims it is a loud failure a moment later on the test's own
/// assertion — the report is enough, and an `assert!` only moves the same
/// failure earlier. For these, the alternative is a *green* run that proves
/// nothing, and a red build beats a false pass every time.
///
/// Assert whichever thing actually discriminates, in this order of preference:
///
/// 1. **The operation's own return value**, when it has one —
///    `DeferOutcome::Deferred`, a `TerminalWrite` that is not `NotOwned`. Best
///    of the three: it rules out a lost claim and a stolen one together, in the
///    line that does the work, with no second query.
///    `the_wait_streak_is_not_reset_by_later_deferrals` is the model.
/// 2. **`worker_id` on the row**, when the operation returns nothing useful. A
///    stolen row is still `claimed`, just not by you, so the owner is the
///    discriminating column and `queue_status` is not.
/// 3. **`queue_status`**, only where the status arm of the predicate under test
///    is itself the confound to rule out — as in
///    `defer_refunds_the_claim_it_returns`.
async fn claim_or_fail(db: &DatabaseConnection, worker: &str, task_id: &str) {
    let claimed = crud::claim_task_under_root(db, worker, task_id)
        .await
        .expect("claim query failed");
    if claimed.is_none() {
        eprintln!(
            "FIXTURE CLAIM DID NOT LAND: worker `{worker}` found no claimable row for \
             `{task_id}`. Everything this test asserts from here is downstream of that. \
             The row was taken by another test's unscoped operation, deferred, or never \
             queued — start there, not at the assertion that follows."
        );
    }
}

/// Age a claimed task's heartbeat so the reaper's visibility timeout has expired.
async fn expire_heartbeat(db: &DatabaseConnection, task_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET last_heartbeat = now() - (visibility_timeout_secs + 10 || ' seconds')::interval \
         WHERE task_id = $1",
        [task_id.into()],
    ))
    .await
    .unwrap();
}

async fn row(db: &DatabaseConnection, task_id: &str) -> agentic_runtime::entity::task_queue::Model {
    crud::get_queue_entry(db, task_id).await.unwrap().unwrap()
}

/// Report `kind`'s outcome as `worker`. The three terminal writers share one
/// ownership predicate, so every property below must hold for all three —
/// covering only `complete` would leave two thirds of the surface unguarded.
async fn terminal_write(
    db: &DatabaseConnection,
    task_id: &str,
    worker: &str,
    kind: &str,
) -> crud::TerminalWrite {
    match kind {
        "complete" => crud::complete_queue_task(db, task_id, worker).await,
        "fail" => crud::fail_queue_task(db, task_id, worker).await,
        "cancel" => crud::cancel_queued_task_owned(db, task_id, worker).await,
        other => panic!("unknown terminal write kind: {other}"),
    }
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn reap_reports_requeue_and_dead_letter_separately() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // One task under the cap -> requeued.
    //
    // Claim scoped to this task's own id (not the generic global `claim_task`,
    // which grabs the oldest queued row in the whole table): this table is
    // shared with every other test in the suite via the reused testcontainer,
    // and a prior test's own reap-requeued row (older `created_at`, still
    // `queued`) would otherwise be claimed instead of the row this test just
    // seeded, silently testing the wrong row.
    let (_, live) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "w-1", &live).await;
    expire_heartbeat(&db, &live).await;

    // One task at the cap -> dead-lettered.
    let (_, doomed) = seed_task(&db, "workflow", TaskScope::Global).await;
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status = 'claimed', claim_count = max_claims \
         WHERE task_id = $1",
        [doomed.clone().into()],
    ))
    .await
    .unwrap();
    expire_heartbeat(&db, &doomed).await;

    let outcome = crud::reap_stale_tasks(&db).await.unwrap();

    assert!(outcome.requeued >= 1, "expected a requeue, got {outcome:?}");
    assert!(
        outcome.dead_lettered >= 1,
        "expected a dead-letter, got {outcome:?}"
    );
    assert_eq!(row(&db, &doomed).await.queue_status, "dead");
}

/// The point of moving the `TASKS_REQUEUED`/`TASKS_DEAD_LETTERED` counters
/// into `reap_stale_tasks` itself: they must increment no matter which
/// caller reached it, not just `background::run_reaper_cycle`.
/// `DurableTransport::run_reaper` is a *different* call site — the one that
/// backs three of the four production reap paths (`oxy worker`'s startup
/// pre-pass, the admin `/run-reaper` handler, and pipeline recovery) — so
/// exercising it here proves the fix, not just the already-tested
/// `reap_stale_tasks` free function.
///
/// Reads the statics as plain process-local globals rather than resetting
/// them: nextest runs every test in its own process (that's the whole
/// reason the suite mandates nextest over `cargo test`), so these start at
/// `0` in this test's process regardless of what any other test does. The
/// `before`/`after` diff is still taken defensively rather than asserting
/// an absolute value, so the test stays correct if that execution model
/// ever changes.
#[tokio::test(flavor = "multi_thread")]
async fn reap_counters_increment_via_a_call_site_other_than_run_reaper_cycle() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    use agentic_runtime::transport::DurableTransport;
    use std::sync::atomic::Ordering;

    let before_requeued = crud::queue::TASKS_REQUEUED.load(Ordering::Relaxed);
    let before_dead = crud::queue::TASKS_DEAD_LETTERED.load(Ordering::Relaxed);

    // One task under the cap -> requeued. See the identical comment on
    // `reap_reports_requeue_and_dead_letter_separately` for why the claim is
    // scoped to this task's own id rather than the generic global claim.
    let (_, live) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "w-dt", &live).await;
    expire_heartbeat(&db, &live).await;

    // One task at the cap -> dead-lettered.
    let (_, doomed) = seed_task(&db, "workflow", TaskScope::Global).await;
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status = 'claimed', claim_count = max_claims \
         WHERE task_id = $1",
        [doomed.clone().into()],
    ))
    .await
    .unwrap();
    expire_heartbeat(&db, &doomed).await;

    // Reap through DurableTransport::run_reaper, NOT
    // background::run_reaper_cycle.
    let transport = DurableTransport::new(db.clone());
    let outcome = transport.run_reaper().await;
    assert!(outcome.requeued >= 1, "expected a requeue, got {outcome:?}");
    assert!(
        outcome.dead_lettered >= 1,
        "expected a dead-letter, got {outcome:?}"
    );

    assert_eq!(
        crud::queue::TASKS_REQUEUED.load(Ordering::Relaxed),
        before_requeued + outcome.requeued,
        "TASKS_REQUEUED must count a reap reached via DurableTransport::run_reaper, \
         not just background::run_reaper_cycle"
    );
    assert_eq!(
        crud::queue::TASKS_DEAD_LETTERED.load(Ordering::Relaxed),
        before_dead + outcome.dead_lettered,
        "TASKS_DEAD_LETTERED must count a reap reached via DurableTransport::run_reaper, \
         not just background::run_reaper_cycle"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reap_nulls_last_heartbeat_on_requeue() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // See the comment in `reap_reports_requeue_and_dead_letter_separately` —
    // scope the claim to this task's own id, not the generic global claim.
    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "w-1", &task_id).await;
    expire_heartbeat(&db, &task_id).await;

    crud::reap_stale_tasks(&db).await.unwrap();

    let r = row(&db, &task_id).await;
    assert_eq!(r.queue_status, "queued");
    assert!(
        r.last_heartbeat.is_none(),
        "requeue must null last_heartbeat, matching the three sibling requeue paths"
    );
}

// ── Graceful release ─────────────────────────────────────────────────────────
//
// Every claim below is `claim_task_under_root` rather than the global
// `claim_task`, for the reason spelled out in the two tests above: this table
// is shared across the whole suite, the global claim takes the *oldest* queued
// row, and a leftover `queued` row from an earlier test is always older than
// the one the test just seeded. With the global claim these tests either pass
// vacuously (having released some other test's row) or fail spuriously — both
// of which hide the regression they exist to catch.

#[tokio::test(flavor = "multi_thread")]
async fn graceful_release_is_budget_neutral_across_repeated_evictions() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;
    let worker = agentic_runtime::transport::process_worker_id();

    // Simulate five rolling-deploy bounces: claim, then release on SIGTERM.
    for i in 0..5 {
        let claimed = crud::claim_task_under_root(&db, worker, &task_id)
            .await
            .unwrap();
        assert_eq!(
            claimed.map(|c| c.task_id).as_deref(),
            Some(task_id.as_str()),
            "claim {i} should succeed on the seeded row"
        );

        let released = crud::release_claims_for_worker(&db, worker).await.unwrap();
        assert_eq!(released, 1, "release {i} should return exactly one claim");

        let r = row(&db, &task_id).await;
        assert_eq!(r.queue_status, "queued", "iteration {i}");
        assert!(r.worker_id.is_none(), "iteration {i}");
        assert!(r.last_heartbeat.is_none(), "iteration {i}");
        // `== 0`, not `<= 1`: claim then release is exactly +1 then -1, so the
        // budget is provably back where it started. The looser bound tolerated
        // a one-per-eviction leak for a whole extra iteration before the
        // `!= "dead"` check below caught it.
        assert_eq!(
            r.claim_count, 0,
            "iteration {i}: claim_count crept to {} — eviction must be budget-neutral",
            r.claim_count
        );
    }

    assert_ne!(
        row(&db, &task_id).await.queue_status,
        "dead",
        "five clean evictions must never dead-letter a healthy task"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hard_crash_still_charges_the_budget() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;

    // Three crashes: claim, then die without releasing. The reaper charges each.
    for _ in 0..3 {
        claim_or_fail(&db, "crasher", &task_id).await;
        expire_heartbeat(&db, &task_id).await;
        crud::reap_stale_tasks(&db).await.unwrap();
    }

    assert_eq!(
        row(&db, &task_id).await.queue_status,
        "dead",
        "a task that repeatedly kills its worker must still dead-letter"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn release_only_touches_this_workers_claims() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, mine) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "worker-a", &mine).await;
    let (_, theirs) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "worker-b", &theirs).await;

    let released = crud::release_claims_for_worker(&db, "worker-a")
        .await
        .unwrap();

    assert_eq!(released, 1);
    assert_eq!(row(&db, &mine).await.queue_status, "queued");
    // `worker_id` rather than `queue_status`, for the same reason as the
    // sibling assertion in `a_claim_taken_during_shutdown_can_be_handed_straight_back`.
    // Weaker here than there — `released == 1` above would already catch a
    // dropped `worker_id = $1`, since a released peer would make it 2 — but the
    // two should read the same way, and the stronger form costs nothing.
    assert_eq!(
        row(&db, &theirs).await.worker_id.as_deref(),
        Some("worker-b"),
        "must never release a peer's live claim"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn release_floors_claim_count_at_zero() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "w-floor", &task_id).await;
    // Same class as `released_agent_root_stays_scoped`: the forced UPDATE below
    // runs regardless of status, and on a lost claim
    // `release_claims_for_worker` then matches nothing — so the final
    // `claim_count == 0` holds without `GREATEST(claim_count - 1, 0)`, the
    // thing under test, ever running.
    // `worker_id`, not `queue_status`: a stolen claim leaves the row `claimed`
    // by the thief, so a status check does not discriminate the case this
    // precondition exists for. Ownership is the property every assertion below
    // actually depends on.
    assert_eq!(
        row(&db, &task_id).await.worker_id.as_deref(),
        Some("w-floor"),
        "precondition: claimed BY US, or the floor below is never exercised"
    );

    // Force the pathological case: claimed with a zero budget already spent.
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET claim_count = 0 WHERE task_id = $1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();

    crud::release_claims_for_worker(&db, "w-floor")
        .await
        .unwrap();

    assert_eq!(
        row(&db, &task_id).await.claim_count,
        0,
        "GREATEST(claim_count - 1, 0) must floor, never underflow"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn releasing_twice_is_a_no_op_and_does_not_double_refund() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // `spawn_shutdown_hook` is registered twice in a full serve (main router
    // + internal router), so the release genuinely runs twice on every
    // graceful shutdown. Idempotency is what makes that safe, and it rests
    // entirely on the `AND queue_status = 'claimed'` predicate — drop that and
    // the second pass refunds a claim nobody spent, walking `claim_count`
    // below the real number of attempts and letting a genuinely failing task
    // retry forever.
    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;

    // Reach `claim_count = 2` before releasing, and reach it honestly: claim,
    // die hard, get reaped (which requeues without refunding), claim again.
    //
    // This setup is what makes the final assertion load-bearing. Releasing from
    // `claim_count = 1` lands on 0, and `GREATEST(0 - 1, 0)` is also 0 — so a
    // genuine double refund would be *invisible*, and the property this test is
    // named for could not fail. From 2 the two outcomes separate: 1 if the
    // second pass correctly matches nothing, 0 if it refunds again.
    claim_or_fail(&db, "w-twice", &task_id).await;
    expire_heartbeat(&db, &task_id).await;
    crud::reap_stale_tasks(&db).await.unwrap();
    claim_or_fail(&db, "w-twice", &task_id).await;
    assert_eq!(
        row(&db, &task_id).await.claim_count,
        2,
        "setup: one reaped crash plus one live claim"
    );

    let first = crud::release_claims_for_worker(&db, "w-twice")
        .await
        .unwrap();
    assert_eq!(first, 1, "the first release returns the one held claim");
    let after_first = row(&db, &task_id).await;
    assert_eq!(after_first.queue_status, "queued");
    assert_eq!(
        after_first.claim_count, 1,
        "the first release refunds only the claim it actually gave back, \
         leaving the earlier crash charged"
    );

    let second = crud::release_claims_for_worker(&db, "w-twice")
        .await
        .unwrap();
    assert_eq!(
        second, 0,
        "the second release must match zero rows — the row is already `queued`"
    );

    let after_second = row(&db, &task_id).await;
    assert_eq!(after_second.queue_status, "queued");
    assert_eq!(
        after_second.claim_count, after_first.claim_count,
        "a repeated release must not refund the budget a second time"
    );
}

/// Named for what it actually pins, not for the drain.
///
/// The previous name (`draining_releases_every_claim_and_stays_budget_neutral`)
/// overclaimed: this body passes byte-identically if `drain_claims_for_worker`
/// is replaced by a single `release_claims_for_worker` call, so it asserts
/// nothing about the *loop*. It is still worth keeping — the drain must not
/// regress either property — but the loop itself is not testable from here.
/// The race the drain narrows is closed at its cause in `recv_assignment`,
/// whose per-row hand-back is pinned by
/// [`a_claim_taken_during_shutdown_can_be_handed_straight_back`].
#[tokio::test(flavor = "multi_thread")]
async fn drain_releases_every_held_claim_without_double_refunding() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, first) = seed_task(&db, "workflow", TaskScope::Global).await;
    let (_, second) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "w-drain", &first).await;
    claim_or_fail(&db, "w-drain", &second).await;

    let released = crud::drain_claims_for_worker(&db, "w-drain").await.unwrap();

    assert_eq!(released, 2, "the drain must release every held claim");
    assert_eq!(row(&db, &first).await.queue_status, "queued");
    assert_eq!(row(&db, &second).await.queue_status, "queued");
    assert_eq!(
        row(&db, &first).await.claim_count,
        0,
        "draining must stay budget-neutral — extra passes must not double-refund"
    );
    assert_eq!(row(&db, &second).await.claim_count, 0);
}

// ── Ownership on terminal writes ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn terminal_writes_cannot_stamp_a_peers_reclaimed_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // The exact interleaving graceful release made reachable:
    //
    //  1. this process claims T and begins executing it;
    //  2. SIGTERM — cancel tokens fire fire-and-forget, then the release flips
    //     T back to `queued` and NOTIFY wakes the fleet;
    //  3. a peer re-claims T within milliseconds (the point of the design);
    //  4. this process's task finally observes the cancel — an in-flight LLM
    //     call can take seconds — and reports its outcome.
    //
    // Without the `worker_id` predicate step 4 stamps the *peer's live claim*
    // terminal. The happy path self-heals when the peer completes; the bad path
    // does not, because the reaper skips terminal rows, so T is never requeued
    // and its parent coordinator waits forever.
    for (label, outcome) in [
        ("completed", "complete"),
        ("failed", "fail"),
        ("cancelled", "cancel"),
    ] {
        let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;
        claim_or_fail(&db, "dying", &task_id).await;
        crud::release_claims_for_worker(&db, "dying").await.unwrap();
        claim_or_fail(&db, "peer", &task_id).await;

        let write = terminal_write(&db, &task_id, "dying", outcome).await;

        assert_eq!(
            write,
            crud::TerminalWrite::NotOwned,
            "{label}: a terminal write from the evicted worker must match zero rows, \
             and must report the peer takeover rather than any of the benign misses"
        );
        let r = row(&db, &task_id).await;
        assert_eq!(
            r.queue_status, "claimed",
            "{label}: the peer's claim must survive the evicted worker's outcome"
        );
        assert_eq!(
            r.worker_id.as_deref(),
            Some("peer"),
            "{label}: the peer must still own the row"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_writes_still_land_for_the_holder() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // The other half of the predicate: it must not break the happy path it
    // guards. Without this, "return false always" would satisfy the test above.
    for (worker, expected, kind) in [
        ("h-done", "completed", "complete"),
        ("h-fail", "failed", "fail"),
        ("h-cancel", "cancelled", "cancel"),
    ] {
        let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;
        claim_or_fail(&db, worker, &task_id).await;

        let write = terminal_write(&db, &task_id, worker, kind).await;

        assert_eq!(
            write,
            crud::TerminalWrite::Stamped,
            "{kind}: the claim holder's terminal write must land"
        );
        assert_eq!(row(&db, &task_id).await.queue_status, expected);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn released_claim_is_immediately_claimable_by_a_successor() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "dying-worker", &task_id).await;
    crud::release_claims_for_worker(&db, "dying-worker")
        .await
        .unwrap();

    // No reaper run, no visibility-timeout wait: the successor picks it up now.
    let claimed = crud::claim_task_under_root(&db, "successor", &task_id)
        .await
        .unwrap();

    assert!(
        claimed.is_some(),
        "a released claim must be available immediately, not after ~90s"
    );
    assert_eq!(claimed.unwrap().task_id, task_id);
    assert_eq!(
        row(&db, &task_id).await.worker_id.as_deref(),
        Some("successor")
    );
}

/// The cause-level half of the gate-to-claim race.
///
/// `recv_assignment` re-checks `is_shutting_down()` *after* its claim returns
/// and hands the row straight back via `release_claim`. The interleaving
/// itself can't be forced from outside the transport (the shutdown flag is
/// process-global and monotonic, so setting it here would disable claiming for
/// every other test in this binary), but the hand-back it performs is exactly
/// this function, and its two properties are what make the fix safe: the row
/// really does become claimable again, and the retry budget is refunded rather
/// than spent on a claim that never ran.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_taken_during_shutdown_can_be_handed_straight_back() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, mine) = seed_task(&db, "workflow", TaskScope::Global).await;
    let (_, sibling) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "w-late", &mine).await;
    claim_or_fail(&db, "w-late", &sibling).await;

    let handed_back = crud::release_claim(&db, &mine, "w-late").await.unwrap();

    assert!(handed_back, "the straggler claim must be handed back");
    let r = row(&db, &mine).await;
    assert_eq!(r.queue_status, "queued");
    assert!(r.worker_id.is_none());
    assert_eq!(
        r.claim_count, 0,
        "a claim handed back before it ran must not charge the retry budget"
    );

    // Scoped to one row on purpose: sibling workers in this process are still
    // executing their own claims when one worker's straggler lands, and a
    // blanket release would yank the row out from under work that will finish.
    // On `worker_id`, not `queue_status`: a sibling stolen by another test is
    // still `claimed` — by the thief — so a status check passes while
    // `release_claim` could have been scoped wrongly all along.
    assert_eq!(
        row(&db, &sibling).await.worker_id.as_deref(),
        Some("w-late"),
        "handing back one claim must not disturb this worker's other claims"
    );

    // Idempotent, same as the bulk release: the row is no longer `claimed` and
    // no longer carries a worker_id.
    assert!(
        !crud::release_claim(&db, &mine, "w-late").await.unwrap(),
        "a second hand-back must match nothing"
    );
    assert_eq!(row(&db, &mine).await.claim_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn release_claim_never_touches_a_peers_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // The per-row release carries the same ownership predicate as the bulk
    // one. Without it, a straggler hand-back could release a row a peer had
    // already re-claimed — reintroducing the very hazard it exists to avoid.
    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "peer", &task_id).await;

    assert!(
        !crud::release_claim(&db, &task_id, "not-the-holder")
            .await
            .unwrap(),
        "releasing a row this worker does not hold must match nothing"
    );
    let r = row(&db, &task_id).await;
    assert_eq!(r.queue_status, "claimed");
    assert_eq!(r.worker_id.as_deref(), Some("peer"));
    assert_eq!(r.claim_count, 1, "and must not refund the peer's budget");
}

// ── Benign terminal-write misses ─────────────────────────────────────────────
//
// The ownership predicate makes a terminal write report a miss in four
// distinct situations, and only one of them is a lost claim. Conflating them
// (as a bare `bool` did) meant the "your work was orphaned" warning fired on
// every user-pressed Stop and on every interactive run's completion — one
// spurious warning per ordinary user action, which is how a signal that was
// added precisely to make orphaned work visible stops being read.

/// The ordinary user-cancel handshake, in the order it really happens.
///
/// `CoordinatorTransport::cancel` awaits the DB write *before* firing the
/// in-memory token, so the row is already `cancelled` by the time the worker
/// observes the token and reports `TaskOutcome::Cancelled`. The worker still
/// owns the row, nothing was released, reaped, or re-claimed, and nothing was
/// dropped — so this must not be reported as a lost claim.
#[tokio::test(flavor = "multi_thread")]
async fn user_cancel_then_worker_cancelled_is_not_a_lost_claim() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;
    claim_or_fail(&db, "w-stop", &task_id).await;

    // 1. The requester (user pressed Stop) stamps the row.
    crud::cancel_queued_task(&db, &task_id).await.unwrap();
    assert_eq!(row(&db, &task_id).await.queue_status, "cancelled");

    // 2. The worker, still the owner, reports its own Cancelled outcome.
    let write = terminal_write(&db, &task_id, "w-stop", "cancel").await;

    assert_ne!(
        write,
        crud::TerminalWrite::NotOwned,
        "the worker still holds this claim — reporting it as lost is a false alarm \
         on every single user-pressed Stop"
    );
    assert_eq!(
        write,
        crud::TerminalWrite::AlreadyTerminal,
        "the requester got there first; the write is a no-op, not a failure"
    );
    assert_eq!(
        row(&db, &task_id).await.queue_status,
        "cancelled",
        "and the status the user asked for must survive"
    );
}

/// Finding 4's asymmetry, resolved: the first terminal status wins.
///
/// Before the status guard was applied uniformly, a worker reporting `Failed`
/// after a user cancel flipped `cancelled -> failed`, and a later `Done`
/// flipped it again to `completed` — so what a cancelled task ended up storing
/// depended on arrival order.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_row_is_not_reopened_by_a_later_outcome() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    for kind in ["fail", "complete"] {
        let (_, task_id) = seed_task(&db, "workflow", TaskScope::Global).await;
        claim_or_fail(&db, "w-late-outcome", &task_id).await;
        crud::cancel_queued_task(&db, &task_id).await.unwrap();

        let write = terminal_write(&db, &task_id, "w-late-outcome", kind).await;

        assert_eq!(
            write,
            crud::TerminalWrite::AlreadyTerminal,
            "{kind}: a late outcome on a cancelled row is a no-op, not a lost claim"
        );
        assert_eq!(
            row(&db, &task_id).await.queue_status,
            "cancelled",
            "{kind}: the first terminal status must win, so a cancelled task \
             stores the same thing regardless of outcome arrival order"
        );
    }
}

/// Root tasks registered via `Coordinator::register_root` are driven by a
/// virtual worker and never published as an assignment, so they have no queue
/// row at all — every interactive analytics/builder run reports its outcome
/// this way. "No row to stamp" is not "a peer took your claim".
#[tokio::test(flavor = "multi_thread")]
async fn a_task_with_no_queue_row_is_not_a_lost_claim() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let absent = format!("never-enqueued-{}", uuid::Uuid::new_v4());

    for kind in ["complete", "fail", "cancel"] {
        assert_eq!(
            terminal_write(&db, &absent, "w-virtual", kind).await,
            crud::TerminalWrite::NoRow,
            "{kind}: a task with no queue row must be distinguishable from a peer takeover"
        );
    }
}

/// The global claim path must never take a task that has a parent, even when
/// that task is `scope_owned = false` (globally claimable in every other
/// respect). A descendant claimed in isolation reports its outcome to
/// whichever coordinator holds the transport — not necessarily the one whose
/// in-memory task map actually contains it — so `handle_done` early-returns,
/// the queue row ends `completed`, the run stays `delegating`, and the parent
/// waits forever. See `DurableTransport::task_id_root` and
/// `claim_task`/`claim_task_under_root` in `orchestrator/crud/queue.rs`.
#[tokio::test(flavor = "multi_thread")]
async fn global_claim_refuses_a_leaf_task() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // Drain any root/global tasks earlier tests in this file left `queued`.
    // This is the only test that calls the truly unscoped `claim_task` —
    // every other test in the file deliberately claims via
    // `claim_task_under_root` instead precisely because this table is shared
    // across the whole suite (see the comment above "── Graceful release"),
    // and several of them (the reap tests, the budget-neutrality loop, the
    // shutdown hand-back test) leave their own root task sitting `queued`
    // once they're done with it. `claim_task` orders by `created_at ASC`, so
    // without draining first, an older leftover row — not our own child —
    // would be the one claimed, and `is_none()` would fail for a reason
    // that has nothing to do with the guard under test.
    //
    // Safe to drain unconditionally: `agentic-runtime`'s `kind(test)` tests
    // run one at a time (see `.config/nextest.toml`'s `serial-db` group), so
    // nothing can add a new row while this loop runs, and no other test ever
    // looks at an earlier test's row again once it has finished.
    while crud::claim_task(&db, "drain-leftover-roots")
        .await
        .unwrap()
        .is_some()
    {}

    let run_id = format!("ev-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "Q", None, "workflow", None, uuid::Uuid::nil())
        .await
        .unwrap();

    // A descendant task, globally claimable but with a parent.
    let child = format!("{run_id}.1");
    crud::enqueue_task(
        &db,
        &child,
        &run_id,
        Some(&run_id),
        &TaskSpec::Agent {
            agent_id: "test-agent".to_string(),
            question: "q".to_string(),
            extra: None,
        },
        None,
        TaskScope::Global,
    )
    .await
    .unwrap();

    let claimed = crud::claim_task(&db, "global-worker").await.unwrap();

    assert!(
        claimed.is_none(),
        "the global path must never claim a leaf in isolation — its outcome \
         would be dropped by a coordinator that does not hold it"
    );
    assert_eq!(row(&db, &child).await.queue_status, "queued");
}

// ── Root eligibility for orphaned scoped work ────────────────────────────────
//
// `mark_released_roots_global` matches on `worker_id` + `queue_status =
// 'claimed'` — both of which `release_claims_for_worker` clears (`worker_id`
// to NULL, status to `queued`). So every test here calls it BEFORE the
// release, matching the real call sites (`spawn_shutdown_hook` in
// `oxy-app`'s `server::router::recovery`, and `release_queue_claims` in
// `cli::commands::worker`), which mark first and drain second. Calling it
// after release (as an earlier draft of this test did) always matches zero
// rows regardless of the `source_type` guard, silently testing nothing.

#[tokio::test(flavor = "multi_thread")]
async fn released_workflow_root_becomes_globally_eligible() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Scoped).await;
    let worker = "w-shutdown";
    claim_or_fail(&db, worker, &task_id).await;

    let marked = crud::mark_released_roots_global(&db, worker).await.unwrap();
    crud::release_claims_for_worker(&db, worker).await.unwrap();

    assert_eq!(marked, 1, "a workflow root should become globally eligible");
    assert!(
        !row(&db, &task_id).await.scope_owned,
        "orphaned workflow root must be claimable by the surviving fleet"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn released_agent_root_stays_scoped() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (_, task_id) = seed_task(&db, "agentic", TaskScope::Scoped).await;
    let worker = "w-shutdown-agent";
    claim_or_fail(&db, worker, &task_id).await;
    // Negative assertion, so the precondition is asserted rather than assumed:
    // `mark_released_roots_global` is scoped to `worker_id` + `claimed`, so a
    // lost claim makes `marked == 0` hold for the wrong reason — and
    // `scope_owned` stays `true` from the seed regardless. Both would be green
    // with the `source_type` guard this test exists for never exercised.
    assert_eq!(
        row(&db, &task_id).await.worker_id.as_deref(),
        Some(worker),
        "precondition: claimed, or `marked == 0` proves nothing about the \
         source_type guard"
    );

    let marked = crud::mark_released_roots_global(&db, worker).await.unwrap();
    crud::release_claims_for_worker(&db, worker).await.unwrap();

    assert_eq!(marked, 0, "agent runs must not be re-driven globally");
    assert!(
        row(&db, &task_id).await.scope_owned,
        "re-driving an interrupted agent run would duplicate LLM calls"
    );
}

/// The whole point of the change, not just "a column flipped": after marking,
/// the global `claim_task` can actually take the released root. Only roots
/// are claimable (Task 4's guard), so this seeds a root with no parent —
/// same as `seed_task` always does.
#[tokio::test(flavor = "multi_thread")]
async fn a_marked_root_is_claimable_by_the_global_path() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // Drain any leftover globally-claimable root rows other tests in this
    // suite left `queued`, for the same reason `global_claim_refuses_a_leaf_task`
    // does: `claim_task` orders by `created_at ASC` across the whole shared
    // table, so an older leftover would be claimed instead of our own row.
    while crud::claim_task(&db, "drain-leftover-roots-2")
        .await
        .unwrap()
        .is_some()
    {}

    let (_, task_id) = seed_task(&db, "workflow", TaskScope::Scoped).await;
    let worker = "w-shutdown-claimable";
    claim_or_fail(&db, worker, &task_id).await;

    crud::mark_released_roots_global(&db, worker).await.unwrap();
    crud::release_claims_for_worker(&db, worker).await.unwrap();

    let claimed = crud::claim_task(&db, "surviving-fleet-worker")
        .await
        .unwrap();

    assert_eq!(
        claimed.map(|c| c.task_id),
        Some(task_id.clone()),
        "the global claim path must be able to pick up the orphaned root \
         once it has been marked globally eligible"
    );
    assert_eq!(
        row(&db, &task_id).await.worker_id.as_deref(),
        Some("surviving-fleet-worker")
    );
}

/// A reviewer mutated `AND parent_task_id IS NULL` out of
/// `mark_released_roots_global`'s SQL and every other test in this section
/// still passed, because they only ever seed roots (`seed_task` always makes
/// `task_id == run_id`, so `parent_task_id` is already `NULL` regardless of
/// the guard). This seeds a *child* instead, under a `workflow` run so
/// `source_type` and `scope_owned` both permit marking — `parent_task_id IS
/// NULL` is the only thing standing between this row and being marked
/// globally eligible.
#[tokio::test(flavor = "multi_thread")]
async fn mark_released_roots_global_refuses_a_claimed_child() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let run_id = format!("ev-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "Q", None, "workflow", None, uuid::Uuid::nil())
        .await
        .unwrap();

    // A scoped child task, claimed by a worker under a workflow run — same
    // shape `global_claim_refuses_a_leaf_task` seeds, but `Scoped` rather than
    // `Global` so `scope_owned = true` too, leaving `parent_task_id IS NULL`
    // as the sole disqualifier.
    let child = format!("{run_id}.1");
    crud::enqueue_task(
        &db,
        &child,
        &run_id,
        Some(&run_id),
        &TaskSpec::Agent {
            agent_id: "test-agent".to_string(),
            question: "q".to_string(),
            extra: None,
        },
        None,
        TaskScope::Scoped,
    )
    .await
    .unwrap();

    let worker = "w-shutdown-child";
    claim_or_fail(&db, worker, &child).await;
    // Asserted, not assumed: this test's claim is a NEGATIVE (`marked == 0`),
    // which holds just as well if the fixture never claimed anything — so a
    // lost claim would pass it vacuously and `claim_or_fail`'s report would be
    // discarded along with the rest of a passing test's output.
    assert_eq!(
        row(&db, &child).await.worker_id.as_deref(),
        Some(worker),
        "precondition: the child must actually be claimed, or `marked == 0` \
         below proves nothing about the `parent_task_id IS NULL` guard"
    );

    let marked = crud::mark_released_roots_global(&db, worker).await.unwrap();

    assert_eq!(
        marked, 0,
        "a claimed descendant must never be marked globally eligible — only \
         roots may be, or its outcome would be dropped by a coordinator that \
         does not hold it"
    );
    assert!(
        row(&db, &child).await.scope_owned,
        "the roots-only guard must refuse the child itself, not merely rely \
         on the global claim path never picking up a leaf"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reap_reports_which_tasks_were_dead_lettered() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let (run_id, doomed) = seed_task(&db, "workflow", TaskScope::Global).await;
    // A worker id unique to this run: the suite shares a database, so a bare
    // 'ghost' could be matched against some other test's row.
    let dying_worker = format!("ghost-{}", uuid::Uuid::new_v4());
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET queue_status = 'claimed', claim_count = max_claims, worker_id = $2 \
         WHERE task_id = $1",
        [doomed.clone().into(), dying_worker.clone().into()],
    ))
    .await
    .unwrap();
    expire_heartbeat(&db, &doomed).await;

    let outcome = crud::reap_stale_tasks(&db).await.unwrap();

    let reaped = outcome
        .dead_tasks
        .iter()
        .find(|t| t.task_id == doomed)
        .unwrap_or_else(|| {
            panic!(
                "dead-lettered rows must be reported so an operator can chase them; got {:?}",
                outcome.dead_tasks
            )
        });

    assert_eq!(
        reaped.run_id, run_id,
        "the dead-lettered row must carry its run id"
    );
    // The whole point of the record: dead-lettering sets `worker_id = NULL`,
    // so a plain `RETURNING worker_id` reports the post-update NULL and the
    // operator-facing log degrades to `worker_id = <none>` — losing exactly
    // the field ("which worker died?") the log exists to answer.
    assert_eq!(
        reaped.worker_id.as_deref(),
        Some(dying_worker.as_str()),
        "the reaped row must name the worker that held the claim, not the NULL \
         it is being reset to"
    );
}

/// `defer_task` must make a task genuinely INVISIBLE to the claim query, not
/// merely re-queued.
///
/// Before `available_at`, the only way to express "not yet" was to claim the
/// task and let its visibility timeout expire — which burns `claim_count`
/// toward `max_claims` and is indistinguishable from a worker that crashed.
/// This pins the property that makes deferral expressible at all.
#[tokio::test]
async fn deferred_task_is_invisible_until_its_window_opens() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (_run_id, task_id) = seed_task(&db, "agent", TaskScope::Global).await;

    let claimed = crud::claim_task_under_root(&db, "w1", &task_id)
        .await
        .unwrap();
    assert!(claimed.is_some(), "precondition: task must be claimable");

    assert_eq!(
        crud::defer_task(&db, &task_id, "w1", 3600, 86_400)
            .await
            .unwrap(),
        crud::DeferOutcome::Deferred,
        "defer must apply to a task this worker holds"
    );

    let r = row(&db, &task_id).await;
    assert_eq!(r.queue_status, "queued", "a deferred task is queued...");
    assert!(r.worker_id.is_none(), "...and unowned");

    // Queued and unowned, yet not claimable — that is the whole point.
    assert!(
        crud::claim_task_under_root(&db, "w2", &task_id)
            .await
            .unwrap()
            .is_none(),
        "a deferred task must not be claimable before available_at"
    );

    // Open the window; it becomes claimable again with no other change.
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET available_at = now() - interval '1 second' \
         WHERE task_id = $1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();
    let after = crud::claim_task_under_root(&db, "w3", &task_id)
        .await
        .unwrap();
    assert_eq!(
        after.map(|t| t.task_id),
        Some(task_id),
        "once available_at has passed the task claims normally"
    );
}

/// A deferral is not an attempt, so it must not spend the retry budget.
///
/// If `claim_count` stayed incremented, an indefinitely-contended task would
/// walk to `max_claims` and dead-letter itself for waiting its turn — the exact
/// outcome deferral exists to avoid.
#[tokio::test]
async fn defer_refunds_the_claim_it_returns() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (_run_id, task_id) = seed_task(&db, "agent", TaskScope::Global).await;

    claim_or_fail(&db, "w1", &task_id).await;
    assert_eq!(row(&db, &task_id).await.claim_count, 1);

    crud::defer_task(&db, &task_id, "w1", 60, 86_400)
        .await
        .unwrap();
    assert_eq!(
        row(&db, &task_id).await.claim_count,
        0,
        "deferral must refund the claim, or contention exhausts max_claims"
    );

    // And a worker that does NOT hold the claim cannot defer it.
    //
    // Re-open the availability window first. The deferral above pushed
    // `available_at` 60s out, and `claim_task_under_root` filters
    // `available_at <= now()` — so without this the re-claim below can never
    // land, the row stays `queued`, and the `NotHeld` asserted next comes back
    // because of `queue_status` rather than because of the `worker_id` scoping
    // this test exists to check. That assertion held with the ownership
    // predicate deleted from `defer_task` entirely.
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET available_at = now() - interval '1 second' \
         WHERE task_id = $1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();
    claim_or_fail(&db, "w1", &task_id).await;
    assert_eq!(
        row(&db, &task_id).await.queue_status,
        "claimed",
        "precondition: the row must be CLAIMED for the next assertion to be about \
         ownership rather than about status"
    );
    assert_eq!(
        crud::defer_task(&db, &task_id, "someone-else", 60, 86_400)
            .await
            .unwrap(),
        crud::DeferOutcome::NotHeld,
        "defer must be scoped to the holding worker"
    );
}

/// An explicit re-enqueue means "run this now" and must clear a prior deferral.
///
/// `available_at` is in the upsert's update set for this reason: without it the
/// re-enqueue resets status and claim count but inherits a deadline its caller
/// never chose, leaving the task silently invisible.
#[tokio::test]
async fn reenqueue_clears_a_pending_deferral() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, task_id) = seed_task(&db, "agent", TaskScope::Global).await;

    claim_or_fail(&db, "w1", &task_id).await;
    crud::defer_task(&db, &task_id, "w1", 3600, 86_400)
        .await
        .unwrap();
    assert!(
        crud::claim_task_under_root(&db, "w2", &task_id)
            .await
            .unwrap()
            .is_none(),
        "precondition: deferred and invisible"
    );

    crud::enqueue_task(
        &db,
        &run_id,
        &task_id,
        None,
        &TaskSpec::Agent {
            agent_id: "test-agent".to_string(),
            question: "q".to_string(),
            extra: None,
        },
        None,
        TaskScope::Global,
    )
    .await
    .unwrap();

    assert_eq!(
        crud::claim_task_under_root(&db, "w3", &task_id)
            .await
            .unwrap()
            .map(|t| t.task_id),
        Some(task_id),
        "a re-enqueued task must be claimable immediately"
    );
}

/// A task that can never run must eventually fail loudly, not wait in silence.
///
/// The whole reason to bound the wait is that a queue which grows quietly
/// looks healthy. Measured in wall clock from the first defer of the streak —
/// not in deferrals, because the retry interval is the domain's choice and can
/// change, so N defers is not a bounded amount of time.
#[tokio::test]
async fn a_task_that_waits_past_its_ceiling_is_dead_lettered() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (_run_id, task_id) = seed_task(&db, "agent", TaskScope::Global).await;

    // First defer starts the streak and stays queued.
    claim_or_fail(&db, "w1", &task_id).await;
    assert_eq!(
        crud::defer_task(&db, &task_id, "w1", 1, 3600)
            .await
            .unwrap(),
        crud::DeferOutcome::Deferred,
        "a fresh streak is nowhere near the ceiling"
    );
    assert_eq!(row(&db, &task_id).await.queue_status, "queued");

    // Age the streak past the ceiling. `first_deferred_at` must survive the
    // intervening defers — if a later defer overwrote it, the streak would
    // reset every hop and never reach any ceiling.
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET first_deferred_at = now() - interval '2 hours', available_at = now() \
         WHERE task_id = $1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();

    claim_or_fail(&db, "w2", &task_id).await;
    assert_eq!(
        crud::defer_task(&db, &task_id, "w2", 1, 3600)
            .await
            .unwrap(),
        crud::DeferOutcome::DeadLettered,
        "past the ceiling the task must be dead-lettered, not deferred again"
    );
    assert_eq!(
        row(&db, &task_id).await.queue_status,
        "dead",
        "and it must actually leave the queue"
    );
}

/// The streak spans consecutive deferrals rather than restarting each time.
#[tokio::test]
async fn the_wait_streak_is_not_reset_by_later_deferrals() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (_run_id, task_id) = seed_task(&db, "agent", TaskScope::Global).await;

    claim_or_fail(&db, "w1", &task_id).await;
    crud::defer_task(&db, &task_id, "w1", 0, 3600)
        .await
        .unwrap();
    let first = row(&db, &task_id).await.first_deferred_at.unwrap();

    claim_or_fail(&db, "w2", &task_id).await;
    // Assert the OUTCOME, not just the `Result` — the idiom both sibling
    // deferral tests already use. The assertion below is that nothing changed,
    // which is exactly the shape that passes when the operation never ran: on a
    // lost or stolen second claim `defer_task` returns `NotHeld`, its UPDATE
    // matches nothing, and `first == second` holds for that reason instead of
    // the one under test. `.unwrap()` alone hides it, since `NotHeld` is an
    // `Ok`. One line, and it discriminates lost and stolen claims together.
    //
    // Exposed rather than theoretical here: the `delay 0` above is what makes
    // the row re-claimable at all, and it is globally claimable — see this
    // test's own cleanup note below.
    assert_eq!(
        crud::defer_task(&db, &task_id, "w2", 0, 3600)
            .await
            .unwrap(),
        crud::DeferOutcome::Deferred,
        "precondition: the second defer must actually run, or the equality \
         below holds because nothing happened"
    );
    let second = row(&db, &task_id).await.first_deferred_at.unwrap();

    assert_eq!(
        first, second,
        "the second defer must not restart the clock, or no ceiling is ever reached"
    );

    // Leave nothing claimable behind. These tests share one database with the
    // whole suite, and `claim_task` is a GLOBAL claim: a stray queued root here
    // gets picked up by another test that assumed it would claim its own task.
    // Deferring with delay 0 (needed above, to re-claim) ends with exactly such
    // a row, so retire it explicitly.
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "DELETE FROM agentic_task_queue WHERE task_id = $1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();
}

/// Reaching the wait ceiling must fail the RUN, not just the queue row.
///
/// The two tests above pin `defer_task`'s own arithmetic, and both stop at the
/// queue: `queue_status == "dead"` and nothing else. That was also where the
/// production behaviour stopped — the transport logged an ERROR and returned,
/// so the run stayed `running` for ever with its only queue row `dead`.
/// `find_pending_global_runs` requires a `queued` row, so nothing selected it
/// again either: a scheduled airway load could stop permanently while the runs
/// list, the API and every alert keyed on a failed run all read "in flight".
///
/// Goes through `WorkerTransport::send` rather than `crud::defer_task`,
/// because the gap was in the arm that interprets the outcome. A test on
/// `defer_task` alone passes either way.
#[tokio::test]
async fn a_dead_lettered_task_fails_its_run() {
    use agentic_core::transport::{WorkerMessage, WorkerTransport};
    use agentic_runtime::transport::{DurableTransport, process_worker_id};
    use sea_orm::EntityTrait;

    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, task_id) = seed_task(&db, "airway", TaskScope::Global).await;

    // The transport defers as `process_worker_id()`, and `defer_task` is
    // ownership-scoped, so the claim has to be held by that same id or the
    // defer is a `NotHeld` no-op and this test passes for the wrong reason.
    claim_or_fail(&db, process_worker_id(), &task_id).await;
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET first_deferred_at = now() - interval '2 hours' WHERE task_id = $1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();

    let transport = DurableTransport::new(db.clone());
    transport
        .send(WorkerMessage::Defer {
            task_id: task_id.clone(),
            delay_secs: 30,
            max_wait_secs: 3600,
            reason: "pipeline yaml not resolvable on this node".to_string(),
        })
        .await
        .expect("a deferral is never an error to the caller");

    assert_eq!(
        row(&db, &task_id).await.queue_status,
        "dead",
        "precondition: past the ceiling the queue row must dead-letter"
    );

    let run = agentic_runtime::entity::run::Entity::find_by_id(run_id.clone())
        .one(&db)
        .await
        .expect("read the run")
        .expect("the run row exists");
    assert_eq!(
        run.task_status.as_deref(),
        Some("failed"),
        "a dead-lettered task must leave its run terminal, not `running` for ever"
    );
    assert!(
        run.error_message
            .as_deref()
            .unwrap_or_default()
            .contains("pipeline yaml not resolvable on this node"),
        "the operator's only account of this is the message, so it must carry \
         the domain's own reason; got {:?}",
        run.error_message
    );
}

/// A dead-lettered CHILD task must retire its ROOT run, not just itself.
///
/// A delegated child's run id **is** its task id
/// (`coordinator::suspension` sets `child_run_id = child_id =
/// "{task_id}.{n}"`), so a run row exists under both `root` and `root.1` and
/// only the ordering in `owning_run_id` decides which is retired. Retiring the
/// child leaves the parent in `delegating` with no live queue row and nothing
/// to wake it — `find_stuck_runs` sweeps only `source_type = 'workflow'` — so
/// the run reads as in flight for ever, one level up from the failure this
/// whole path exists to stop.
///
/// The root is also what `cancel_queued_tasks_for_run` needs in order to
/// cancel the rest of the tree: it matches `task_id = $1 OR task_id LIKE $1 ||
/// '.%'`, which cancels nothing useful when handed a leaf.
#[tokio::test]
async fn a_dead_lettered_child_task_fails_its_root_run() {
    use agentic_core::transport::{WorkerMessage, WorkerTransport};
    use agentic_runtime::transport::{DurableTransport, process_worker_id};
    use sea_orm::EntityTrait;

    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (root_id, _root_task) = seed_task(&db, "automation", TaskScope::Global).await;

    // A delegated child, shaped exactly as the coordinator spawns one.
    let child_id = format!("{root_id}.1");
    crud::insert_child_run(&db, &child_id, &root_id, "child", "airway", 0, None)
        .await
        .expect("seed child run");
    crud::enqueue_task(
        &db,
        &child_id,
        &child_id,
        Some(&root_id),
        &TaskSpec::Agent {
            agent_id: "test-agent".to_string(),
            question: "q".to_string(),
            extra: None,
        },
        None,
        TaskScope::Global,
    )
    .await
    .expect("enqueue child task");

    claim_or_fail(&db, process_worker_id(), &child_id).await;
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET first_deferred_at = now() - interval '2 hours' WHERE task_id = $1",
        [child_id.clone().into()],
    ))
    .await
    .unwrap();

    DurableTransport::new(db.clone())
        .send(WorkerMessage::Defer {
            task_id: child_id.clone(),
            delay_secs: 30,
            max_wait_secs: 3600,
            reason: "pipeline yaml not resolvable on this node".to_string(),
        })
        .await
        .expect("a deferral is never an error to the caller");

    assert_eq!(
        row(&db, &child_id).await.queue_status,
        "dead",
        "precondition: past the ceiling the child's queue row must dead-letter"
    );

    let root = agentic_runtime::entity::run::Entity::find_by_id(root_id.clone())
        .one(&db)
        .await
        .expect("read the root run")
        .expect("the root run row exists");
    assert_eq!(
        root.task_status.as_deref(),
        Some("failed"),
        "the ROOT must go terminal — retiring only the child leaves the parent \
         waiting on a child that will never report"
    );

    // And the child's own row, which retiring only the root would strand.
    // Inert while it sits there — listings filter `parent_run_id IS NULL` —
    // and not permanent: `cleanup_stale_runs`' second pass fails children of
    // a terminal parent. But that runs at `oxy serve` STARTUP only, so
    // reconciliation is a restart rather than a sweep, and the row is
    // non-terminal for as long as the deployment stays up. Closing it in the
    // same transaction is what removes the dependency on that restart.
    let child = agentic_runtime::entity::run::Entity::find_by_id(child_id.clone())
        .one(&db)
        .await
        .expect("read the child run")
        .expect("the child run row exists");
    assert_eq!(
        child.task_status.as_deref(),
        Some("failed"),
        "the child must go terminal too, or a later sweep resurrects it"
    );
}

/// Retiring a run must not rewrite a child that already finished.
///
/// The tree-wide terminal write carries `NOT IN (<terminal>)` for this: a
/// delegation can complete and report before a later sibling strands the root,
/// and overwriting that `done` with `failed` would rewrite history rather than
/// close it. Pinned separately because the widened `UPDATE` is the kind of
/// change where "cover the descendants" and "cover ONLY the unfinished
/// descendants" look identical until someone's completed sub-run is relabelled.
#[tokio::test]
async fn retiring_a_root_leaves_an_already_finished_child_alone() {
    use sea_orm::EntityTrait;

    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (root_id, _root_task) = seed_task(&db, "automation", TaskScope::Global).await;

    let done_child = format!("{root_id}.1");
    crud::insert_child_run(&db, &done_child, &root_id, "child", "airway", 0, None)
        .await
        .expect("seed child run");
    crud::transition_run(&db, &done_child, "done", None, None, None)
        .await
        .expect("the child finished before the root was retired");

    crud::retire_run(&db, &root_id, "exhausted")
        .await
        .expect("retire the root");

    let child = agentic_runtime::entity::run::Entity::find_by_id(done_child.clone())
        .one(&db)
        .await
        .expect("read the child run")
        .expect("the child run row exists");
    assert_eq!(
        child.task_status.as_deref(),
        Some("done"),
        "a child that completed keeps its own outcome; retiring the root closes \
         what is open, it does not rewrite what finished"
    );
}
