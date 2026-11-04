//! Integration tests for `PostgresTaskRouter` against a real Postgres.
//!
//! Mirrors the testcontainers setup in `integration_tests.rs` so the
//! suite runs out of the box with `cargo nextest run -p agentic-runtime
//! --test integration -E 'test(router_test)'`.

use std::sync::Arc;
use std::time::Duration;

use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use agentic_runtime::router::{PostgresTaskRouter, PostgresTaskRouterOptions, TaskRouter};
use sea_orm::{Database, DatabaseConnection};

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    std::sync::Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

/// Spin up (or reuse) a Postgres testcontainer and return `(url, db)`.
pub(super) async fn test_db() -> Option<(String, DatabaseConnection)> {
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
            let port = container
                .get_host_port_ipv4(5432_u16)
                .await
                .expect("failed to get Postgres port");
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
            Err(_) if attempt < 9 => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => panic!("failed to connect to test database: {e}"),
        }
    }
    let db = db.unwrap();
    // Central then runtime (production order — see oxy_test_utils::migration).
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations failed")
        .finish()
        .await;
    Some((url, db))
}

/// Happy path: `notify_enqueued` wakes a parked `wait_for_task` well
/// inside the backstop timeout.
///
/// 500ms is generous slack on a 100ms backstop — anything close to 100ms
/// would mean the wake came from the timeout, not the notification.
/// We assert < 500ms which proves it came from NOTIFY.
#[tokio::test]
async fn notify_wakes_waiting_caller() {
    let Some((url, db)) = test_db().await else {
        return;
    };

    let factory = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let (router, cancel) = PostgresTaskRouter::start(db, factory);

    // The driver task needs a moment to open the listener connection
    // and issue `LISTEN`. Without this, the very first `notify_enqueued`
    // can race ahead of the listener and the test sees a spurious
    // backstop wake instead of a real notification.
    //
    // 200ms is well below the per-test budget but well above the
    // typical connection setup time (~10-30ms locally).
    tokio::time::sleep(Duration::from_millis(200)).await;

    let waiter_router = Arc::clone(&router);
    let waiter = tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        waiter_router
            .wait_for_task(&[], Duration::from_secs(5))
            .await;
        start.elapsed()
    });

    // Small delay so the waiter is definitely parked before NOTIFY fires.
    tokio::time::sleep(Duration::from_millis(50)).await;
    router.notify_enqueued(Some("io_bound")).await;

    let elapsed = waiter.await.expect("waiter panicked");
    assert!(
        elapsed < Duration::from_millis(500),
        "wake took {elapsed:?}, expected < 500ms (should come from NOTIFY, \
         not the 5s backstop)"
    );

    cancel.cancel();
}

/// Backstop path: with no notification, `wait_for_task` returns when
/// the timeout elapses — the trait contract for the "nothing happened"
/// case. Without this, a hung router could silently break the worker's
/// claim loop.
#[tokio::test]
async fn wait_returns_on_timeout_when_silent() {
    let Some((url, db)) = test_db().await else {
        return;
    };

    let factory = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let (router, cancel) = PostgresTaskRouter::start(db, factory);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let start = tokio::time::Instant::now();
    router.wait_for_task(&[], Duration::from_millis(150)).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(140) && elapsed < Duration::from_millis(500),
        "expected ~150ms backstop, got {elapsed:?}"
    );

    cancel.cancel();
}

/// Two routers + two waiters: a single `notify_enqueued` should wake
/// *every* waiter, not just one. This is the multi-instance shape —
/// each app process holds its own listener, all wake on every NOTIFY,
/// and `SKIP LOCKED` resolves who actually claims a row.
#[tokio::test]
async fn notify_wakes_every_listener() {
    let Some((url, db)) = test_db().await else {
        return;
    };
    let db2 = Database::connect(&url).await.unwrap();

    let factory_a = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let factory_b = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let (router_a, cancel_a) = PostgresTaskRouter::start(db, factory_a);
    let (router_b, cancel_b) = PostgresTaskRouter::start(db2, factory_b);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let a = Arc::clone(&router_a);
    let b = Arc::clone(&router_b);
    let waiter_a = tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        a.wait_for_task(&[], Duration::from_secs(5)).await;
        start.elapsed()
    });
    let waiter_b = tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        b.wait_for_task(&[], Duration::from_secs(5)).await;
        start.elapsed()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Fire from router_a's pool — both A and B should hear it because
    // they both `LISTEN` on the same channel on the same Postgres.
    router_a.notify_enqueued(None).await;

    let elapsed_a = waiter_a.await.unwrap();
    let elapsed_b = waiter_b.await.unwrap();
    assert!(
        elapsed_a < Duration::from_millis(500),
        "router A waiter took {elapsed_a:?}"
    );
    assert!(
        elapsed_b < Duration::from_millis(500),
        "router B waiter took {elapsed_b:?}"
    );

    cancel_a.cancel();
    cancel_b.cancel();
}

/// Force-kill the listener's backend connection mid-flight, then
/// enqueue. The router should reconnect via its backoff loop and
/// resume delivering notifications. Validates that a Postgres
/// failover / `pg_terminate_backend` doesn't leave the router silent.
///
/// Strategy: every PG backend has a `pid` accessible via
/// `pg_backend_pid()`. We can't easily get the listener's pid from
/// outside, but we can broadcast a `pg_terminate_backend` against
/// every backend running a `LISTEN` for our channel. That kills the
/// listener; the reconnect loop should re-establish within a few
/// hundred ms, and the next enqueue should wake the parked waiter.
#[tokio::test]
async fn listener_recovers_from_terminated_backend() {
    use agentic_core::delegation::TaskSpec;
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    let Some((url, db)) = test_db().await else {
        return;
    };
    let factory = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let (router, cancel) = PostgresTaskRouter::start(db.clone(), factory);
    // Let the listener establish before we yank it.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Kill any backend that has issued LISTEN oxy_task_enqueued.
    // `pg_stat_activity.query` is best-effort — different PG versions
    // record LISTEN differently — but `pg_listening_channels` is
    // session-scoped, so we have to match on what the activity view
    // exposes. Most pragmatic match: query text starts with 'LISTEN '.
    db.execute_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE query LIKE 'LISTEN %' AND pid <> pg_backend_pid()"
            .to_string(),
    ))
    .await
    .expect("terminate listener backend");

    // Give the router's reconnect loop time to re-establish.
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Now do an enqueue. If reconnection worked, the parked waiter
    // wakes within ~500ms; if not, we'd hit the 5s timeout.
    let run_id = format!("router-reconnect-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "test", None, "test", None, uuid::Uuid::nil())
        .await
        .expect("insert run");

    let waiter_router = Arc::clone(&router);
    let waiter = tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        waiter_router
            .wait_for_task(&[], Duration::from_secs(5))
            .await;
        start.elapsed()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let spec = TaskSpec::Agent {
        agent_id: "test_agent".into(),
        question: "after reconnect".into(),
        extra: None,
    };
    crud::enqueue_task(
        &db,
        &run_id,
        &run_id,
        None,
        &spec,
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("enqueue task");

    let elapsed = waiter.await.expect("waiter panicked");
    assert!(
        elapsed < Duration::from_millis(1500),
        "wake after listener-kill took {elapsed:?}; reconnect path \
         may not be re-establishing LISTEN"
    );

    cancel.cancel();
}

/// Probe emitted by a router round-trips through Postgres and lands
/// back in the same router's `last_probe_received_at`. Without this
/// the alert plumbing for "matcher pipeline silently broken" wouldn't
/// have a way to verify itself end-to-end.
#[tokio::test]
async fn emit_health_probe_round_trips_to_last_probe_received_at() {
    let Some((url, db)) = test_db().await else {
        return;
    };

    let factory = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let (router, cancel) = PostgresTaskRouter::start(db, factory);
    // Let LISTEN be issued before we fire.
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        router.last_probe_received_at().is_none(),
        "no probe should have been seen before we emit one"
    );

    router.emit_health_probe().await;

    // Wait briefly for NOTIFY → listener → AtomicI64 store.
    let mut latest: Option<std::time::SystemTime> = None;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Some(ts) = router.last_probe_received_at() {
            latest = Some(ts);
            break;
        }
    }
    assert!(
        latest.is_some(),
        "router never recorded a probe receipt within 1s"
    );

    cancel.cancel();
}

/// Two routers + one fires a probe → both record it. Same
/// fan-out-to-all-listeners property the task-enqueue notifications
/// have; covers the multi-instance shape where any instance's probe
/// proves the pipeline for every other instance.
#[tokio::test]
async fn health_probe_fans_out_to_every_listener() {
    use sea_orm::Database;
    let Some((url, db)) = test_db().await else {
        return;
    };
    let db2 = Database::connect(&url).await.unwrap();

    let factory_a = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let factory_b = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let (router_a, cancel_a) = PostgresTaskRouter::start(db, factory_a);
    let (router_b, cancel_b) = PostgresTaskRouter::start(db2, factory_b);
    tokio::time::sleep(Duration::from_millis(300)).await;

    router_a.emit_health_probe().await;

    let mut a_ts = None;
    let mut b_ts = None;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        a_ts = a_ts.or_else(|| router_a.last_probe_received_at());
        b_ts = b_ts.or_else(|| router_b.last_probe_received_at());
        if a_ts.is_some() && b_ts.is_some() {
            break;
        }
    }
    assert!(a_ts.is_some(), "emitting router missed its own probe");
    assert!(b_ts.is_some(), "peer router missed the probe");

    cancel_a.cancel();
    cancel_b.cancel();
}

/// Background task with a short `health_probe_interval` should
/// produce repeated probe receipts. Validates that the tick arm is
/// wired into the select! loop *and* survives multiple cycles (a
/// missing `MissedTickBehavior` setting or an accidentally-consumed
/// `&mut` would break the second fire).
#[tokio::test]
async fn background_task_fires_probes_on_interval() {
    use agentic_runtime::background;

    let Some((url, db)) = test_db().await else {
        return;
    };
    let factory = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let (router, listener_cancel) = PostgresTaskRouter::start(db.clone(), factory);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let router_trait: Arc<dyn TaskRouter> = router.clone();
    let bg_cancel = background::start_with_options(
        db,
        router_trait,
        background::BackgroundJobsOptions {
            // Reaper interval doesn't matter for this test; pick a
            // value that won't fire during the window.
            reaper_interval: Duration::from_secs(60),
            health_probe_interval: Duration::from_millis(150),
        },
    );

    // First probe fires after ~150ms (we drain the immediate tick at
    // background startup). Wait long enough for at least two probes
    // — proves the tick keeps firing, not just the first one.
    tokio::time::sleep(Duration::from_millis(450)).await;

    let first = router.last_probe_received_at().expect("first probe");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let second = router.last_probe_received_at().expect("second probe");
    assert!(
        second > first,
        "second probe timestamp ({second:?}) should advance past first ({first:?})"
    );

    bg_cancel.cancel();
    listener_cancel.cancel();
}

/// Keepalive sanity check. With a 100ms keepalive interval, the
/// listener should issue several `SELECT 1` pings during a 600ms
/// idle window without the connection being dropped — proving the
/// keepalive path runs without panicking, doesn't trip the
/// reconnect loop on its own success, and survives multiple cycles
/// of activity (which it would not if we forgot to handle the
/// MissedTickBehavior or accidentally cancelled the keepalive arm).
#[tokio::test]
async fn listener_keepalive_runs_silently_when_healthy() {
    use agentic_core::delegation::TaskSpec;
    let Some((url, db)) = test_db().await else {
        return;
    };

    let factory = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let options = PostgresTaskRouterOptions {
        keepalive_interval: Duration::from_millis(100),
        ..Default::default()
    };
    let (router, cancel) = PostgresTaskRouter::start_with_options(db.clone(), factory, options);
    // Let the listener establish and let several keepalive ticks fire.
    tokio::time::sleep(Duration::from_millis(600)).await;

    // After all that idle time + keepalive activity, the listener must
    // still be functional: a fresh enqueue should wake a parked waiter.
    // If keepalive had killed or detached the connection, the wake
    // would only happen via the 5s backstop.
    let run_id = format!("router-keepalive-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "test", None, "test", None, uuid::Uuid::nil())
        .await
        .expect("insert run");

    let waiter_router = Arc::clone(&router);
    let waiter = tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        waiter_router
            .wait_for_task(&[], Duration::from_secs(5))
            .await;
        start.elapsed()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let spec = TaskSpec::Agent {
        agent_id: "test_agent".into(),
        question: "after keepalive cycles".into(),
        extra: None,
    };
    crud::enqueue_task(
        &db,
        &run_id,
        &run_id,
        None,
        &spec,
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("enqueue task");

    let elapsed = waiter.await.expect("waiter panicked");
    assert!(
        elapsed < Duration::from_millis(500),
        "wake after several keepalive cycles took {elapsed:?}; \
         keepalive may have broken the listener"
    );

    cancel.cancel();
}

/// End-to-end: a real `enqueue_task` (no manual `notify_enqueued` call)
/// should wake a parked router via the SQL trigger installed in the
/// `AddTaskQueueNotifyTrigger` migration. This is the production path —
/// the trigger fires automatically, the Rust side doesn't have to
/// remember to call NOTIFY at every enqueue site.
#[tokio::test]
async fn enqueue_task_wakes_via_trigger() {
    use agentic_core::delegation::TaskSpec;
    let Some((url, db)) = test_db().await else {
        return;
    };

    let factory = PostgresTaskRouter::password_factory_from_url(&url).expect("valid url");
    let (router, cancel) = PostgresTaskRouter::start(db.clone(), factory);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let waiter_router = Arc::clone(&router);
    let waiter = tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        waiter_router
            .wait_for_task(&[], Duration::from_secs(5))
            .await;
        start.elapsed()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Insert a parent run row first (FK target for the task queue).
    let run_id = format!("router-trigger-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "test", None, "test", None, uuid::Uuid::nil())
        .await
        .expect("insert run");

    // Now enqueue a task. We do NOT call `router.notify_enqueued`;
    // the SQL trigger should fire `pg_notify` itself.
    let spec = TaskSpec::Agent {
        agent_id: "test_agent".into(),
        question: "hello".into(),
        extra: None,
    };
    crud::enqueue_task(
        &db,
        &run_id,
        &run_id,
        None,
        &spec,
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("enqueue task");

    let elapsed = waiter.await.expect("waiter panicked");
    assert!(
        elapsed < Duration::from_millis(500),
        "wake took {elapsed:?}, expected < 500ms via SQL trigger \
         (got the 5s backstop — trigger not firing?)"
    );

    cancel.cancel();
}
