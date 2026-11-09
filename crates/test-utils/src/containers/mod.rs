//! The local test containers, and test databases that clean up after themselves.
//!
//! Every helper here defers to `OXY_DATABASE_URL` when it is set — CI's
//! service container, fresh per job, or a database a developer chose — and
//! only otherwise starts a container.
//!
//! # A bare `with_reuse` adopts any container
//!
//! testcontainers' `ReuseDirective::Always` (0.27) picks a container to reuse
//! by **name and labels only** — never image, tag or ports — and takes the
//! newest match, starting it if it's stopped. With neither set, every
//! testcontainer on the machine matches: a MinIO, a ClickHouse, airhouse's
//! Postgres with its own password. The test then dies on `PortNotExposed {
//! port: Tcp(5432) }` or `password authentication failed`, which reads like a
//! broken database rather than a container mixup.
//!
//! [`start_reusable`] pins reuse to a name *and* a label, both derived from the
//! image and tag (`oxy-test-postgres-18-alpine`). Both, because Docker's name
//! filter is a substring match; the fixed name also means two processes racing
//! to start it can't both create one.
//!
//! # A reused container never forgets
//!
//! The shared fixtures (agentic runtime, pipeline, airway) used to write into
//! the container's own `postgres` database, so each run's rows piled on top of
//! every earlier run's, and per-test databases were created and never dropped.
//! Now [`shared_db_url`] gives each `cargo nextest run` a database of its own,
//! cloned from a migrated template, and [`fresh_db_url`] gives one to a test. Both are marked with a comment
//! carrying their creation time, and whichever test next creates one drops
//! every marked database older than [`STALE_AFTER`]. Only marked databases are
//! dropped, so a sweep against a developer's real Postgres can't reach anything
//! a test didn't make.
//!
//! `just test-gc` removes containers a killed run left behind; see the
//! justfile.

use std::time::Duration;

use sea_orm::{Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ContainerRequest, Image, ImageExt, ReuseDirective};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::OnceCell;

use crate::migration::AdvisoryLock;

mod databases;
use databases::{create_marked_database, database_exists, ensure_template, sweep_stale_databases};

/// Image tag for the test Postgres. Part of the container's name, so bumping it
/// starts a new container rather than reusing one on the old version.
pub const POSTGRES_TAG: &str = "18-alpine";

/// The label [`start_reusable`] scopes reuse by. Its value is the container
/// name.
pub const CONTAINER_LABEL: &str = "tech.oxy.test-container";

/// A marked database older than this is dropped by the next sweep. Long enough
/// that no live run loses its database; short enough that a reused container
/// holds a few hours of runs, not all of them.
pub const STALE_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

/// Serializes creating the shared run database and its template. `SHRDTP`;
/// listed with the other keys in [`crate::migration`].
const RUN_DB_LOCK_KEY: i64 = 0x5348_5244_5450;

/// 64 MB (Docker's default) is too small: a parallel plan wants a 32 MB DSM
/// segment and a reused container accumulates them. See
/// internal-docs/workspace-source.md.
const SHM_BYTES: u64 = 1024 * 1024 * 1024;

static POSTGRES: OnceCell<ContainerAsync<Postgres>> = OnceCell::const_new();
static SERVER_URL: OnceCell<String> = OnceCell::const_new();
static SHARED_DB_URL: OnceCell<String> = OnceCell::const_new();

/// The name [`start_reusable`] gives a container for `descriptor`
/// (`image:tag`): `clickhouse/clickhouse-server:25.8-alpine` becomes
/// `oxy-test-clickhouse-server-25.8-alpine`.
pub fn container_name(descriptor: &str) -> String {
    let image = descriptor.rsplit('/').next().unwrap_or(descriptor);
    let safe: String = image
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '-',
        })
        .collect();
    format!("oxy-test-{safe}")
}

/// Starts the container `request` builds, or the one an earlier run started.
///
/// Sets the name, the label and `ReuseDirective::Always`, so `request` needs
/// only the image, tag and configuration. `request` is a builder, not a value,
/// because a request can't be cloned and a start is retried: a process racing
/// another to create the container loses with a name conflict, then finds the
/// winner's container on its next attempt. Only a conflict is retried; any
/// other error (Docker not running) returns at once, so a suite that skips
/// without Docker skips fast.
pub async fn start_reusable<I, R>(request: impl Fn() -> R) -> Result<ContainerAsync<I>, String>
where
    I: Image,
    R: Into<ContainerRequest<I>>,
{
    let name = container_name(&request().into().descriptor());
    let mut last_err = String::new();
    for attempt in 0..5 {
        let req: ContainerRequest<I> = request().into();
        let started = req
            .with_container_name(&name)
            .with_label(CONTAINER_LABEL, &name)
            .with_reuse(ReuseDirective::Always)
            .start()
            .await;
        match started {
            Ok(container) => return Ok(container),
            Err(e) => last_err = e.to_string(),
        }
        if !is_name_conflict(&last_err) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
    }
    Err(format!("start {name} (is Docker running?): {last_err}"))
}

/// Whether a start failed because another process created the same-named
/// container first (Docker answers 409 "...is already in use").
fn is_name_conflict(err: &str) -> bool {
    err.contains("409") || err.contains("Conflict") || err.contains("already in use")
}

/// The shared test Postgres container, started on first use, or why it
/// couldn't be. For a suite that needs the container itself rather than
/// [`server_url`], and skips without Docker; it does not consult
/// `OXY_DATABASE_URL`.
pub async fn try_postgres_container() -> Result<&'static ContainerAsync<Postgres>, String> {
    POSTGRES
        .get_or_try_init(|| async {
            start_reusable(|| {
                Postgres::default()
                    .with_tag(POSTGRES_TAG)
                    .with_shm_size(SHM_BYTES)
            })
            .await
        })
        .await
}

/// [`try_postgres_container`], panicking without Docker.
pub async fn postgres_container() -> &'static ContainerAsync<Postgres> {
    try_postgres_container()
        .await
        .unwrap_or_else(|e| panic!("{e}"))
}

/// The admin URL of the test Postgres server: `OXY_DATABASE_URL` when set,
/// otherwise the `postgres` database of the shared container.
pub async fn server_url() -> String {
    SERVER_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            let port = postgres_container()
                .await
                .get_host_port_ipv4(5432_u16)
                .await
                .expect("test postgres exposes 5432");
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone()
}

/// The database the shared-schema fixtures use for this run.
///
/// `OXY_DATABASE_URL` unchanged when it's set. Otherwise one database per
/// `cargo nextest run`, named after `NEXTEST_RUN_ID`, so every binary in the
/// run shares it (as they always have) and the next run starts empty. A plain
/// `cargo test` has no run id, and gets the server's own database.
///
/// The run's database is cloned from a template already carrying central and
/// `Runtime`'s migrations, so a run doesn't pay the whole chain to start
/// empty; see [`ensure_template`]. The fixture still runs
/// `migration::migrate_shared_test_db` afterwards, which applies the domain
/// migrators and finds everything else done. `Runtime` is
/// `agentic_runtime::migration::RuntimeMigrator`, injected for the same reason
/// as in `migration`.
pub async fn shared_db_url<Runtime: MigratorTrait>() -> String {
    SHARED_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            let server = server_url().await;
            let Some(tag) = run_tag() else {
                return server;
            };
            let name = format!("oxyrun_{tag}");
            // Connect (with retries) before locking: a reused container that
            // was stopped comes back accepting TCP before queries, and the
            // lock's own connect is a single attempt.
            let admin = connect(&server).await;
            // Every process in the run arrives here at once. Without the lock
            // each builds its own template, and nine concurrent migration
            // chains take several times as long as one.
            let lock = AdvisoryLock::acquire(&server, RUN_DB_LOCK_KEY)
                .await
                .unwrap_or_else(|e| panic!("lock the shared test database: {e}"));
            if !database_exists(&admin, &name).await {
                sweep_stale_databases(&admin).await;
                let template = ensure_template::<Runtime>(&admin, &server).await;
                create_marked_database(&admin, &name, template.as_deref()).await;
            }
            let _ = admin.close().await;
            lock.release().await;
            swap_database(&server, &name)
        })
        .await
        .clone()
}

/// A new, empty database of its own on the test server, named `<prefix>_<uuid>`.
///
/// Marked, so the sweep drops it once it's stale; a test doesn't have to.
pub async fn fresh_db_url(prefix: &str) -> String {
    let server = server_url().await;
    let name = format!("{prefix}_{}", uuid::Uuid::new_v4().simple());
    let admin = connect(&server).await;
    sweep_stale_databases(&admin).await;
    create_marked_database(&admin, &name, None).await;
    let _ = admin.close().await;
    swap_database(&server, &name)
}

/// Connects to `url`, retrying while a just-started container accepts TCP but
/// not yet queries.
pub async fn connect(url: &str) -> DatabaseConnection {
    let mut last_err = None;
    for _ in 0..20 {
        match Database::connect(url).await {
            Ok(conn) => return conn,
            Err(e) => last_err = Some(e),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("connect to test database: {last_err:?}")
}

/// Points `url` at `db_name`, keeping credentials, host and query string.
pub fn swap_database(url: &str, db_name: &str) -> String {
    let mut url =
        url::Url::parse(url).unwrap_or_else(|e| panic!("not a database URL: {url} ({e})"));
    url.set_path(db_name);
    url.into()
}

/// This nextest run's id, reduced to something safe in a database name.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_name_conflict_is_retried() {
        assert!(is_name_conflict(
            "Docker responded with status code 409: Conflict. The container name \"/oxy-test-postgres-18-alpine\" is already in use"
        ));
        assert!(!is_name_conflict(
            "Error in the hyper legacy client: client error (Connect)"
        ));
    }

    #[test]
    fn container_names_come_from_the_image_and_tag() {
        assert_eq!(
            container_name("postgres:18-alpine"),
            "oxy-test-postgres-18-alpine"
        );
        assert_eq!(
            container_name("clickhouse/clickhouse-server:25.8-alpine"),
            "oxy-test-clickhouse-server-25.8-alpine"
        );
    }

    #[test]
    fn swapping_the_database_keeps_the_query_string() {
        assert_eq!(
            swap_database("postgresql://u:p@h:5/postgres?sslmode=require", "x"),
            "postgresql://u:p@h:5/x?sslmode=require"
        );
    }
}
