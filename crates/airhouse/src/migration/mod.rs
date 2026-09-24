//! SeaORM migrations for the airhouse-owned tables.
//!
//! Uses a **separate tracking table** (`seaql_migrations_airhouse`) so this
//! migrator is fully independent of the central `crates/migration` migrator,
//! mirroring the pattern used by `agentic-runtime` and `agentic-analytics`.
//!
//! # Upgrade path
//!
//! These three migrations originally lived in the central `migration` crate
//! and were tracked in `seaql_migrations`. On deployments that ran the
//! central migrator before this move, the airhouse tables already exist and
//! their migration entries are recorded in the central tracking table — but
//! the new `seaql_migrations_airhouse` table is empty, so a naïve
//! `AirhouseMigrator::up` call would re-attempt them and fail (`CREATE TABLE
//! airhouse_tenants` against an existing table).
//!
//! [`up`] handles this by pre-stamping `seaql_migrations_airhouse` from the
//! central tracking table for any of the three known migration names that
//! are already recorded there. New deployments see no rows in either
//! tracking table and run the migrations normally.

mod m20260429_000001_create_airhouse_tenants;
mod m20260429_000002_create_airhouse_users;
mod m20260430_000001_airhouse_workspace_rebind;
mod m20260507_000001_airhouse_tenants_sa;
mod m20260508_000001_drop_airhouse_users;

use sea_orm::{ConnectionTrait, DatabaseConnection};
use sea_orm_migration::prelude::*;

pub struct AirhouseMigrator;

#[async_trait::async_trait]
impl MigratorTrait for AirhouseMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260429_000001_create_airhouse_tenants::Migration),
            Box::new(m20260429_000002_create_airhouse_users::Migration),
            Box::new(m20260430_000001_airhouse_workspace_rebind::Migration),
            Box::new(m20260507_000001_airhouse_tenants_sa::Migration),
            Box::new(m20260508_000001_drop_airhouse_users::Migration),
        ]
    }

    fn migration_table_name() -> sea_orm::DynIden {
        Alias::new("seaql_migrations_airhouse").into_iden()
    }

    oxy_migration_tolerance::tolerate_schema_ahead!();
}

/// Run the airhouse migrator, pre-stamping its tracking table from the
/// central one so existing deployments don't re-run already-applied
/// migrations. Call this from the host's startup migration sequence after
/// the central migrator has run.
pub async fn up(db: &DatabaseConnection) -> Result<(), DbErr> {
    sync_legacy_tracking(db).await?;
    AirhouseMigrator::up(db, None).await
}

/// Names of the three airhouse migrations that originally lived in the
/// central `migration` crate. If any of these are recorded in the central
/// `seaql_migrations` table, copy them into `seaql_migrations_airhouse`
/// before the migrator runs so it knows they're already applied.
const LEGACY_MIGRATION_NAMES: &[&str] = &[
    "m20260429_000001_create_airhouse_tenants",
    "m20260429_000002_create_airhouse_users",
    "m20260430_000001_airhouse_workspace_rebind",
];

async fn sync_legacy_tracking(db: &DatabaseConnection) -> Result<(), DbErr> {
    // Create the airhouse tracking table if it doesn't exist. Schema matches
    // what `sea_orm_migration` creates internally on first `up()` — doing it
    // ourselves first lets us pre-populate before the migrator inspects it.
    db.execute_unprepared(
        "CREATE TABLE IF NOT EXISTS seaql_migrations_airhouse (
            version VARCHAR NOT NULL PRIMARY KEY,
            applied_at BIGINT NOT NULL
        )",
    )
    .await?;

    // Skip the copy if the central tracking table doesn't exist (fresh
    // deployment that never ran the central migrator yet — unlikely, but
    // protects test setups).
    let central_exists = db
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT 1 FROM information_schema.tables WHERE table_name = $1 LIMIT 1",
            ["seaql_migrations".into()],
        ))
        .await?
        .is_some();
    if !central_exists {
        return Ok(());
    }

    let names_quoted = LEGACY_MIGRATION_NAMES
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO seaql_migrations_airhouse (version, applied_at)
         SELECT version, applied_at
         FROM seaql_migrations
         WHERE version IN ({names_quoted})
         ON CONFLICT (version) DO NOTHING"
    );
    db.execute_unprepared(&sql).await?;
    Ok(())
}
