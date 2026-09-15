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

use chrono::Utc;
use entity::custom_app_migrations;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use airhouse::sql_rules::{self, Access};

use super::apply::{app_lock_key, pg_detail, read_ledger};
use super::plan::plan;
use super::types::{Applied, DeclaredMigration, MigrationError, STORE_AIRHOUSE};

/// Distinguishes this lock from the OLTP apply's key for the same app, which
/// is taken in a different database but must never be mistaken for this one in
/// `pg_locks`.
const AIRHOUSE_LOCK_SALT: i64 = 0x6169_7268_6f75_7365; // "airhouse"

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
    // Pre-flight against the control plane: nothing to apply means no mint and
    // no tenant connection, and an EDITED file fails before either.
    if plan(declared, &read_ledger(db, app_id, STORE_AIRHOUSE).await?)?.is_empty() {
        return Ok(Applied {
            applied: Vec::new(),
            already_applied: declared.len(),
        });
    }
    let schema = airhouse_schema_for(app_slug)?;

    let pool = db.get_postgres_connection_pool();
    let mut lock = pool
        .begin()
        .await
        .map_err(|e| MigrationError::Db(format!("open the apply lock: {e}")))?;
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(app_lock_key(app_id) ^ AIRHOUSE_LOCK_SALT)
        .fetch_one(&mut *lock)
        .await
        .map_err(|e| MigrationError::Db(format!("acquire the apply lock: {e}")))?;
    if !got {
        return Err(MigrationError::Busy);
    }

    let outcome = apply_locked(
        db,
        app_id,
        app_slug,
        workspace_id,
        build_pk,
        declared,
        &schema,
    )
    .await;
    // Ending the transaction is what releases the lock. A failed rollback is
    // moot: the dropped connection ends the transaction too.
    let _ = lock.rollback().await;
    outcome
}

/// Everything that runs under the lock: re-plan, connect, run, record.
async fn apply_locked(
    db: &DatabaseConnection,
    app_id: Uuid,
    app_slug: &str,
    workspace_id: Uuid,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
    schema: &str,
) -> Result<Applied, MigrationError> {
    let ledger = read_ledger(db, app_id, STORE_AIRHOUSE).await?;
    let pending = plan(declared, &ledger)?;
    let mut outcome = Applied {
        applied: Vec::new(),
        already_applied: declared.len() - pending.len(),
    };
    if pending.is_empty() {
        return Ok(outcome);
    }

    let client = connect_as_app(workspace_id, app_slug, schema).await?;
    client
        .simple_query(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))
        .await
        .map_err(|e| MigrationError::Infra {
            filename: String::new(),
            message: format!("creating schema {schema}: {}", pg_detail(&e)),
        })?;
    info!(%schema, pending = pending.len(), "applying custom-app Airhouse migrations");

    for m in pending {
        run_file(&client, schema, m).await?;
        // Recorded after the tenant commit, for the reason the OLTP path gives:
        // a crash in between re-attempts the file, loudly, rather than skipping
        // a file that never ran.
        record_applied(db, app_id, m, build_pk).await?;
        info!(filename = %m.filename, %schema, "applied custom-app Airhouse migration");
        outcome.applied.push(m.filename.clone());
    }
    Ok(outcome)
}

/// `app_<writer>` for this slug — the same derivation `ctx.airhouse` uses.
pub(super) fn airhouse_schema_for(app_slug: &str) -> Result<String, MigrationError> {
    let writer = oxy_oltp::schema::app_writer_name(app_slug).ok_or_else(|| {
        MigrationError::BadManifest(format!(
            "oxy-app.json declares airhouseMigrations, but the app's slug '{app_slug}' cannot name \
             a schema: a slug must start with a letter, be at most {max} characters, and use only \
             lowercase letters, digits and hyphens",
            max = oxy_oltp::schema::MAX_NAME_LEN,
        ))
    })?;
    oxy_oltp::schema::WriterRef::app(&writer)
        .map(|w| w.schema_name())
        .map_err(|e| MigrationError::BadManifest(e.to_string()))
}

/// Check one file against the schema and DuckLake's rules.
pub(super) fn check_file(
    m: &DeclaredMigration,
    schema: &str,
) -> Result<Vec<String>, MigrationError> {
    sql_rules::check(&m.sql, schema, Access::Ddl).map_err(|e| MigrationError::AirhouseRule {
        filename: m.filename.clone(),
        message: e.to_string(),
    })
}

async fn run_file(
    client: &tokio_postgres::Client,
    schema: &str,
    m: &DeclaredMigration,
) -> Result<(), MigrationError> {
    let statements = check_file(m, schema)?;
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
    m: &DeclaredMigration,
    build_pk: Uuid,
) -> Result<(), MigrationError> {
    custom_app_migrations::ActiveModel {
        app_id: Set(app_id),
        store: Set(STORE_AIRHOUSE.to_string()),
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

    fn file(sql: &str) -> DeclaredMigration {
        DeclaredMigration {
            filename: "0001_init.sql".into(),
            checksum: "x".into(),
            sql: sql.into(),
        }
    }

    #[test]
    fn the_schema_is_the_one_ctx_airhouse_writes() {
        assert_eq!(airhouse_schema_for("store-ops").unwrap(), "app_store_ops");
        assert!(
            airhouse_schema_for("store_ops").is_err(),
            "an underscore aliases a hyphen"
        );
    }

    #[test]
    fn a_file_with_a_key_is_the_authors_to_fix() {
        let err = check_file(
            &file("CREATE TABLE app_store_ops.visits (visit_id VARCHAR PRIMARY KEY)"),
            "app_store_ops",
        )
        .unwrap_err();
        assert!(err.is_author_fault(), "{err}");
        assert!(err.to_string().contains("0001_init.sql"), "{err}");
    }

    #[test]
    fn a_clean_file_splits_into_its_statements() {
        let statements = check_file(
            &file(
                "CREATE TABLE app_store_ops.visits (visit_id VARCHAR NOT NULL, recorded_at TIMESTAMPTZ);
                 CREATE VIEW app_store_ops.latest AS SELECT * FROM app_store_ops.visits;",
            ),
            "app_store_ops",
        )
        .expect("clean");
        assert_eq!(statements.len(), 2);
    }
}
