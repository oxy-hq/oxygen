//! Reverting to an older image must not be blocked by the *newer* image's
//! bookkeeping rows.
//!
//! This reproduces the 0.5.111 → 0.5.110 incident against a real Postgres.
//! Sea-ORM treats the migration ledger as an exact set equality against the
//! binary's compiled-in list, so a row it cannot match aborts the run with
//! "Migration file of version '…' is missing". The migrate step is a blocking
//! pre-upgrade hook, so that abort blocked the rollback outright — while the
//! schema itself was fine.
//!
//! There are **eight** ledger tables, and any one of them can wedge a revert, so
//! the synthetic row goes into all eight. See
//! `internal-docs/revert-safe-migrations.md`.
//!
//! A `mod` of the `platform` group, not a `tests/*.rs` of its own: a new
//! top-level test target links DuckDB, DataFusion, Arrow and the AWS SDK all
//! over again, on every full run.
//!
//! # Not to be confused with `migration_rollback_safety`
//!
//! The sibling with the nearly identical name guards the **schema**: a deploy's
//! migrations must be additive, so the binary you revert to can still read and
//! write what it finds. This one guards the **ledger**: even with a schema the
//! old binary is happy with, a `seaql_migrations` row it cannot match aborts
//! its migrate step before it serves a request.
//!
//! Both have to hold for a revert to work, and neither implies the other — a
//! purely additive release still wedges the rollback through the ledger, which
//! is exactly what 0.5.111 → 0.5.110 was.

use std::collections::BTreeSet;

use agentic_pipeline::{AirwayMigrator, AnalyticsMigrator, AutomationMigrator};
use agentic_runtime::migration::RuntimeMigrator;
use migration::{Migrator, MigratorTrait};
use oxy_cameras::CamerasMigrator;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement};

/// A version no binary will ever compile in — it stands in for "a migration
/// from the release we are rolling back from."
const AHEAD_VERSION: &str = "m29991231_000001_from_the_future";

/// Every ledger `run_all_migrators` touches, in the same order.
///
/// Eight, not seven. `seaql_migrations_oltp` was the one this test did not
/// know about, and `OltpMigrator` was correspondingly the one migrator nobody
/// had wired tolerance into — the test's blind spot and the gap were the same
/// omission, which is why `tolerance_is_wired_into_every_migrator` now exists
/// to catch the next one mechanically.
const LEDGERS: &[&str] = &[
    "seaql_migrations",
    "seaql_migrations_orchestrator",
    "seaql_migrations_analytics",
    "seaql_migrations_workflow",
    "seaql_migrations_airway",
    "seaql_migrations_airhouse",
    "seaql_migrations_oltp",
    "seaql_migrations_cameras",
];

/// Mirrors `run_all_migrators` in `cli/commands/serve.rs` — same migrators, same
/// order (the airway migrator FKs into the runtime's `agentic_runs`, and the
/// airhouse wrapper pre-stamps from the central table, so order is load-bearing).
///
/// Local rather than `common::migrate_domains` because this is the code under
/// test: it must return `Result` so the assertion can be "the revert run does
/// not abort", where the shared helper `expect`s and would turn the finding
/// into a panic inside a helper. The baseline above uses the shared path, so
/// the two cannot disagree about which ledgers exist.
async fn run_all_migrators(db: &DatabaseConnection) -> Result<(), DbErr> {
    Migrator::up(db, None).await?;
    RuntimeMigrator::up(db, None).await?;
    AnalyticsMigrator::up(db, None).await?;
    AutomationMigrator::up(db, None).await?;
    AirwayMigrator::up(db, None).await?;
    airhouse::migration::up(db).await?;
    oxy_oltp::migration::up(db).await?;
    CamerasMigrator::up(db, None).await?;
    Ok(())
}

/// The full contents of one ledger, as `(version, applied_at)`.
async fn ledger_rows(db: &DatabaseConnection, table: &str) -> Vec<(String, i64)> {
    db.query_all_raw(Statement::from_string(
        db.get_database_backend(),
        format!("SELECT version, applied_at FROM \"{table}\" ORDER BY version ASC"),
    ))
    .await
    .unwrap_or_else(|e| panic!("read {table}: {e}"))
    .into_iter()
    .map(|row| {
        (
            row.try_get::<String>("", "version").expect("version"),
            row.try_get::<i64>("", "applied_at").expect("applied_at"),
        )
    })
    .collect()
}

async fn insert_ahead_row(db: &DatabaseConnection, table: &str) {
    db.execute_raw(Statement::from_string(
        db.get_database_backend(),
        format!(
            "INSERT INTO \"{table}\" (version, applied_at) VALUES ('{AHEAD_VERSION}', 4102444800)"
        ),
    ))
    .await
    .unwrap_or_else(|e| panic!("insert ahead row into {table}: {e}"));
}

/// Migration names the central migrator reports as applied.
async fn applied_names(db: &DatabaseConnection) -> BTreeSet<String> {
    Migrator::get_applied_migrations(db)
        .await
        .expect("read applied migrations")
        .iter()
        .map(|m| m.name().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ledger_row_from_a_newer_release_does_not_block_migrations() {
    // Baseline through the group's maintained helper, not the mirror below:
    // `Schema::All` is "every migrator `oxy serve` runs", kept current by
    // everyone, so a ninth migrator's ledger table exists here without this
    // test being edited. The mirror is then only used for the run under test.
    let db = crate::common::test_db_with(crate::common::Schema::All).await;

    // The oracle: what the migrators see before anything is ahead.
    let applied_before = applied_names(&db).await;
    let ledgers_before: Vec<_> = {
        let mut v = Vec::new();
        for table in LEDGERS {
            v.push(ledger_rows(&db, table).await);
        }
        v
    };
    assert!(
        Migrator::get_pending_migrations(&db)
            .await
            .expect("pending read")
            .is_empty(),
        "baseline must be fully migrated"
    );

    // Roll the image back: the newer release's rows are still in every ledger.
    for table in LEDGERS {
        insert_ahead_row(&db, table).await;
    }

    // 1. The revert runs. Before this fix, the *first* migrator aborted with
    //    `DbErr::Custom("Migration file of version '…' is missing")` and the
    //    pre-upgrade hook failed, so the rollback never happened.
    run_all_migrators(&db)
        .await
        .expect("a ledger row from a newer release must not abort the migration run");

    // 2. Nothing was applied or re-applied: same migration set, same
    //    `applied_at` timestamps. This is what proves the fix only changes the
    //    READ — an override that accidentally re-ran a migration would move
    //    `applied_at`, and one that mutated the ledger would change the set.
    assert_eq!(
        applied_names(&db).await,
        applied_before,
        "the applied set for known migrations must be unchanged"
    );
    assert!(
        Migrator::get_pending_migrations(&db)
            .await
            .expect("pending read")
            .is_empty(),
        "the pending set for known migrations must be unchanged"
    );

    // 3. The ahead rows are STILL THERE. Non-destructive is the whole safety
    //    argument: rolling forward again must still see those migrations as
    //    applied, so no migration has to be idempotent for this to be sound.
    for (table, before) in LEDGERS.iter().zip(ledgers_before) {
        let after = ledger_rows(&db, table).await;
        let expected: Vec<(String, i64)> = {
            let mut rows = before;
            rows.push((AHEAD_VERSION.to_owned(), 4_102_444_800));
            rows.sort();
            rows
        };
        assert_eq!(
            after, expected,
            "{table}: the ahead row must be left in place, untouched, alongside every known row"
        );
    }
}
