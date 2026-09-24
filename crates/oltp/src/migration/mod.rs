//! SeaORM migrations for the OLTP-owned tables.
//!
//! Uses a **separate tracking table** (`seaql_migrations_oltp`) so this migrator
//! is independent of the central `crates/migration` migrator, mirroring
//! `airhouse`, `agentic-runtime`, and `agentic-analytics`.
//!
//! Unlike `airhouse::migration`, there is no legacy pre-stamping step: these
//! tables have never lived in the central migrator, so both tracking tables
//! start empty on every deployment.
//!
//! **The squash is free only while this feature is undeployed.** Five
//! migrations became one because nothing has ever run them in production —
//! that is what makes rewriting history harmless. It is not harmless for any
//! database that DID run the earlier build: a review app, a shared staging box,
//! a colleague's laptop. Each needs the one-line trim below, by hand, and the
//! symptom is SeaORM refusing to start, which reads as a broken build rather
//! than a ledger that needs a delete. Fresh databases and CI are unaffected.
//!
//! Squashing again after merge would not be free, and should not be done.
//!
//! The trim — the tables are already in the right shape, so nothing is rebuilt:
//!
//! ```sql
//! DELETE FROM seaql_migrations_oltp
//!  WHERE version <> 'm20260804_000001_create_oltp_tables';
//! ```
//!
//! A fresh database is unaffected.

mod m20260804_000001_create_oltp_tables;

use sea_orm::DatabaseConnection;
use sea_orm_migration::prelude::*;

pub struct OltpMigrator;

#[async_trait::async_trait]
impl MigratorTrait for OltpMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        // ONE migration, deliberately. Nothing has deployed this feature, so
        // the four alters that followed the original create were history with
        // no audience — squashed into the shape the code actually expects.
        vec![Box::new(m20260804_000001_create_oltp_tables::Migration)]
    }

    fn migration_table_name() -> sea_orm::DynIden {
        Alias::new("seaql_migrations_oltp").into_iden()
    }

    oxy_migration_tolerance::tolerate_schema_ahead!();
}

/// Run the OLTP migrator. Call from the host's startup migration sequence
/// **after** the central migrator, since `oltp_tenants` has an FK to
/// `organizations`.
pub async fn up(db: &DatabaseConnection) -> Result<(), DbErr> {
    OltpMigrator::up(db, None).await
}
