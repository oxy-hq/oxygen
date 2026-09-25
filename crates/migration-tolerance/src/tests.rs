//! Unit tests. No database: everything here runs on canned ledger rows.
//!
//! The real read (SQL, quoting, ordering) is covered end-to-end against
//! Postgres by `crates/app/tests/migration_rollback_tolerance.rs`.

use std::collections::HashSet;

use super::partition_known;
use sea_orm_migration::seaql_migrations::Model;

const KNOWN_A: &str = "m20220101_000001_first";
const KNOWN_B: &str = "m20260722_000001_app_visibility_and_members";
/// Stands in for a migration that only exists in a newer release.
const AHEAD: &str = "m29991231_000001_from_the_future";

fn row(version: &str, applied_at: i64) -> Model {
    Model {
        version: version.to_owned(),
        applied_at,
    }
}

fn known_set(versions: &[&str]) -> HashSet<String> {
    versions.iter().map(|v| (*v).to_owned()).collect()
}

// ── The filter ───────────────────────────────────────────────────────────────

#[test]
fn ahead_rows_are_split_out_and_known_rows_keep_their_order() {
    let (known, ahead) = partition_known(
        vec![row(KNOWN_A, 1), row(KNOWN_B, 2), row(AHEAD, 3)],
        &known_set(&[KNOWN_A, KNOWN_B]),
    );

    assert_eq!(
        known.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(),
        vec![KNOWN_A, KNOWN_B],
        "known rows must come back in the order the query returned them"
    );
    assert_eq!(
        known.iter().map(|r| r.applied_at).collect::<Vec<_>>(),
        vec![1, 2],
        "known rows must be passed through untouched, applied_at included"
    );
    assert_eq!(
        ahead.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(),
        vec![AHEAD]
    );
}

#[test]
fn a_ledger_with_nothing_ahead_is_returned_verbatim() {
    let ledger = vec![row(KNOWN_A, 1), row(KNOWN_B, 2)];
    let (known, ahead) = partition_known(ledger.clone(), &known_set(&[KNOWN_A, KNOWN_B]));

    assert_eq!(known, ledger, "the common case must be a pure pass-through");
    assert!(ahead.is_empty());
}

#[test]
fn an_empty_ledger_stays_empty() {
    let (known, ahead) = partition_known(vec![], &known_set(&[KNOWN_A]));
    assert!(known.is_empty());
    assert!(ahead.is_empty());
}

#[test]
fn a_ledger_entirely_ahead_yields_an_empty_known_set() {
    // A revert across several releases: every row is unknown, and every
    // compiled-in migration is therefore pending (not an error).
    let (known, ahead) = partition_known(vec![row(AHEAD, 3)], &known_set(&[KNOWN_A]));
    assert!(known.is_empty());
    assert_eq!(ahead.len(), 1);
}

// ── The seam ─────────────────────────────────────────────────────────────────
//
// `Migration` has private fields, so `get_migration_models` is the ONLY point at
// which this crate can intercept the ledger read. If a `sea-orm-migration` bump
// ever inlines that read into `get_migration_with_status`, the override goes
// silently dead and reverts start failing again with no signal at all.
//
// These two tests pin the call path documented in the crate docs:
//
//   get_pending_migrations → get_migration_with_status → get_migration_models
//
// They run against `DatabaseConnection::Disconnected` on purpose. The probes
// answer both `install` and `get_migration_models` from memory, so nothing may
// touch the connection. If upstream stops routing through the override, the read
// falls through to the (severed) connection and the test fails with a connection
// error or a panic instead of the assertion below — loudly, either way.

mod seam {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use sea_orm::{ConnectionTrait, DbBackend, DbErr, ExecResult, QueryResult, Statement};
    use sea_orm_migration::prelude::*;
    use sea_orm_migration::seaql_migrations::Model;

    use super::{AHEAD, KNOWN_A, row};
    use crate::{known_versions, partition_known};

    /// How many times the override was consulted. If upstream stops calling it,
    /// this stays put and the assertions below say so by name.
    static LEDGER_READS: AtomicUsize = AtomicUsize::new(0);

    /// A connection that refuses every statement.
    ///
    /// Both probes stub the ledger read, so no SQL should ever reach the
    /// database — this is the value that proves it: if the seam starts issuing
    /// a statement of its own, the test fails here with this message rather
    /// than quietly talking to something. sea-orm 1.x had
    /// `DatabaseConnection::Disconnected` for this; 2.0 made
    /// `DatabaseConnection` a struct, and owning the type is better anyway,
    /// since it cannot be removed underneath us and it says what it is for.
    struct NeverConnected;

    const NO_SQL: &str = "the seam tests stub every ledger read; no statement should reach a \
                          database. If you are seeing this, the call path issues SQL the \
                          override no longer covers.";

    #[async_trait::async_trait]
    impl ConnectionTrait for NeverConnected {
        fn get_database_backend(&self) -> DbBackend {
            DbBackend::Postgres
        }

        async fn execute_raw(&self, _stmt: Statement) -> Result<ExecResult, DbErr> {
            Err(DbErr::Custom(NO_SQL.to_owned()))
        }

        async fn execute_unprepared(&self, _sql: &str) -> Result<ExecResult, DbErr> {
            Err(DbErr::Custom(NO_SQL.to_owned()))
        }

        async fn query_one_raw(&self, _stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
            Err(DbErr::Custom(NO_SQL.to_owned()))
        }

        async fn query_all_raw(&self, _stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
            Err(DbErr::Custom(NO_SQL.to_owned()))
        }
    }

    /// The one migration both probes are compiled with. `up` is never executed:
    /// these tests only ever ask for migration *status*.
    struct KnownMigration;

    impl MigrationName for KnownMigration {
        fn name(&self) -> &str {
            KNOWN_A
        }
    }

    #[sea_orm_migration::async_trait::async_trait]
    impl MigrationTrait for KnownMigration {
        async fn up(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
            unreachable!("the seam tests never apply a migration")
        }
    }

    /// A ledger as it looks right after a revert: one row this binary knows, one
    /// written by the release we are rolling back from.
    fn ledger_after_a_revert() -> Vec<Model> {
        vec![row(KNOWN_A, 1), row(AHEAD, 2)]
    }

    /// Reports the post-revert ledger **unfiltered** — i.e. what upstream's own
    /// default read would return. Stands in for the shipped behavior.
    struct UnfilteredProbe;

    #[sea_orm_migration::async_trait::async_trait]
    impl MigratorTrait for UnfilteredProbe {
        fn migrations() -> Vec<Box<dyn MigrationTrait>> {
            vec![Box::new(KnownMigration)]
        }

        async fn install<C>(_db: &C) -> Result<(), DbErr>
        where
            C: ConnectionTrait,
        {
            Ok(())
        }

        async fn get_migration_models<C>(_db: &C) -> Result<Vec<Model>, DbErr>
        where
            C: ConnectionTrait,
        {
            LEDGER_READS.fetch_add(1, Ordering::SeqCst);
            Ok(ledger_after_a_revert())
        }
    }

    /// Same ledger, run through this crate's **real** filter — only the SQL
    /// round-trip is stubbed out.
    struct TolerantProbe;

    #[sea_orm_migration::async_trait::async_trait]
    impl MigratorTrait for TolerantProbe {
        fn migrations() -> Vec<Box<dyn MigrationTrait>> {
            vec![Box::new(KnownMigration)]
        }

        async fn install<C>(_db: &C) -> Result<(), DbErr>
        where
            C: ConnectionTrait,
        {
            Ok(())
        }

        async fn get_migration_models<C>(_db: &C) -> Result<Vec<Model>, DbErr>
        where
            C: ConnectionTrait,
        {
            LEDGER_READS.fetch_add(1, Ordering::SeqCst);
            let (known, _ahead) =
                partition_known(ledger_after_a_revert(), &known_versions::<Self>());
            Ok(known)
        }
    }

    #[tokio::test]
    async fn get_migration_models_is_still_the_ledger_read() {
        let db = NeverConnected;
        let before = LEDGER_READS.load(Ordering::SeqCst);

        // `Migration` is not Debug, so no `expect_err`.
        let err = match UnfilteredProbe::get_pending_migrations(&db).await {
            Err(err) => err,
            Ok(_) => panic!(
                "an unknown ledger version must still abort — that IS the bug this crate \
                 tolerates. If sea-orm stopped aborting, `tolerate_schema_ahead!()` is dead \
                 weight and should be removed rather than left as decoration."
            ),
        };

        assert!(
            LEDGER_READS.load(Ordering::SeqCst) > before,
            "SEAM MOVED: sea-orm-migration no longer reads the ledger through \
             MigratorTrait::get_migration_models, so `tolerate_schema_ahead!()` is now a no-op \
             and reverts are broken again. Re-read the call path in the crate docs against the \
             new sea-orm-migration and move the override to whatever replaced it."
        );
        assert!(
            err.to_string().contains("is missing"),
            "SEAM MOVED: the abort no longer comes from the value get_migration_models returned \
             (got: {err}). The override can only prevent an abort it still feeds."
        );
    }

    #[tokio::test]
    async fn filtering_that_read_is_what_lets_a_revert_proceed() {
        let db = NeverConnected;

        // Same call path, same ledger, filter applied: no abort.
        let pending = TolerantProbe::get_pending_migrations(&db)
            .await
            .expect("an ahead row must not block a migration run");
        assert!(
            pending.is_empty(),
            "the pending set for KNOWN migrations must be unchanged — nothing is re-applied"
        );

        let applied = TolerantProbe::get_applied_migrations(&db)
            .await
            .expect("status read must succeed");
        assert_eq!(
            applied.iter().map(|m| m.name()).collect::<Vec<_>>(),
            vec![KNOWN_A],
            "the known migration must still read as applied"
        );
    }
}

/// `sea-orm-migration` 2.0 added a read-only family —
/// `get_migration_with_status_read_only` and the `get_pending_migrations_read_only`
/// / `get_applied_migrations_read_only` built on it — for checking migration
/// status from a connection without DDL rights.
///
/// **That family bypasses this crate.** It reads the ledger through a private
/// free function rather than `Self::get_migration_models`, so
/// `tolerate_schema_ahead!()` never sees it and an ahead row aborts exactly as
/// it did before. It cannot be overridden from outside `sea-orm-migration`
/// either: the override would have to return `Vec<Migration>`, and `Migration`
/// has private fields and no public constructor.
///
/// Nothing in this workspace uses it, and this test is what keeps that true.
/// A silent loss of tolerance on a path someone adds later is exactly the
/// failure this crate exists to prevent, so the discovery is a build failure
/// rather than a revert that aborts in production.
mod read_only_family {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The methods that skip the override, by name.
    const BYPASSES: &[&str] = &[
        "get_migration_with_status_read_only",
        "get_pending_migrations_read_only",
        "get_applied_migrations_read_only",
    ];

    pub(super) fn workspace_crates() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/migration-tolerance has a parent")
            .to_path_buf()
    }

    pub(super) fn rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                // `target` alone is worth minutes; the rest are never sources.
                if name == "target" || name == "node_modules" || name.starts_with('.') {
                    continue;
                }
                rust_files(&path, into);
            } else if path.extension().is_some_and(|e| e == "rs") {
                into.push(path);
            }
        }
    }

    #[test]
    fn no_migrator_reaches_for_the_read_only_family() {
        let root = workspace_crates();
        let own_source = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        rust_files(&root, &mut files);
        assert!(
            files.len() > 100,
            "the walk found only {} files under {} — it is not scanning crates/, so it \
             proves nothing",
            files.len(),
            root.display()
        );

        let mut found = Vec::new();
        for file in files {
            // This crate names them in its own docs and in this test.
            if file.starts_with(own_source) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            for method in BYPASSES {
                if text.contains(method) {
                    found.push(format!("{}: {method}", file.display()));
                }
            }
        }

        assert!(
            found.is_empty(),
            "these call sites use sea-orm-migration's read-only migration-status family, which \
             does NOT go through `tolerate_schema_ahead!()` — a ledger row from a newer release \
             will abort them, and reverts through this path are not safe:\n  {}\n\nIt cannot be \
             overridden from outside sea-orm-migration (`Migration` has no public constructor). \
             Either use the DDL-capable equivalent, which is tolerant, or upstream a fix and \
             update `oxy_migration_tolerance`'s docs.",
            found.join("\n  ")
        );
    }
}

/// Every migrator in the workspace must carry `tolerate_schema_ahead!()`.
///
/// The macro is opt-in per migrator, and `run_all_migrators` is a
/// `?`-propagating sequence: ONE untolerated ledger aborts the whole run with
/// the incident error, so seven of eight is worth exactly as much as zero. That
/// is not hypothetical — `OltpMigrator` shipped without it, and the integration
/// test that was supposed to notice had the same blind spot, because both were
/// written from the same hand-copied list of seven.
///
/// A list cannot guard a list. This reads the migrators out of the source.
mod every_migrator {
    use std::fs;

    use super::read_only_family::{rust_files, workspace_crates};

    /// What a migrator is, in source: the `impl` header. Matching the header
    /// rather than a type name means a migrator added under any crate, with any
    /// name, is judged.
    const IMPL_HEADER: &str = "impl MigratorTrait for";
    const MACRO: &str = "tolerate_schema_ahead!()";

    #[test]
    fn tolerance_is_wired_into_every_migrator() {
        let own_source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        rust_files(&workspace_crates(), &mut files);

        let mut migrators = 0usize;
        let mut untolerated = Vec::new();
        for file in files {
            // This crate's own probes implement MigratorTrait to STUB the read
            // the macro overrides; wiring it into them would test nothing.
            if file.starts_with(own_source) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            // Code lines only: a commented-out `// oxy_migration_tolerance::tolerate_schema_ahead!();`
            // must not pass for the real thing, nor a commented-out `impl` count
            // as a migrator.
            let code: Vec<&str> = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect();
            let count = code
                .iter()
                .filter(|line| line.contains(IMPL_HEADER))
                .count();
            if count == 0 {
                continue;
            }
            migrators += count;
            // Per file, not per impl: a file with two migrators and one macro
            // would pass, but no file in this workspace has two, and the
            // alternative is parsing Rust. If that changes, split on the header.
            if !code.iter().any(|line| line.contains(MACRO)) {
                untolerated.push(file.display().to_string());
            }
        }

        assert!(
            migrators >= 8,
            "found only {migrators} `{IMPL_HEADER}` sites — the scan is not finding the \
             migrators, so it proves nothing"
        );
        assert!(
            untolerated.is_empty(),
            "these migrators do not call `oxy_migration_tolerance::tolerate_schema_ahead!()`:\n  \
             {}\n\n`run_all_migrators` propagates with `?`, so ONE untolerated ledger aborts \
             the whole migration run and wedges the revert — the other seven buy nothing. Add \
             the macro after `migration_table_name()`, add the crate to the migrator's \
             Cargo.toml, and add its ledger table to LEDGERS in \
             `crates/app/tests/platform/migration_rollback_tolerance.rs`.",
            untolerated.join("\n  ")
        );
    }
}
