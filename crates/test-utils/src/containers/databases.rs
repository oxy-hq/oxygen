//! The test databases [`super`] hands out: creating them, marking them, the
//! sweep that drops them once stale, and the shared run database's template.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use sea_orm_migration::MigratorTrait;

use super::{STALE_AFTER, swap_database};

/// Prefix of the `COMMENT ON DATABASE` that marks a database as a test's.
pub(super) const DB_MARKER: &str = "oxy-test-db:";

/// A migrated template for the shared run database, built on first use, or
/// `None` to have the caller start from an empty database instead.
///
/// Named after [`migration_fingerprint`]: the migration names *and* sources,
/// so a new migration and a migration edited in place both get a new
/// template. A cached schema is only trustworthy if editing what produced it
/// invalidates it (internal-docs/testing.md). The template is also marked like
/// any test database, so the sweep retires it after [`STALE_AFTER`] whatever
/// happens.
///
/// Built under a scratch name and renamed when done, so a crash mid-migration
/// leaves marked litter for the sweep rather than a half-migrated template.
/// Callers hold `RUN_DB_LOCK_KEY`, so one process builds; should two ever race
/// anyway, the loser's rename fails and it drops its copy.
pub(super) async fn ensure_template<Runtime: MigratorTrait>(
    admin: &DatabaseConnection,
    server: &str,
) -> Option<String> {
    let template = format!("oxytpl_shared_{:016x}", migration_fingerprint::<Runtime>());
    if database_exists(admin, &template).await {
        return Some(template);
    }

    let scratch = format!("oxytpl_build_{}", uuid::Uuid::new_v4().simple());
    create_marked_database(admin, &scratch, None).await;
    let migrated = match Database::connect(&swap_database(server, &scratch)).await {
        Ok(db) => {
            let result = async {
                migration::Migrator::up(&db, None).await?;
                Runtime::up(&db, None).await
            }
            .await;
            // MUST close before the rename: Postgres refuses to rename, or
            // clone, a database anyone is connected to.
            let _ = db.close().await;
            result.is_ok()
        }
        Err(_) => false,
    };

    if migrated {
        // Retried because a closed session's backend can outlive the close by
        // a moment, and the rename is refused until it's gone.
        for _ in 0..10 {
            let rename = format!("ALTER DATABASE \"{scratch}\" RENAME TO \"{template}\"");
            if exec(admin, &rename).await.is_ok() {
                return Some(template);
            }
            if database_exists(admin, &template).await {
                break; // Another process published first.
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let _ = exec(admin, &format!("DROP DATABASE IF EXISTS \"{scratch}\"")).await;
    database_exists(admin, &template).await.then_some(template)
}

/// Creates `name`, from `template` when given, and marks it with the current
/// time. A database that already exists is left as it is: the shared run
/// database is created by whichever process gets there first.
///
/// A clone that fails (the template was swept, or is busy) falls back to an
/// empty database, which the fixture's own migration then fills.
pub(super) async fn create_marked_database(
    admin: &DatabaseConnection,
    name: &str,
    template: Option<&str>,
) {
    if database_exists(admin, name).await {
        return;
    }
    let cloned = match template {
        Some(t) => exec(
            admin,
            &format!("CREATE DATABASE \"{name}\" TEMPLATE \"{t}\""),
        )
        .await
        .is_ok(),
        None => false,
    };
    if !cloned {
        if let Err(e) = exec(admin, &format!("CREATE DATABASE \"{name}\"")).await {
            // Lost a race to another process creating the same run database.
            if database_exists(admin, name).await {
                return;
            }
            panic!("create test database {name}: {e}");
        }
    }
    let marker = format!("{DB_MARKER}{}", unix_now());
    exec(
        admin,
        &format!("COMMENT ON DATABASE \"{name}\" IS '{marker}'"),
    )
    .await
    .unwrap_or_else(|e| panic!("mark test database {name}: {e}"));
}

/// Drops every marked database older than [`STALE_AFTER`]. Best effort: a
/// database something is still connected to survives until a later sweep.
pub(super) async fn sweep_stale_databases(admin: &DatabaseConnection) {
    let sql = format!(
        "SELECT datname, shobj_description(oid, 'pg_database') AS note FROM pg_database \
         WHERE shobj_description(oid, 'pg_database') LIKE '{DB_MARKER}%'"
    );
    let Ok(rows) = admin
        .query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
    else {
        return;
    };
    let cutoff = unix_now().saturating_sub(STALE_AFTER.as_secs());
    for row in rows {
        let (Ok(name), Ok(note)) = (
            row.try_get::<String>("", "datname"),
            row.try_get::<String>("", "note"),
        ) else {
            continue;
        };
        let created = note
            .strip_prefix(DB_MARKER)
            .and_then(|s| s.parse::<u64>().ok());
        if created.is_some_and(|t| t < cutoff) {
            let _ = exec(admin, &format!("DROP DATABASE IF EXISTS \"{name}\"")).await;
        }
    }
}

pub(super) async fn database_exists(admin: &DatabaseConnection, name: &str) -> bool {
    admin
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 FROM pg_database WHERE datname = $1",
            [name.into()],
        ))
        .await
        .ok()
        .flatten()
        .is_some()
}

pub(super) async fn exec(db: &DatabaseConnection, sql: &str) -> Result<(), sea_orm::DbErr> {
    db.execute_unprepared(sql).await.map(|_| ())
}

/// Hashes the names of central's and `Runtime`'s migrations, and every source
/// file they're built from.
///
/// The sources are read by path, relative to this crate: a type parameter
/// can't say where its file is. A path that isn't there contributes nothing,
/// so a moved file costs a stale-on-edit template, not a failed run, until it's
/// listed here again.
fn migration_fingerprint<Runtime: MigratorTrait>() -> u64 {
    let mut hash = Fnv1a::new();
    for m in migration::Migrator::migrations()
        .iter()
        .chain(Runtime::migrations().iter())
    {
        hash.write(m.name().as_bytes());
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for source in [
        "migration/src",
        "agentic/runtime/src/migration.rs",
        "agentic/runtime/src/migration",
    ] {
        collect_rs_files(&root.join(source), &mut files);
    }
    files.sort();
    for file in files {
        if let Ok(bytes) = std::fs::read(&file) {
            hash.write(&bytes);
        }
    }
    hash.0
}

fn collect_rs_files(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
    } else if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            collect_rs_files(&entry.path(), out);
        }
    }
}

/// FNV-1a, for a template name that's stable across processes and toolchains
/// (`DefaultHasher` promises neither).
struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
        // A separator, so ("ab", "c") and ("a", "bc") differ.
        self.0 = (self.0 ^ 0xff).wrapping_mul(0x0100_0000_01b3);
    }
}

pub(super) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containers::{connect, server_url};

    /// The sweep drops a stale marked database and leaves a fresh one and an
    /// unmarked one alone: the property that makes it safe to point at a real
    /// Postgres.
    #[tokio::test]
    async fn the_sweep_drops_only_stale_marked_databases() {
        let admin = connect(&server_url().await).await;
        let id = uuid::Uuid::new_v4().simple();
        let stale = format!("sweeptest_stale_{id}");
        let fresh = format!("sweeptest_fresh_{id}");
        let unmarked = format!("sweeptest_unmarked_{id}");

        create_marked_database(&admin, &stale, None).await;
        let long_ago = unix_now() - STALE_AFTER.as_secs() - 60;
        exec(
            &admin,
            &format!("COMMENT ON DATABASE \"{stale}\" IS '{DB_MARKER}{long_ago}'"),
        )
        .await
        .unwrap();
        create_marked_database(&admin, &fresh, None).await;
        exec(&admin, &format!("CREATE DATABASE \"{unmarked}\""))
            .await
            .unwrap();

        sweep_stale_databases(&admin).await;

        let left = (
            database_exists(&admin, &stale).await,
            database_exists(&admin, &fresh).await,
            database_exists(&admin, &unmarked).await,
        );
        for db in [&fresh, &unmarked] {
            let _ = exec(&admin, &format!("DROP DATABASE IF EXISTS \"{db}\"")).await;
        }
        assert_eq!(left, (false, true, true), "(stale, fresh, unmarked) exist");
    }
}
