//! The same ledger, applied to the app's schema in its workspace's Airhouse.
//!
//! An app declares `airhouseMigrations: { dir }` for the tables its facts land
//! in (`ctx.airhouse`). The rules are the OLTP path's — once per file, recorded,
//! refused if edited — and three things differ, each forced by DuckLake or
//! Airhouse:
//!
//! - **Every file is parsed and checked** (`airhouse::sql_rules`, `Access::Ddl`)
//!   at declare time and again before it runs: every object in `app_<writer>`,
//!   no keys, `UNIQUE`, indexes or foreign keys. Postgres grants contain the
//!   OLTP path; nothing in DuckDB does.
//! - **Statements run one at a time** inside `BEGIN`/`COMMIT`, because a scoped
//!   credential refuses a multi-statement string.
//! - **The lock lives in Oxy's own Postgres**, as a transaction-scoped advisory
//!   lock held for the whole apply. DuckDB has no advisory locks, and a
//!   transaction-scoped one cannot outlive a cancelled publish on a pooled
//!   connection the way a session lock would.
//!
//! The credential is minted for the app. When Airhouse scopes it, a scoped
//! Writer may run DDL inside its schema. When Airhouse predates scoping, an
//! unscoped Writer cannot `CREATE TABLE`, so the apply mints an Admin for the
//! same app subject and the statement check is the only fence — which is why
//! the check runs on every file, every time.
//!
//! **Where the files run** is an [`AirhouseHome`] (`airhouse_home`): production
//! applies them to `app_<writer>` at promote; every publish also applies them
//! to staging's sibling, `app_<writer>__staging`, under its own ledger target,
//! so staging's applied files never read as production's.

use chrono::Utc;
use entity::custom_app_migrations;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection};
use tokio::time::Instant;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use super::airhouse_home::AirhouseHome;
use super::apply::{app_lock_key, pg_detail, read_ledger};
use super::plan::plan;
use super::types::{Applied, DeclaredMigration, MigrationError, MigrationTarget, STORE_AIRHOUSE};

/// Distinguishes this lock from the OLTP apply's key for the same app, which
/// is taken in a different database but must never be mistaken for this one in
/// `pg_locks`.
const AIRHOUSE_LOCK_SALT: i64 = 0x6169_7268_6f75_7365; // "airhouse"

/// The advisory lock one app's Airhouse apply to `target` holds. Production's
/// is the key every apply held before targets existed; a sibling's is its own,
/// so a staging apply never makes a production promote `Busy`, or wait.
pub fn airhouse_lock_key(app_id: Uuid, target: &MigrationTarget) -> i64 {
    let base = app_lock_key(app_id) ^ AIRHOUSE_LOCK_SALT;
    match target {
        MigrationTarget::Production => base,
        other => base ^ fnv1a(other.as_key().as_bytes()),
    }
}

/// A stable 64-bit FNV-1a hash, never zero for a non-empty target key.
fn fnv1a(bytes: &[u8]) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash as i64
}

/// Apply this bundle's Airhouse migrations to the app's own schema.
///
/// Called from `publish` after the OLTP migrations and **before** the published
/// pointer moves, so a failure leaves the app serving its previous build.
#[instrument(skip(db, declared), fields(app_id = %app_id, app_slug = %app_slug))]
pub(crate) async fn apply_airhouse_on_promote(
    db: &DatabaseConnection,
    app_id: Uuid,
    app_slug: &str,
    workspace_id: Uuid,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
) -> Result<Applied, MigrationError> {
    if declared.is_empty() {
        return Ok(Applied::default());
    }
    let home = AirhouseHome::production(app_slug)?;
    let run = AirhouseRun {
        app_id,
        app_slug,
        workspace_id,
        build_pk,
        start_files_until: None,
    };
    apply_airhouse(db, run, declared, &home).await
}

/// Who an Airhouse apply runs for, and until when it may start a file.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AirhouseRun<'a> {
    pub app_id: Uuid,
    pub app_slug: &'a str,
    pub workspace_id: Uuid,
    pub build_pk: Uuid,
    /// No file **starts** after this instant; a file in progress finishes, so
    /// a deadline never falls between a tenant `COMMIT` and its ledger row.
    /// Files not started are left for the next apply (`Applied::deferred`).
    /// `None`: no deadline.
    pub start_files_until: Option<Instant>,
}

/// Apply this bundle's Airhouse migrations to `environment`'s sibling of the
/// app's schema, creating the sibling on the first apply. Recorded under the
/// sibling's own target, so production's ledger — and so promote's plan — is
/// untouched. Nothing runs for production, or for a slug that names no sibling.
#[instrument(skip(db, declared), fields(app_id = %run.app_id, app_slug = %run.app_slug, %environment))]
pub(crate) async fn apply_airhouse_to_environment(
    db: &DatabaseConnection,
    run: AirhouseRun<'_>,
    declared: &[DeclaredMigration],
    environment: &AppEnvironment,
) -> Result<Applied, MigrationError> {
    if declared.is_empty() {
        return Ok(Applied::default());
    }
    let Some(home) = AirhouseHome::for_environment(run.app_slug, environment)? else {
        warn!(%environment, "no sibling Airhouse schema can be named; its migrations are skipped");
        return Ok(Applied::default());
    };
    apply_airhouse(db, run, declared, &home).await
}

/// Plan against `home`'s ledger, take the app's lock, connect on a credential
/// scoped to `home`'s schema, and apply.
async fn apply_airhouse(
    db: &DatabaseConnection,
    run: AirhouseRun<'_>,
    declared: &[DeclaredMigration],
    home: &AirhouseHome,
) -> Result<Applied, MigrationError> {
    let app_id = run.app_id;
    // Pre-flight against the control plane: nothing to apply means no mint and
    // no tenant connection, and an EDITED file fails before either.
    if plan(
        declared,
        &read_ledger(db, app_id, STORE_AIRHOUSE, home.target()).await?,
    )?
    .is_empty()
    {
        return Ok(Applied {
            already_applied: declared.len(),
            ..Applied::default()
        });
    }

    let pool = db.get_postgres_connection_pool();
    let mut lock = pool
        .begin()
        .await
        .map_err(|e| MigrationError::Db(format!("open the apply lock: {e}")))?;
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(airhouse_lock_key(app_id, home.target()))
        .fetch_one(&mut *lock)
        .await
        .map_err(|e| MigrationError::Db(format!("acquire the apply lock: {e}")))?;
    if !got {
        return Err(MigrationError::Busy);
    }

    let outcome = match connect_as_app(run.workspace_id, run.app_slug, home.schema()).await {
        Ok(client) => {
            let until = run.start_files_until;
            apply_airhouse_over_until(db, app_id, run.build_pk, declared, home, &client, until)
                .await
        }
        Err(e) => Err(e),
    };
    // Ending the transaction is what releases the lock. A failed rollback is
    // moot: the dropped connection ends the transaction too.
    let _ = lock.rollback().await;
    outcome
}

/// Everything that runs under the lock, over `client`: re-plan, create the
/// schema, run each pending file, record it under `home`'s target.
///
/// Public so a test can drive the apply over a connection of its own; the
/// publish path reaches it only through the functions above, which hold the
/// app's lock and connect on the app's scoped credential.
pub async fn apply_airhouse_over(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
    home: &AirhouseHome,
    client: &tokio_postgres::Client,
) -> Result<Applied, MigrationError> {
    apply_airhouse_over_until(db, app_id, build_pk, declared, home, client, None).await
}

/// [`apply_airhouse_over`], starting no file after `until` — one in progress
/// finishes and is recorded; the rest are `deferred` to the next apply.
pub async fn apply_airhouse_over_until(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
    home: &AirhouseHome,
    client: &tokio_postgres::Client,
    until: Option<Instant>,
) -> Result<Applied, MigrationError> {
    let ledger = read_ledger(db, app_id, STORE_AIRHOUSE, home.target()).await?;
    let pending = plan(declared, &ledger)?;
    let mut outcome = Applied {
        already_applied: declared.len() - pending.len(),
        ..Applied::default()
    };
    if pending.is_empty() {
        return Ok(outcome);
    }
    let schema = home.schema();
    client
        .simple_query(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))
        .await
        .map_err(|e| MigrationError::Infra {
            filename: String::new(),
            message: format!("creating schema {schema}: {}", pg_detail(&e)),
        })?;
    info!(%schema, pending = pending.len(), "applying custom-app Airhouse migrations");

    for (started, m) in pending.iter().enumerate() {
        if until.is_some_and(|until| Instant::now() >= until) {
            outcome.deferred = pending[started..]
                .iter()
                .map(|m| m.filename.clone())
                .collect();
            warn!(%schema, deferred = outcome.deferred.len(), "apply deadline passed between files");
            break;
        }
        run_file(client, home, m).await?;
        // Recorded after the tenant commit, for the reason the OLTP path gives:
        // a crash in between re-attempts the file, loudly, rather than skipping
        // a file that never ran.
        record_applied(db, app_id, home, m, build_pk).await?;
        info!(filename = %m.filename, %schema, "applied custom-app Airhouse migration");
        outcome.applied.push(m.filename.clone());
    }
    Ok(outcome)
}

async fn run_file(
    client: &tokio_postgres::Client,
    home: &AirhouseHome,
    m: &DeclaredMigration,
) -> Result<(), MigrationError> {
    let statements = home.statements(m)?;
    let infra = |e: tokio_postgres::Error| MigrationError::Infra {
        filename: m.filename.clone(),
        message: pg_detail(&e),
    };
    client.simple_query("BEGIN").await.map_err(infra)?;
    for statement in &statements {
        if let Err(e) = client.simple_query(statement).await {
            let _ = client.simple_query("ROLLBACK").await;
            return Err(MigrationError::Failed {
                filename: m.filename.clone(),
                message: pg_detail(&e),
            });
        }
    }
    client.simple_query("COMMIT").await.map_err(infra)?;
    Ok(())
}

/// A client on the app's own Airhouse credential: a scoped Writer when
/// Airhouse supports it, otherwise an Admin for the same app subject.
async fn connect_as_app(
    workspace_id: Uuid,
    app_slug: &str,
    schema: &str,
) -> Result<tokio_postgres::Client, MigrationError> {
    let infra = |message: String| MigrationError::Infra {
        filename: String::new(),
        message,
    };
    let endpoint = airhouse::wire_endpoint()
        .ok_or_else(|| infra("Airhouse is not configured on this deployment".into()))?;
    let broker = airhouse::token_broker()
        .ok_or_else(|| infra("the Airhouse token broker is not initialised".into()))?;
    let ttl = airhouse::DEFAULT_INTERNAL_TTL;
    let writer = broker
        .mint_for_app(
            workspace_id,
            app_slug,
            schema,
            airhouse::UserRole::Writer,
            ttl,
        )
        .await
        .map_err(|e| infra(format!("minting the app's Airhouse credential: {e}")))?;
    let cred = if writer.write_schemas.is_some() {
        writer
    } else {
        warn!(
            %workspace_id,
            app = %app_slug,
            %schema,
            "Airhouse cannot scope this app's credential; applying its migrations as an Admin \
             confined by the statement check alone"
        );
        broker
            .mint_for_app(
                workspace_id,
                app_slug,
                schema,
                airhouse::UserRole::Admin,
                ttl,
            )
            .await
            .map_err(|e| infra(format!("minting the app's Airhouse credential: {e}")))?
    };

    let mut config = tokio_postgres::Config::new();
    config
        .host(&endpoint.host)
        .port(endpoint.port)
        .user(&cred.username)
        .password(&cred.password)
        .dbname(&cred.tenant);
    let (client, connection) = config
        .connect(tokio_postgres::NoTls)
        .await
        .map_err(|e| MigrationError::Connect(pg_detail(&e)))?;
    // The driver ends when `client` is dropped at the end of the apply.
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            warn!("airhouse migration connection ended: {e}");
        }
    });
    Ok(client)
}

async fn record_applied(
    db: &DatabaseConnection,
    app_id: Uuid,
    home: &AirhouseHome,
    m: &DeclaredMigration,
    build_pk: Uuid,
) -> Result<(), MigrationError> {
    custom_app_migrations::ActiveModel {
        app_id: Set(app_id),
        store: Set(STORE_AIRHOUSE.to_string()),
        target: Set(home.target().as_key()),
        filename: Set(m.filename.clone()),
        checksum: Set(m.checksum.clone()),
        applied_at: Set(Utc::now().fixed_offset()),
        applied_by_build: Set(Some(build_pk)),
    }
    .insert(db)
    .await
    .map(|_| ())
    .map_err(|e| {
        warn!(filename = %m.filename, error = %e, "Airhouse migration applied but not recorded");
        MigrationError::LedgerWriteFailed {
            filename: m.filename.clone(),
            message: e.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_keeps_its_lock_and_a_sibling_takes_its_own() {
        let app = Uuid::from_u128(42);
        let production = airhouse_lock_key(app, &MigrationTarget::Production);
        assert_eq!(production, app_lock_key(app) ^ AIRHOUSE_LOCK_SALT);
        let staging = MigrationTarget::Schema("app_x__staging".into());
        let sibling = airhouse_lock_key(app, &staging);
        assert_ne!(sibling, production);
        assert_eq!(sibling, airhouse_lock_key(app, &staging), "stable");
    }
}
