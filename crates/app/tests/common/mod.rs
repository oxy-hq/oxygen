//! Shared harness for the `oxy-app` integration binaries.
//!
//! Testcontainers, falling back to `OXY_DATABASE_URL` when it's set (CI's
//! service container). Testcontainers rather than an env-gated skip because a
//! skip means a broken seed passes on a laptop and only fails after push.
//!
//! # Why there is a template database
//!
//! Every DB-backed test wants its own database — they seed conflicting rows and
//! run background sweepers that would otherwise see each other's work. The
//! obvious way to get one is `CREATE DATABASE` followed by `Migrator::up`, and
//! that is what five near-identical copies of this harness used to do. It costs
//! the **full 130+ migration chain per test**, which dominated the runtime of
//! the whole integration suite.
//!
//! Postgres can copy an existing database at the filesystem level
//! (`CREATE DATABASE x TEMPLATE y`), which is roughly a hundred milliseconds and
//! independent of how many migrations produced the source. So the chain runs
//! **once per `cargo nextest run`**, into a template, and every test clones it.
//!
//! Two things make that safe:
//!
//! - **Invalidation is free.** The template is named after `NEXTEST_RUN_ID`, a
//!   UUID nextest mints per run and exports to every test process. A new run
//!   never sees an older run's template, so an edited migration cannot be served
//!   from a stale copy — the failure mode that makes cached schemas untrustworthy.
//!   Runs also can't collide, so `--test-threads` and reused containers are fine.
//! - **It degrades to the old behavior.** Anything that stops the template from
//!   being built — no `NEXTEST_RUN_ID` (a plain `cargo test`), no permission to
//!   `CREATE DATABASE`, an unreachable admin connection — falls back to migrating
//!   the fresh database directly. Worst case this is exactly as slow as before,
//!   never wrong.
//!
//! `CREATE DATABASE ... TEMPLATE` refuses to run while any session is connected
//! to the source, so the build closes its connection before publishing the
//! template, and clones happen under the same advisory lock that guards the
//! build. The lock is held for a file copy, not for a migration chain.
#![allow(dead_code)] // Each test binary uses a different subset.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use uuid::Uuid;

/// Comment-stripping source scans, shared by the coverage tests that read Rust and
/// TypeScript sources (`custom_apps::shape_zoo_coverage`, `custom_apps::canary_coverage`).
pub mod source_scan;

/// Keeps the state dir alive for the whole test binary. Dropping it would
/// delete the bundle bytes mid-test.
static STATE_DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

/// Advisory-lock key guarding template build and clone on the admin database.
/// Distinct from `server::test_support`'s key, which is taken on the *app*
/// database; keys share one namespace per database, so these never meet.
const TEMPLATE_LOCK_KEY: i64 = 0x0787_5EED_7E51;

/// Every prefix this harness has ever given a per-test database. Old runs leave
/// them behind in a reused container, so a run drops the strays it finds — which
/// only works if the list is complete, hence the pre-consolidation names too.
///
/// Deliberately excludes `sa_` / `ahsa_`: those appear in the old airhouse tests
/// as *bearer tokens*, not database names, and a prefix that short in a `DROP
/// DATABASE` sweep is an accident waiting for someone's unrelated database.
const TEST_DB_PREFIXES: &[&str] = &[
    "oxytest_",
    // Pre-consolidation, one per hand-rolled harness.
    "seed_app_",
    "airhouse_prov_",
    "airhouse_broker_",
    "airhouse_lc_",
    "csr_",
    "twcb_",
    // Do NOT add a namespace the *product* names. Staleness here is decided by
    // the absence of a nextest run tag, and only this harness puts one in a
    // database name — so a live tenant (`oxy_org_<uuid>`, from
    // `oltp::provisioner::project_name_for`) would look permanently stale and
    // get dropped, both on a developer's own cluster and mid-run by whichever
    // binary reaches `ensure_template` first. A test that creates such a
    // database owns cleaning it up on the failure path; see `with_fx` in
    // `platform/oltp_provisioner.rs`.
];

/// Image tag for the test Postgres. Also the reuse-label value, so bumping it
/// starts a new container rather than reusing one on the old version.
const POSTGRES_TAG: &str = "18-alpine";

/// Label that scopes container reuse to *our* Postgres. See the comment at the
/// call site — without it, reuse matches any testcontainer on the machine.
const REUSE_LABEL: &str = "tech.oxy.test-postgres";

/// The demo project the seed points workspaces at.
pub fn examples_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// `rel`, relative to the repository root, as a string — for the source-scanning and
/// fixture-reading tests, which otherwise each spell `CARGO_MANIFEST_DIR/../..` themselves.
pub fn read_repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// The demo workspace id `seed_demo` derives — UUID v5 of "demo.oxy.local" in
/// the DNS namespace. Re-derived rather than imported because the seed's helper
/// is private; if that derivation ever changes, this fails loudly, which is the
/// point (the id is documented as stable so saved IDE state stays valid).
pub fn demo_workspace_id() -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"demo.oxy.local")
}

pub const APP_SLUG: &str = "oxy-starter";

/// Which migrators a database needs. Each variant gets its own template, so a
/// test that wants only the central schema never pays for the airhouse tables.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Schema {
    /// The central `migration` crate only.
    Central,
    /// Central plus the airhouse warehouse tables.
    CentralAirhouse,
    /// Central plus the per-org OLTP control-plane tables (`oltp_tenants`,
    /// `oltp_roles`). `oltp_tenants.org_id` carries a real FK to
    /// `organizations`, so the central chain has to be underneath it.
    CentralOltp,
    /// Every migrator `oxy serve` runs — the whole of Oxy's own database, for a
    /// test that reasons about all of it.
    All,
}

impl Schema {
    fn tag(self) -> &'static str {
        match self {
            Schema::Central => "central",
            Schema::CentralAirhouse => "airhouse",
            Schema::CentralOltp => "oltp",
            Schema::All => "all",
        }
    }

    /// Applies the chain. Panics on failure — a migration that cannot apply is a
    /// real defect, not an absent environment.
    async fn migrate(self, db: &DatabaseConnection) {
        Migrator::up(db, None)
            .await
            .expect("run central migrations");
        if self == Schema::All {
            migrate_domains(db).await;
            return;
        }
        if self == Schema::CentralAirhouse {
            airhouse::migration::up(db)
                .await
                .expect("run airhouse migrations");
        }
        if self == Schema::CentralOltp {
            oxy_oltp::migration::up(db)
                .await
                .expect("run oltp migrations");
        }
    }
}

/// The side migrators, in the order `run_all_migrators` (`cli/commands/serve.rs`)
/// runs them: airway's tables reference the runtime's, and OLTP's reference
/// `organizations`.
async fn migrate_domains(db: &DatabaseConnection) {
    agentic_runtime::migration::RuntimeMigrator::up(db, None)
        .await
        .expect("run runtime migrations");
    agentic_pipeline::AnalyticsMigrator::up(db, None)
        .await
        .expect("run analytics migrations");
    agentic_pipeline::AutomationMigrator::up(db, None)
        .await
        .expect("run automation migrations");
    agentic_pipeline::AirwayMigrator::up(db, None)
        .await
        .expect("run airway migrations");
    airhouse::migration::up(db)
        .await
        .expect("run airhouse migrations");
    oxy_oltp::migration::up(db)
        .await
        .expect("run oltp migrations");
    oxy_cameras::CamerasMigrator::up(db, None)
        .await
        .expect("run cameras migrations");
}

/// The admin (`postgres`) connection URL: CI's service container when
/// `OXY_DATABASE_URL` is set, otherwise a reused testcontainer.
pub async fn admin_url() -> String {
    TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url; // CI: reuse the service container.
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;
            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    Arc::new(
                        Postgres::default()
                            .with_tag(POSTGRES_TAG)
                            // MUST stay: `ReuseDirective::Always` on its own matches
                            // ANY container carrying `org.testcontainers.managed-by`
                            // and takes the most recently created one — no image,
                            // name or tag is part of the lookup. So a MinIO (or any
                            // other) testcontainer left running by an unrelated test
                            // gets "reused" as this Postgres, and every DB test then
                            // dies on `PortNotExposed { port: Tcp(5432) }`, which
                            // reads like a broken database rather than a container
                            // mixup. Custom labels *are* part of the lookup filter,
                            // so this pins reuse to our own image + tag.
                            .with_label(REUSE_LABEL, POSTGRES_TAG)
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("start postgres testcontainer (is Docker running?)"),
                    )
                })
                .await;
            let port = container
                .get_host_port_ipv4(5432_u16)
                .await
                .expect("get postgres port");
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone()
}

/// This run's identity, as something safe to put in a database name.
///
/// `None` under a plain `cargo test`, which is what turns the template off.
fn run_tag() -> Option<String> {
    let run_id = std::env::var("NEXTEST_RUN_ID").ok()?;
    Some(
        run_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(16)
            .collect(),
    )
}

/// Fails loudly outside nextest's process-per-test model.
///
/// Every harness here writes `OXY_DATABASE_URL` with `unsafe { set_var }` to
/// point the process-wide `establish_connection()` pool at a per-test database.
/// That is sound only because nextest gives each *test* its own process. Under
/// `cargo test` the tests in a binary are threads sharing one environment, so
/// they would overwrite each other's database URL mid-run — and now that 6–9
/// files share a binary, they'd do it far more often than before the
/// consolidation. Silent cross-talk is the worst version of this; a panic naming
/// the cause is the cheap version.
fn assert_process_per_test() {
    let mode = std::env::var("NEXTEST_EXECUTION_MODE");
    assert_eq!(
        mode.as_deref(),
        Ok("process-per-test"),
        "database-backed tests require nextest's process-per-test isolation \
         (found NEXTEST_EXECUTION_MODE={mode:?}). Run `cargo nextest run`, not \
         `cargo test` — these harnesses set OXY_DATABASE_URL per test, which \
         races across threads in a shared process."
    );
}

/// A fresh, migrated database of its own, plus the URL that reaches it.
///
/// Prefer this over hand-rolling `CREATE DATABASE` + `Migrator::up`: those cost
/// the whole migration chain per test, and this clones a per-run template.
pub async fn fresh_db(schema: Schema) -> (DatabaseConnection, String) {
    assert_process_per_test();

    let admin_url = admin_url().await;
    // The run tag has to be IN the name: `drop_stale_databases` decides what
    // belongs to an older run by looking for it, so a name without one is
    // indistinguishable from a stray and gets swept while it's still in use.
    // `assert_process_per_test` above already established we're under nextest,
    // which sets NEXTEST_RUN_ID alongside NEXTEST_EXECUTION_MODE — so the
    // untagged fallback this used to carry was unreachable, and an unreachable
    // fallback is just a name the sweep can't recognize waiting to be reached.
    let tag = run_tag().expect("nextest sets NEXTEST_RUN_ID with NEXTEST_EXECUTION_MODE");
    let db_name = format!("oxytest_{tag}_{}", Uuid::new_v4().simple());

    let cloned = create_test_database(&admin_url, &db_name, schema).await;
    let url = swap_database(&admin_url, &db_name);

    let db = Database::connect(&url)
        .await
        .expect("connect to per-test database");
    if !cloned {
        // No template available; pay the chain directly. Deliberately outside
        // the advisory lock, so a fallback run still migrates in parallel.
        schema.migrate(&db).await;
    }
    (db, url)
}

/// A per-test database with **nothing applied** — no template, no chain.
///
/// The opposite of [`fresh_db`], and needed by exactly one kind of test: one
/// that reasons about the migration chain itself and so has to drive it a step
/// at a time (`migration_rollback_safety`). Everything else wants `fresh_db`,
/// which is two orders of magnitude cheaper.
pub async fn empty_db() -> (DatabaseConnection, String) {
    assert_process_per_test();

    let admin_url = admin_url().await;
    // Same naming rule as `fresh_db`: the run tag has to be in the name or
    // `drop_stale_databases` cannot tell this from a stray and sweeps it.
    let tag = run_tag().expect("nextest sets NEXTEST_RUN_ID with NEXTEST_EXECUTION_MODE");
    let db_name = format!("oxytest_{tag}_{}", Uuid::new_v4().simple());

    create_plain_database(&admin_url, &db_name).await;
    let url = swap_database(&admin_url, &db_name);
    let db = Database::connect(&url)
        .await
        .expect("connect to per-test database");
    (db, url)
}

/// A migrated, per-test database, with the process pointed at it.
///
/// The env writes are what make the *seed's own* connection land on this
/// database rather than the developer's.
pub async fn test_db() -> DatabaseConnection {
    test_db_with(Schema::Central).await
}

/// [`test_db`] over a chosen schema — for a test whose code under test also
/// reads a side migrator's tables, such as a queued function run's
/// `agentic_runs` / `agentic_task_queue` rows (`Schema::All`).
pub async fn test_db_with(schema: Schema) -> DatabaseConnection {
    let (db, test_url) = fresh_db(schema).await;

    let state_dir = STATE_DIR.get_or_init(|| tempfile::tempdir().expect("state dir"));

    // SAFETY: single-threaded setup, before anything else touches the env.
    // `establish_connection()` is a process-wide OnceCell that reads
    // OXY_DATABASE_URL once, and nextest gives each *test* its own process
    // (`NEXTEST_EXECUTION_MODE=process-per-test`) — so this is what points the
    // seed's own connection at our DB, and it stays isolated even though many
    // tests now share one binary.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        // The seed writes real bundle bytes. Without this it would write them
        // into the developer's actual state dir.
        std::env::set_var("OXY_STATE_DIR", state_dir.path());
        // establish_connection branches on auth mode; an inherited IAM setting
        // connects to nothing.
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
        // The build store refuses a filesystem write unless the role is `all`,
        // and picks S3 whenever a bucket is set. Either would fail the seed for
        // reasons that have nothing to do with the code under test.
        std::env::remove_var("OXY_ROLE");
        std::env::remove_var("OXY_CUSTOMER_APPS_S3_BUCKET");
        // Platform standing is read from these. A developer with either set in
        // their shell (`.env` binds them at seed time) would make the test user
        // STAFF — and the staff path in `user_can_access_app` short-circuits
        // before the org-membership check, so the multi-tenant assertions would
        // pass for the wrong reason, or fail only on someone else's machine.
        std::env::remove_var("OXY_OWNER");
        std::env::remove_var("OXY_GLOBAL_ADMINS");
    }

    db
}

/// Points a connection URL at `db_name`, preserving everything else.
///
/// This is the single chokepoint every DB test goes through, so it does not get
/// to be approximate. A plain `rfind('/')` split silently ate any query string —
/// `…/db?sslmode=require` came back as `…/oxytest_x`, quietly dropping TLS. Not
/// reachable from today's CI URL, which carries no query, but a URL is not a
/// string to slice by hand.
fn swap_database(admin_url: &str, db_name: &str) -> String {
    let mut url = url::Url::parse(admin_url)
        .unwrap_or_else(|e| panic!("admin_url is not a URL: {admin_url} ({e})"));
    url.set_path(db_name);
    url.into()
}

/// Creates `db_name`. Returns `true` when it was cloned from a migrated
/// template (so the caller can skip the chain), `false` when it is empty.
///
/// Every failure path returns `false` after creating a plain database, so a
/// missing template degrades to the previous behavior instead of failing tests.
async fn create_test_database(admin_url: &str, db_name: &str, schema: Schema) -> bool {
    let Some(lock) = TemplateLock::acquire(admin_url).await else {
        create_plain_database(admin_url, db_name).await;
        return false;
    };

    let template = ensure_template(&lock, admin_url, schema).await;
    let cloned = match &template {
        Some(tpl) => exec(
            &lock.0,
            &format!("CREATE DATABASE \"{db_name}\" TEMPLATE \"{tpl}\""),
        )
        .await
        .is_ok(),
        None => false,
    };
    if !cloned {
        let _ = exec(&lock.0, &format!("CREATE DATABASE \"{db_name}\"")).await;
    }
    lock.release().await;
    cloned
}

async fn create_plain_database(admin_url: &str, db_name: &str) {
    let admin = connect_admin(admin_url)
        .await
        .expect("connect to admin database");
    exec(&admin, &format!("CREATE DATABASE \"{db_name}\""))
        .await
        .expect("create per-test database");
}

/// Builds this run's template for `schema` if it isn't there yet, and returns
/// its name. `None` means "no templating available" — the caller migrates.
///
/// Must be called with the advisory lock held.
async fn ensure_template(lock: &TemplateLock, admin_url: &str, schema: Schema) -> Option<String> {
    // Without a run id there is nothing safe to key a cached schema on, and a
    // schema cached under a stale key is exactly the bug worth avoiding.
    let short = run_tag()?;
    let template = format!("oxytpl_{}_{}", schema.tag(), short);

    if database_exists(&lock.0, &template).await {
        return Some(template); // Another test in this run already built it.
    }

    // First test of the run to get here: clear out what earlier runs left in a
    // reused container. Databases are cheap to make and never collected, so
    // without this a long-lived container accumulates one per DB test forever.
    drop_stale_databases(&lock.0, &short).await;

    // Build under a scratch name and rename on success. The rename is the
    // publish step, so a crash mid-migration leaves a scratch database (which
    // the next run sweeps) rather than a half-migrated template that later
    // tests would happily clone.
    //
    // The run tag is in the scratch name for the same reason it is in every
    // other name here: so the sweep can tell "mine, in progress" from "someone
    // else's litter".
    let scratch = format!("oxytpl_build_{short}_{}", Uuid::new_v4().simple());
    exec(&lock.0, &format!("CREATE DATABASE \"{scratch}\""))
        .await
        .ok()?;

    let scratch_url = swap_database(admin_url, &scratch);
    let db = Database::connect(&scratch_url).await.ok()?;
    schema.migrate(&db).await;
    // MUST close before the rename and before any clone: Postgres refuses both
    // while a session is connected to the database.
    let _ = db.close().await;

    exec(
        &lock.0,
        &format!("ALTER DATABASE \"{scratch}\" RENAME TO \"{template}\""),
    )
    .await
    .ok()?;

    Some(template)
}

/// Drops templates and per-test databases belonging to runs other than `keep`.
///
/// The exclusion is a *containment* test, not a suffix test. It used to be
/// `NOT LIKE '%{keep}'`, which happened to work for template names (they end in
/// the run tag) but not for per-test databases (the tag sits in the middle, and
/// originally wasn't there at all). That made every live database of the current
/// run look stale: the `CentralAirhouse` template is built on the first airhouse
/// test, by which point `custom_apps`/`platform` tests already hold `oxytest_*`
/// databases. Connected ones survive — Postgres refuses to drop them — but
/// `create_test_database` releases the advisory lock before `fresh_db` connects,
/// and a database caught in that window was dropped out from under its test
/// (`database "oxytest_…" does not exist`). Same window for a second concurrent
/// nextest run against a reused container.
async fn drop_stale_databases(admin: &DatabaseConnection, keep: &str) {
    // `_` is a single-character wildcard in LIKE, so an unescaped `csr_%` also
    // matches `csrXanything`. This is a DROP DATABASE loop against whatever
    // OXY_DATABASE_URL points at — which can be a developer's real Postgres — so
    // the prefixes get escaped, for the same reason `sa_`/`ahsa_` are not in the
    // list at all. (`oxytpl\_%` was already escaped; the rest were not.)
    let mut patterns: Vec<String> = TEST_DB_PREFIXES
        .iter()
        .map(|p| format!("datname LIKE '{}%'", p.replace('_', "\\_")))
        .collect();
    patterns.push("datname LIKE 'oxytpl\\_%'".to_string());
    let sql = format!(
        "SELECT datname FROM pg_database WHERE ({}) AND datname NOT LIKE '%{keep}%'",
        patterns.join(" OR ")
    );

    let Ok(rows) = admin
        .query_all_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            sql,
        ))
        .await
    else {
        return;
    };
    for row in rows {
        let Ok(name) = row.try_get::<String>("", "datname") else {
            continue;
        };
        // Best effort: another run may hold connections to its own databases.
        let _ = exec(admin, &format!("DROP DATABASE IF EXISTS \"{name}\"")).await;
    }
}

async fn database_exists(admin: &DatabaseConnection, name: &str) -> bool {
    admin
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT 1 FROM pg_database WHERE datname = $1",
            [name.into()],
        ))
        .await
        .ok()
        .flatten()
        .is_some()
}

async fn exec(db: &DatabaseConnection, sql: &str) -> Result<(), sea_orm::DbErr> {
    db.execute_unprepared(sql).await.map(|_| ())
}

/// Retries because a freshly started container accepts TCP before it accepts
/// queries.
async fn connect_admin(admin_url: &str) -> Option<DatabaseConnection> {
    for attempt in 0..10 {
        match Database::connect(admin_url).await {
            Ok(conn) => return Some(conn),
            Err(e) => {
                if attempt == 9 {
                    eprintln!("admin connect failed after 10 attempts: {e}");
                    return None;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
    None
}

/// A held `pg_advisory_lock` on the admin database, on a connection of its own.
struct TemplateLock(DatabaseConnection);

impl TemplateLock {
    async fn acquire(admin_url: &str) -> Option<Self> {
        // A pool of exactly one connection. Advisory locks are per *session*, so
        // on a multi-connection pool the unlock could land on a different
        // connection than the lock — leaving it held until that session closed,
        // and every later process blocked behind it.
        let mut opt = ConnectOptions::new(admin_url.to_string());
        opt.max_connections(1).min_connections(1);

        let mut conn = None;
        for attempt in 0..10 {
            match Database::connect(opt.clone()).await {
                Ok(c) => {
                    conn = Some(c);
                    break;
                }
                Err(e) => {
                    if attempt == 9 {
                        eprintln!("template lock connect failed: {e}");
                        return None;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
        let conn = conn?;
        exec(
            &conn,
            &format!("SELECT pg_advisory_lock({TEMPLATE_LOCK_KEY})"),
        )
        .await
        .ok()?;
        Some(Self(conn))
    }

    async fn release(self) {
        // Best effort: dropping the connection ends the session and Postgres
        // releases the lock anyway. That's also what covers a panicking test —
        // the lock cannot outlive the process holding it, so a failure here
        // can't wedge the rest of the suite.
        let _ = exec(
            &self.0,
            &format!("SELECT pg_advisory_unlock({TEMPLATE_LOCK_KEY})"),
        )
        .await;
        let _ = self.0.close().await;
    }
}
