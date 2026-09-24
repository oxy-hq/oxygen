//! Make a Sea-ORM migrator tolerate ledger rows written by a **newer** binary.
//!
//! # Why this exists
//!
//! Sea-ORM treats the migration ledger as an *exact set equality* against the
//! binary's compiled-in migration list, not as a forward-only log. Its
//! `get_migration_with_status` computes `migration_in_db - migration_in_fs` and
//! aborts if that set is non-empty:
//!
//! ```text
//! Migration file of version '<version>' is missing,
//! this migration has been applied but its file is missing
//! ```
//!
//! That check is correct for forward rollouts and exactly backwards for
//! **reverts**. Rolling image N back to N-1 leaves N's bookkeeping row in the
//! table, so N-1 refuses to run — and because the migrate step is a blocking
//! pre-upgrade hook, the revert never happens at all. That is what took
//! 0.5.111 → 0.5.110 down: the schema was fine (the migration was
//! `ADD COLUMN IF NOT EXISTS` throughout); only the *ledger row* was the
//! problem. See `internal-docs/revert-safe-migrations.md`.
//!
//! # What this does
//!
//! It changes what the migrator **reads**, never what it **applies**:
//!
//! - Unknown ("ahead") versions are filtered out of the ledger read, so
//!   `missing_migrations_in_fs` is empty and `up()` proceeds.
//! - The rows stay **in the table**. Nothing is deleted or mutated. Rolling
//!   *forward* again therefore still sees those migrations as applied and does
//!   not re-run them — so no migration has to be idempotent for this to be
//!   sound.
//! - For **known** versions the behavior is bit-identical: same query, same
//!   ordering, same pending set.
//!
//! # What this does NOT do
//!
//! This fixes the rollout **mechanism**, not schema backward-compatibility. A
//! `DROP COLUMN`, a `NOT NULL` without a default, a tightened `CHECK`, or a data
//! backfill in release N still breaks N-1 — the difference is that N-1 now
//! *starts* and fails at query time instead of blocking the rollout. Only
//! expand/contract discipline prevents that class. Do not read this crate as
//! "we can always roll back."
//!
//! # Usage
//!
//! One line inside an existing `impl MigratorTrait` block:
//!
//! ```ignore
//! #[async_trait::async_trait]
//! impl MigratorTrait for MyMigrator {
//!     fn migrations() -> Vec<Box<dyn MigrationTrait>> { /* … */ }
//!
//!     fn migration_table_name() -> sea_orm::DynIden {
//!         Alias::new("seaql_migrations_mine").into_iden()
//!     }
//!
//!     oxy_migration_tolerance::tolerate_schema_ahead!();
//! }
//! ```
//!
//! The macro expands to the `async_trait`-desugared form of the method, so it
//! works whether or not the impl block carries `#[async_trait::async_trait]`
//! (all of Oxy's do). See [`tolerate_schema_ahead!`] for why it has to.
//!
//! # Maintenance — re-verify on every `sea-orm-migration` bump
//!
//! `get_migration_models` is the **only** seam on this path.
//! `sea_orm_migration::migrator::Migration` has private fields and no public
//! constructor, so overriding `get_migration_with_status` directly does not
//! compile. The call path this crate depends on is:
//!
//! ```text
//! Migrator::up
//!   └─ exec_up::<Migrator>              (private free fn)
//!        └─ M::get_pending_migrations   (provided trait method)
//!             └─ M::get_migration_with_status   ← the abort lives here
//!                  └─ M::get_migration_models   ← the only override point
//! ```
//!
//! If upstream ever inlines that read, this override goes **silently dead** and
//! reverts start failing again with no signal. `tests::seam` pins exactly that
//! and fails loudly if the seam moves. Verified against `sea-orm-migration`
//! **2.0.1** — the seam survived the 1.x → 2.0 bump unchanged, and the probe
//! that asserts the untolerated read still aborts confirms the upstream
//! behavior this crate exists for is still there.
//!
//! # What it does not cover: the read-only family
//!
//! 2.0 added `get_migration_with_status_read_only`, and
//! `get_pending_migrations_read_only` / `get_applied_migrations_read_only` on
//! top of it, for reading migration status from a connection with no DDL
//! rights. They reach the ledger through a **private free function**, not
//! `Self::get_migration_models`, so `tolerate_schema_ahead!()` never sees them
//! and an ahead row aborts them exactly as before.
//!
//! They cannot be overridden from out here: the override would have to return
//! `Vec<Migration>`, and `Migration` has private fields and no public
//! constructor. Nothing in this workspace calls them, and
//! `tests::read_only_family` fails the build if that changes — the point being
//! that losing tolerance is discovered at compile time rather than during a
//! revert.

use std::collections::HashSet;

use sea_orm::sea_query::{IntoIden, Order, Query};
use sea_orm::{FromQueryResult, Iterable};

// Re-exported so `tolerate_schema_ahead!` expands without the call site needing
// any import beyond this crate.
pub use sea_orm::{ConnectionTrait, DbErr};
pub use sea_orm_migration::MigratorTrait;
pub use sea_orm_migration::seaql_migrations;

/// Read the migration ledger, dropping versions this binary does not know about.
///
/// A drop-in replacement for `MigratorTrait::get_migration_models`. Prefer
/// wiring it in via [`tolerate_schema_ahead!`] rather than calling it directly.
///
/// Emits one `WARN` per ledger table naming every tolerated version. This must
/// never be silent: "the database is newer than this binary" is a real signal,
/// and downgrading it from a hard abort to a warning is the whole trade.
pub async fn applied_models_tolerating_ahead<M, C>(
    db: &C,
) -> Result<Vec<seaql_migrations::Model>, DbErr>
where
    M: MigratorTrait + ?Sized,
    C: ConnectionTrait,
{
    M::install(db).await?;

    let table = M::migration_table_name();
    let table_display = table.to_string();
    // Same query the upstream default builds. It uses a private `QueryTable`
    // extension trait for the FROM clause; `.from(..)` is that trait's entire
    // body for `SelectStatement`, so this is the same statement via public API
    // — and still built through sea-query, so the table name is quoted per
    // backend rather than interpolated into a string.
    let statement = {
        let select = Query::select()
            .from(table)
            .columns(seaql_migrations::Column::iter().map(IntoIden::into_iden))
            .order_by(seaql_migrations::Column::Version, Order::Asc)
            .to_owned();
        db.get_database_backend().build(&select)
    };
    let rows = seaql_migrations::Model::find_by_statement(statement)
        .all(db)
        .await?;

    let (known_rows, ahead) = partition_known(rows, &known_versions::<M>());
    if !ahead.is_empty() {
        warn_schema_ahead(&table_display, &ahead);
    }
    Ok(known_rows)
}

/// The set of migration names compiled into this binary.
fn known_versions<M>() -> HashSet<String>
where
    M: MigratorTrait + ?Sized,
{
    M::migrations()
        .iter()
        .map(|m| m.name().to_owned())
        .collect()
}

/// Split ledger rows into (known to this binary, ahead of this binary).
///
/// Order is preserved within each bucket, so the `known` half is still sorted by
/// version exactly as the query returned it.
fn partition_known(
    rows: Vec<seaql_migrations::Model>,
    known: &HashSet<String>,
) -> (Vec<seaql_migrations::Model>, Vec<seaql_migrations::Model>) {
    rows.into_iter()
        .partition(|row| known.contains(&row.version))
}

fn warn_schema_ahead(table: &str, ahead: &[seaql_migrations::Model]) {
    let versions = ahead
        .iter()
        .map(|row| row.version.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    tracing::warn!(
        migration_table = %table,
        ahead_count = ahead.len(),
        ahead_versions = %versions,
        "migrations: database schema is AHEAD of this binary — tolerating ledger rows from a \
         newer release so a rollback can proceed. The rows are left in place, so rolling \
         forward again will not re-run them. The schema is assumed ADDITIVE: verify no \
         destructive migration (DROP/RENAME column, NOT NULL without default, tightened \
         CHECK, data backfill) is involved, because this binary will now start and then \
         fail at query time if one is."
    );
}

/// Wire the tolerant ledger read into a `MigratorTrait` impl. One line per
/// migrator, adjacent to `migration_table_name()`.
///
/// See the crate docs for what this does and does not buy you.
///
/// # Why this expands to a hand-desugared `async fn`
///
/// `MigratorTrait` is declared under `#[async_trait::async_trait]`, so the real
/// signature of `get_migration_models` is a `-> Pin<Box<dyn Future + Send>>`,
/// not an `async fn`. Attribute macros run *before* function-like macros, so
/// the `#[async_trait::async_trait]` on the impl block cannot see through this
/// invocation and would leave a plain `async fn` behind — which then fails to
/// match the trait (`E0195`, lifetime bounds do not match). Writing the
/// desugared form here is what makes a one-liner call site possible at all.
///
/// This mirrors `async-trait`'s own output. If that output ever changes shape,
/// this breaks at **compile** time in all eight migrators — noisy, not silent,
/// which is the only reason it is an acceptable thing to hand-write.
#[macro_export]
macro_rules! tolerate_schema_ahead {
    () => {
        /// Overridden so a ledger row from a NEWER release does not abort this
        /// binary's migration run — see `oxy_migration_tolerance` for why, and
        /// for what it does not cover.
        fn get_migration_models<'life0, 'async_trait, C>(
            db: &'life0 C,
        ) -> ::core::pin::Pin<
            ::std::boxed::Box<
                dyn ::core::future::Future<
                        Output = ::core::result::Result<
                            ::std::vec::Vec<$crate::seaql_migrations::Model>,
                            $crate::DbErr,
                        >,
                    > + ::core::marker::Send
                    + 'async_trait,
            >,
        >
        where
            C: $crate::ConnectionTrait + 'async_trait,
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            ::std::boxed::Box::pin($crate::applied_models_tolerating_ahead::<Self, C>(db))
        }
    };
}

#[cfg(test)]
mod tests;
