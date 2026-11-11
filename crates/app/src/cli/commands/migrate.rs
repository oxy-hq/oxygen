use agentic_pipeline::{AirwayMigrator, AnalyticsMigrator, AutomationMigrator};
use agentic_runtime::migration::RuntimeMigrator;
use migration::{Migrator, MigratorTrait};
use oxy::database::client::establish_connection;
use oxy_cameras::CamerasMigrator;
use oxy_shared::errors::OxyError;
use std::time::{Duration, Instant};

/// Run ALL database migrations to the latest version.
///
/// Delegates to [`run_database_migrations`], which
/// runs every domain migrator (core SeaORM + runtime + analytics + workflow +
/// airway + airhouse + cameras) under a process-serialising Postgres advisory
/// lock — the SAME complete set `oxy serve` runs. The previous implementation
/// ran only the core `Migrator`, so a `migrate`-as-Job left the domain tables
/// uncreated.
///
/// `oxy migrate` is the dedicated one-shot entrypoint — e.g. a Helm
/// pre-install/pre-upgrade Job — so the serving pods can boot with
/// `OXY_SKIP_MIGRATIONS=1` instead of each racing the migrator.
///
/// That Job is never the single process, and the `oxy-app` chart wires
/// `encryptionKey.secretName` into the pods but not into the Job. A migration that seals a
/// value (the camera `device_secret` backfill does) must then fail, naming the
/// variable, rather than seal it under a key file minted inside the Job.
pub async fn migrate() -> Result<(), OxyError> {
    oxy::utils::forbid_key_file_creation("oxy migrate");
    // With the key in KMS-sealed `server_keys`, the Job needs no variable at
    // all: open the row before a migration can seal anything. Only without the
    // variable: while it is set the Job uses it and never calls KMS, so its own
    // service account needs no KMS grant until then. The Job never seals a new
    // row; that stays with `oxy serve` / `oxy worker`, which compare it. A
    // fresh database has no `server_keys` yet, and nothing to seal either.
    if std::env::var(oxy::utils::ENCRYPTION_KEY_VAR).is_err() {
        crate::server::shared_state_check::open_master_key()
            .await
            .map_err(OxyError::RuntimeError)?;
    }
    run_database_migrations(false).await
}

/// A fixed, process-independent key for the Postgres session-level advisory lock
/// that serialises startup migrations. Every oxy process uses this same key, so
/// concurrent migrators (the split fleet starting the ide + serve nodes
/// together, or `oxy start` bringing up serve + a worker) take turns instead of
/// racing the non-idempotent `CREATE TYPE` enum DDL — which otherwise crashes one
/// node with "duplicate key value violates unique constraint
/// pg_type_typname_nsp_index". The value is arbitrary; it only has to be
/// distinct from the OTHER advisory keys oxy uses in the SAME single-bigint
/// space — Postgres does not separate the session (`pg_advisory_lock`) and
/// transaction (`pg_try_advisory_xact_lock`) variants into different
/// namespaces. The per-workspace lazy-compile lock in
/// `server/api/middlewares/workspace_context.rs` is the only other user; it
/// keys off `workspace_id & 0x7fff_ffff_ffff_ffff`, so a workspace UUID whose
/// low 63 bits equal this key COULD collide (~1-in-2^63). The collision is
/// benign: a lazy-compile that loses this lock during the brief boot-migration
/// window just skips its non-blocking dedup and re-attempts on the next
/// request — no lost work. Pick future advisory keys deliberately rather than
/// assuming this space is otherwise empty.
const MIGRATION_ADVISORY_LOCK_KEY: i64 = 0x0078_795F_6D69_6772; // "\0xy_migr"

/// How long [`run_database_migrations`] polls for [`MIGRATION_ADVISORY_LOCK_KEY`]
/// before giving up.
///
/// Must comfortably exceed how long a full `run_all_migrators` can take on a
/// cold database — a release can ship several `CREATE INDEX CONCURRENTLY`
/// migrations at once (see `migration::m20260911_000002_*`), each of which
/// waits out every other session's in-flight transaction before it can
/// finish. The pool's own startup timeouts (`ACQUIRE_TIMEOUT` = 30s,
/// `CONNECT_TIMEOUT` = 10s, `oxy_platform::db::client`) bound a single
/// connection operation, not "wait for a sibling node's entire migration
/// run" — this is deliberately much larger. Past this bound, either a
/// migrator died while holding the lock or something else is wrong, and the
/// node should fail its boot loudly rather than hang.
const MIGRATION_LOCK_WAIT_BOUND: Duration = Duration::from_secs(300);

/// First retry delay when polling the lock; doubles up to
/// [`MIGRATION_LOCK_POLL_MAX_BACKOFF`] between attempts.
const MIGRATION_LOCK_POLL_INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const MIGRATION_LOCK_POLL_MAX_BACKOFF: Duration = Duration::from_secs(1);

/// How often, while waiting, to print that we're still waiting.
const MIGRATION_LOCK_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// True when `OXY_SKIP_MIGRATIONS` is set. The long-lived pods — serve, compile,
/// worker — consult this so a dedicated migrate Job / the canonical StatefulSet
/// migrator owns the schema and the rest skip on boot (uniform intent across the
/// fleet). `oxy migrate` itself NEVER consults this — it always runs the full
/// migrator, since it IS the dedicated migrator.
pub fn skip_migrations_requested() -> bool {
    std::env::var("OXY_SKIP_MIGRATIONS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub async fn run_database_migrations(_enterprise: bool) -> Result<(), OxyError> {
    println!("migrations: establishing database connection (this builds the connection pool)");
    let db = establish_connection()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Failed to connect to database: {}", e)))?;

    // Serialise migrations across processes with a session-level advisory lock.
    // Multiple oxy nodes booting together (the split fleet, or `oxy start`'s
    // serve + worker) would otherwise run these migrators concurrently, and the
    // `CREATE TYPE` enum migrations are NOT concurrency-safe → a duplicate-key
    // crash on `pg_type`. Holding the lock makes the losers WAIT, then find every
    // migration already applied (a no-op). The lock is held on one dedicated
    // pooled connection (pool max is 80, so it never starves the migrators) and
    // released even when a migrator fails, so a failure can't wedge other nodes
    // behind a stuck lock. A process that dies mid-migration drops its
    // connection, and Postgres releases the lock on disconnect too.
    //
    // A waiter POLLS for the lock (`acquire_migration_lock`) rather than
    // blocking inside `pg_advisory_lock` — see that function's docs for why a
    // blocking wait here can deadlock against this very migrator's own
    // `CREATE INDEX CONCURRENTLY` migrations.
    let mut lock_conn = db
        .get_postgres_connection_pool()
        .acquire()
        .await
        .map_err(|e| {
            OxyError::RuntimeError(format!(
                "migrations: failed to acquire lock connection: {e}"
            ))
        })?;
    acquire_migration_lock(&mut lock_conn).await?;
    println!("migrations: advisory lock held, running SeaORM migrations");

    let result = run_all_migrators(&db).await;

    if let Err(e) = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_ADVISORY_LOCK_KEY)
        .execute(&mut *lock_conn)
        .await
    {
        tracing::warn!("migrations: failed to release advisory lock (frees on disconnect): {e}");
    }

    result
}

/// Polls `pg_try_advisory_lock` for [`MIGRATION_ADVISORY_LOCK_KEY`] rather
/// than blocking in `pg_advisory_lock`.
///
/// A blocking wait holds this connection's implicit transaction — and the
/// snapshot that comes with it — open for however long the wait takes. This
/// migrator's own `run_all_migrators` includes `CREATE INDEX CONCURRENTLY`
/// migrations (`use_transaction() -> Some(false)`), which must wait for
/// every *other* session's open snapshot to clear before they can finish. So
/// a second node blocking here while waiting for this lock is a snapshot the
/// lock holder's own concurrent index build is waiting on — the holder can't
/// finish migrating, can't release the lock, and the waiter can't stop
/// waiting. That snapshot wait IS a real, trackable lock-manager wait —
/// `WaitForOlderSnapshots` goes through `VirtualXactLock` and shows up in
/// `pg_locks` as a `virtualxid` wait — so the deadlock detector isn't blind
/// to it. What it can't see is the OTHER leg: `lock_conn` is its own idle,
/// separate connection, which isn't waiting on anything from Postgres's
/// point of view. "This connection only releases once `db`'s migration
/// finishes" is a dependency this function's own code enforces, not one any
/// backend-to-backend wait expresses — so the cycle has no edge back to the
/// holder, and the detector reports nothing wrong.
///
/// Polling instead keeps every attempt to one short statement, so no
/// snapshot is ever held open between attempts and the cycle can't form.
/// Logs progress every [`MIGRATION_LOCK_LOG_INTERVAL`] and fails with a
/// named cause after [`MIGRATION_LOCK_WAIT_BOUND`] rather than hanging.
async fn acquire_migration_lock(lock_conn: &mut sqlx::PgConnection) -> Result<(), OxyError> {
    let start = Instant::now();
    let mut backoff = MIGRATION_LOCK_POLL_INITIAL_BACKOFF;
    let mut next_log = MIGRATION_LOCK_LOG_INTERVAL;
    loop {
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(MIGRATION_ADVISORY_LOCK_KEY)
            .fetch_one(&mut *lock_conn)
            .await
            .map_err(|e| {
                OxyError::RuntimeError(format!("migrations: failed to poll advisory lock: {e}"))
            })?;
        if locked {
            return Ok(());
        }

        let elapsed = start.elapsed();
        if elapsed >= MIGRATION_LOCK_WAIT_BOUND {
            return Err(OxyError::RuntimeError(format!(
                "migrations: advisory lock {MIGRATION_ADVISORY_LOCK_KEY} still held after \
                 {:.0}s — another node's migrator likely died while holding it, or is still \
                 migrating a cold database. Giving up rather than hanging.",
                elapsed.as_secs_f64()
            )));
        }
        if elapsed >= next_log {
            println!(
                "migrations: still waiting for the advisory lock (held by another node) — \
                 {:.0}s elapsed",
                elapsed.as_secs_f64()
            );
            next_log += MIGRATION_LOCK_LOG_INTERVAL;
        }
        tokio::time::sleep(jittered_backoff(backoff)).await;
        backoff = (backoff * 2).min(MIGRATION_LOCK_POLL_MAX_BACKOFF);
    }
}

/// Equal-jitter backoff: half of `backoff` fixed, half random — so several
/// nodes waiting on the same lock don't retry in lockstep.
fn jittered_backoff(backoff: Duration) -> Duration {
    let half_ms = (backoff.as_millis() as u64 / 2).max(1);
    Duration::from_millis(half_ms + rand::random_range(0..=half_ms))
}

/// Run every domain's migrator in dependency order. The caller holds the
/// migration advisory lock — see [`run_database_migrations`].
async fn run_all_migrators(db: &sea_orm::DatabaseConnection) -> Result<(), OxyError> {
    // Run SeaORM migrations for PostgreSQL
    Migrator::up(db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Failed to run database migrations: {}", e)))?;
    println!("migrations: SeaORM migrations complete");

    // Run orchestrator runtime migrations (separate tracking table).
    RuntimeMigrator::up(db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("runtime migrations failed: {}", e)))?;
    println!("migrations: runtime migrations complete");

    // Run analytics domain extension migrations (separate tracking table).
    AnalyticsMigrator::up(db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("analytics migrations failed: {}", e)))?;
    println!("migrations: analytics migrations complete");

    // Run automation state migrations (separate tracking table).
    AutomationMigrator::up(db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("workflow migrations failed: {}", e)))?;
    println!("migrations: workflow migrations complete");

    // Run airway extension migrations (separate tracking table). Must
    // follow the runtime migrator — `airway_run_extensions.run_id` FKs
    // to `agentic_runs.id`.
    AirwayMigrator::up(db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("airway migrations failed: {}", e)))?;
    println!("migrations: airway migrations complete");

    // Run airhouse migrations (separate tracking table). The wrapper pre-stamps
    // `seaql_migrations_airhouse` from the central tracking table so existing
    // deployments don't re-run migrations whose tables already exist.
    airhouse::migration::up(db)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("airhouse migrations failed: {}", e)))?;
    println!("migrations: airhouse migrations complete");

    // Per-org OLTP migrations (separate tracking table). Must follow the
    // central migrator — `oltp_tenants.org_id` FKs to `organizations.id`.
    oxy_oltp::migration::up(db)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("oltp migrations failed: {}", e)))?;
    println!("migrations: oltp migrations complete");

    // Camera fleet domain migrations (separate tracking table). No FK deps on
    // other domains — sites.workspace_id is a loose UUID per
    // domain-boundaries.md P3.
    //
    // We intentionally do NOT register a post-provision hook for the
    // cameras DDL: provisioning Airhouse is "this workspace wants a
    // data warehouse" — not "this workspace wants cameras". Eagerly
    // creating `oxy_cam_*` tables in every tenant would clutter
    // warehouses that never touch the camera fleet. Instead, the DDL
    // fires lazily at camera-intent points (see
    // `oxy_cameras::service::onboarding::import` and
    // `oxy_cameras::service::registration::register_edge_box`), with
    // the lazy ensure on the ingest path
    // (`oxy_cameras::airhouse::connect_and_ensure`) as a final safety
    // net.
    CamerasMigrator::up(db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("cameras migrations failed: {}", e)))?;
    println!("migrations: cameras migrations complete");

    // Observability schema (DuckDB / Postgres / ClickHouse) is initialized by
    // the backend itself during `*Storage::open()` in `main.rs`, so no separate
    // migration step is needed here.

    Ok(())
}
