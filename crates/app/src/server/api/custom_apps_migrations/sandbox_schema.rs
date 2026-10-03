//! A sandbox's own OLTP schema on the org's staging branch: created and
//! seeded once, then migrated by each of the sandbox's builds
//! (`internal-docs/per-org-oltp-postgres.md` → Sandbox schemas on the staging
//! branch).
//!
//! Staging's apply ([`super::branch`]) runs a bundle's files in the app's own
//! schema on the branch, under `branch:<provider id>`. A sandbox's runs the
//! **same files** in `app_<writer>__<label>` on the same branch, as the same
//! writer with that schema as its only `search_path` entry, under a ledger
//! target of its own — `schema:<sandbox schema>` in store `oltp` — so a file
//! applied to one sandbox is never read as applied to staging, to production
//! or to another sandbox.
//!
//! **The ledger starts as a copy of staging's.** The schema is seeded as a
//! copy of staging's (`oxy_oltp::sandbox_schema::seed`), which already holds
//! every table staging's applied files made, so the sandbox's ledger is set
//! to those rows when it is seeded: a publish then runs only the files
//! staging has not applied. Both are read under the app's apply lock on the
//! branch — the lock a staging apply holds from before its first file until
//! its last is recorded — so the rows describe the structure that was copied.
//!
//! **A file names no other schema.** Files are written with unqualified
//! names, which is what lets one file serve every environment. A pending file
//! that spells staging's schema, or another sandbox's, is refused before
//! anything runs: the writer holds grants there, and Postgres would run it.
//!
//! Called only by the queued task (`custom_apps_sandboxes::oltp_task`), under
//! the sandbox's lock.

use std::time::Duration;

use entity::custom_app_migrations as ledger;
use oxy_oltp::OltpBranch;
use oxy_oltp::branches::BranchCut;
use oxy_oltp::resolver::BranchWriter;
use oxy_oltp::sandbox_schema::seed::{SeedCaps, SeedReport, seed};
use oxy_oltp::sandbox_schema::{
    Dropped, SandboxSchema, SandboxSchemaError, create_on_branch, drop_on_branch,
    resolve_branch_sandbox_writer_for_org,
};
use oxy_oltp::schema::WriterRef;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, TransactionTrait,
};
use tracing::{info, instrument};
use uuid::Uuid;

use super::apply::{Destination, apply_to, open_locked, read_ledger};
use super::plan::plan;
use super::types::{Applied, DeclaredMigration, MigrationError, MigrationTarget, STORE_OLTP};
use crate::server::api::custom_apps_functions::env_policy::{SandboxFence, schema_named_in};

/// How long one statement of a seed or a sandbox apply waits for a lock.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one statement may run — a table's copy included.
const STATEMENT_TIMEOUT: Duration = Duration::from_secs(120);

/// Whose sandbox schema an operation is for.
#[derive(Clone, Copy, Debug)]
pub struct SandboxOltp<'a> {
    pub app_id: Uuid,
    pub org_id: Uuid,
    pub writer: &'a WriterRef,
    pub schema: &'a SandboxSchema,
}

impl SandboxOltp<'_> {
    /// The ledger target the sandbox's files are recorded under.
    pub fn target(&self) -> MigrationTarget {
        MigrationTarget::Schema(self.schema.name().to_string())
    }

    fn destination<'a>(&self, writer: &'a BranchWriter, setup: &'a str) -> Destination<'a> {
        Destination {
            conn: &writer.connection,
            target: self.target(),
            session_setup: Some(setup),
            cut: Some(&writer.cut),
        }
    }

    /// The app's writer on the branch, in the sandbox's schema. `Ok(None)`:
    /// the org has no staging branch.
    async fn writer_on_branch(
        &self,
        db: &DatabaseConnection,
    ) -> Result<Option<BranchWriter>, MigrationError> {
        resolve_branch_sandbox_writer_for_org(
            db,
            self.org_id,
            OltpBranch::Staging,
            self.writer,
            self.schema,
        )
        .await
        .map_err(unusable)
    }
}

fn session_setup() -> String {
    format!(
        "SET lock_timeout = '{}ms'; SET statement_timeout = '{}ms'",
        LOCK_TIMEOUT.as_millis(),
        STATEMENT_TIMEOUT.as_millis()
    )
}

fn unusable(e: SandboxSchemaError) -> MigrationError {
    MigrationError::Infra {
        filename: String::new(),
        message: format!("the sandbox's schema on the staging branch is not usable: {e}"),
    }
}

/// Create the sandbox's schema on the branch — empty, replacing whatever was
/// under the name — and seed it from staging's, with staging's ledger.
/// Answers the cut it was seeded on and what was copied; `Ok(None)` when the
/// org has no staging branch. [`MigrationError::Busy`] while a staging apply
/// of the app holds its lock on the branch.
#[instrument(skip(db, run), fields(app_id = %run.app_id, schema = %run.schema))]
pub async fn create_and_seed(
    db: &DatabaseConnection,
    run: SandboxOltp<'_>,
    caps: &SeedCaps,
) -> Result<Option<(BranchCut, SeedReport)>, MigrationError> {
    let created = create_on_branch(db, run.org_id, OltpBranch::Staging, run.writer, run.schema)
        .await
        .map_err(unusable)?;
    let Some(created_on) = created else {
        return Ok(None);
    };
    let Some(writer) = run.writer_on_branch(db).await? else {
        return Ok(None);
    };
    if writer.cut != created_on {
        return Err(reset_during("it was being created"));
    }
    let setup = session_setup();
    let (mut client, lock_key) = open_locked(run.app_id, &run.destination(&writer, &setup)).await?;
    // Under the app's apply lock: no staging apply is between a file's DDL
    // and its ledger row, so these rows are what the copy below holds.
    let staging = MigrationTarget::Branch(writer.cut.provider_branch_id.clone());
    let snapshot = ledger_rows(db, run.app_id, &staging).await;
    let seeded = match &snapshot {
        Ok(_) => seed(&mut client, run.schema, caps)
            .await
            .map_err(|e| MigrationError::Infra {
                filename: String::new(),
                message: e.to_string(),
            }),
        Err(e) => Err(MigrationError::Db(e.to_string())),
    };
    let _ = client
        .execute("SELECT pg_advisory_unlock($1)", &[&lock_key])
        .await;
    let report = seeded?;
    let snapshot = snapshot.map_err(|e| MigrationError::Db(e.to_string()))?;
    copy_ledger(db, &run, &writer.cut, &snapshot).await?;
    info!(
        tables = report.tables.len(),
        rows = report.rows_copied,
        structure_only = report.structure_only.len(),
        "seeded a sandbox's OLTP schema from staging's"
    );
    Ok(Some((writer.cut, report)))
}

fn reset_during(when: &str) -> MigrationError {
    MigrationError::Infra {
        filename: String::new(),
        message: format!(
            "the staging branch was reset or re-cut while {when}; the next publish to the \
             sandbox starts again on the new copy"
        ),
    }
}

async fn ledger_rows<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    target: &MigrationTarget,
) -> Result<Vec<ledger::Model>, sea_orm::DbErr> {
    ledger::Entity::find()
        .filter(ledger::Column::AppId.eq(app_id))
        .filter(ledger::Column::Store.eq(STORE_OLTP))
        .filter(ledger::Column::Target.eq(target.as_key()))
        .all(db)
        .await
}

/// Make the sandbox's ledger exactly `snapshot` (staging's rows), re-targeted
/// — in one control-plane transaction, and only while the branch is still
/// `cut`: a reset since discarded the copy these rows describe.
async fn copy_ledger(
    db: &DatabaseConnection,
    run: &SandboxOltp<'_>,
    cut: &BranchCut,
    snapshot: &[ledger::Model],
) -> Result<(), MigrationError> {
    let db_err = |e: sea_orm::DbErr| MigrationError::Db(e.to_string());
    let target = run.target().as_key();
    let txn = db.begin().await.map_err(db_err)?;
    if !oxy_oltp::branches::still_current(&txn, cut)
        .await
        .map_err(db_err)?
    {
        let _ = txn.rollback().await;
        return Err(reset_during("the sandbox's schema was being seeded"));
    }
    clear_ledger_on(&txn, run.app_id, run.schema)
        .await
        .map_err(db_err)?;
    let copies = snapshot.iter().map(|row| ledger::ActiveModel {
        app_id: ActiveValue::Set(row.app_id),
        store: ActiveValue::Set(row.store.clone()),
        target: ActiveValue::Set(target.clone()),
        filename: ActiveValue::Set(row.filename.clone()),
        checksum: ActiveValue::Set(row.checksum.clone()),
        applied_at: ActiveValue::Set(row.applied_at),
        applied_by_build: ActiveValue::Set(row.applied_by_build),
    });
    if !snapshot.is_empty() {
        ledger::Entity::insert_many(copies)
            .exec(&txn)
            .await
            .map_err(db_err)?;
    }
    txn.commit().await.map_err(db_err)
}

async fn clear_ledger_on<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    schema: &SandboxSchema,
) -> Result<u64, sea_orm::DbErr> {
    let target = MigrationTarget::Schema(schema.name().to_string());
    let deleted = ledger::Entity::delete_many()
        .filter(ledger::Column::AppId.eq(app_id))
        .filter(ledger::Column::Store.eq(STORE_OLTP))
        .filter(ledger::Column::Target.eq(target.as_key()))
        .exec(db)
        .await?;
    Ok(deleted.rows_affected)
}

/// Apply `declared` to the sandbox's schema and record each file under the
/// sandbox's target. `Ok(None)`: the org has no staging branch.
#[instrument(skip(db, run, declared), fields(app_id = %run.app_id, schema = %run.schema))]
pub async fn apply(
    db: &DatabaseConnection,
    run: SandboxOltp<'_>,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
) -> Result<Option<Applied>, MigrationError> {
    if declared.is_empty() {
        return Ok(Some(Applied::default()));
    }
    let ledger = read_ledger(db, run.app_id, STORE_OLTP, &run.target()).await?;
    let pending = plan(declared, &ledger)?;
    if pending.is_empty() {
        return Ok(Some(Applied {
            already_applied: declared.len(),
            ..Applied::default()
        }));
    }
    refuse_other_schemas(run.schema, &pending)?;
    let Some(writer) = run.writer_on_branch(db).await? else {
        return Ok(None);
    };
    let setup = session_setup();
    let dest = run.destination(&writer, &setup);
    apply_to(db, run.app_id, build_pk, declared, &dest)
        .await
        .map(Some)
}

/// Refuse a pending file that spells a schema of the app other than the
/// sandbox's own — staging's, or another sandbox's.
fn refuse_other_schemas(
    schema: &SandboxSchema,
    pending: &[&DeclaredMigration],
) -> Result<(), MigrationError> {
    let fence = SandboxFence::new(schema.app_schema(), schema.name());
    for m in pending {
        let message = match schema_named_in(&fence, &m.sql) {
            Ok(None) => continue,
            Ok(Some(named)) => format!(
                "it names schema {named}, and a sandbox applies a file in its own schema \
                 ({schema}); write table names unqualified, so one file serves every environment"
            ),
            Err(unreadable) => format!("it {}", unreadable.why),
        };
        return Err(MigrationError::Failed {
            filename: m.filename.clone(),
            message,
        });
    }
    Ok(())
}

/// What a [`drop_schema`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SandboxOltpDrop {
    /// `false` when nothing connected: no row or ledger recorded a schema.
    pub attempted: bool,
    /// The org has no staging branch any more; the schema went with it.
    pub branch_gone: bool,
    pub ledger_rows_cleared: u64,
}

/// Drop the sandbox's schema on the branch, confirm it is gone, and clear its
/// ledger rows — the OLTP step of a sandbox's teardown.
///
/// **Nothing connects unless something records a schema**: `recorded` (the
/// sandbox's row holds a state) or a ledger row under its target. **With
/// either, the drop is confirmed or it fails** — a branch mid-reset, or one
/// the worker cannot reach, is an error, and the caller keeps the row.
#[instrument(skip(db), fields(%app_id, %schema))]
pub async fn drop_schema(
    db: &DatabaseConnection,
    app_id: Uuid,
    org_id: Uuid,
    schema: &SandboxSchema,
    recorded: bool,
) -> Result<SandboxOltpDrop, MigrationError> {
    let db_err = |e: sea_orm::DbErr| MigrationError::Db(e.to_string());
    let target = MigrationTarget::Schema(schema.name().to_string());
    let applied = ledger::Entity::find()
        .filter(ledger::Column::AppId.eq(app_id))
        .filter(ledger::Column::Store.eq(STORE_OLTP))
        .filter(ledger::Column::Target.eq(target.as_key()))
        .count(db)
        .await
        .map_err(db_err)?;
    if !recorded && applied == 0 {
        return Ok(SandboxOltpDrop::default());
    }
    let dropped = drop_on_branch(db, org_id, OltpBranch::Staging, schema)
        .await
        .map_err(|e| MigrationError::Infra {
            filename: String::new(),
            message: format!("the sandbox's OLTP schema {schema} was not dropped: {e}"),
        })?;
    let cleared = clear_ledger_on(db, app_id, schema).await.map_err(db_err)?;
    info!(?dropped, cleared, "dropped a sandbox's OLTP schema");
    Ok(SandboxOltpDrop {
        attempted: true,
        branch_gone: dropped == Dropped::NoBranch,
        ledger_rows_cleared: cleared,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> SandboxSchema {
        SandboxSchema::for_writer(&WriterRef::app("store").expect("a writer"), "dev_a1")
            .expect("a name")
    }

    fn file(name: &str, sql: &str) -> DeclaredMigration {
        DeclaredMigration {
            filename: name.to_string(),
            checksum: "0".repeat(64),
            sql: sql.to_string(),
        }
    }

    /// The same key shape as the Airhouse sibling's, in store `oltp`: never
    /// staging's `branch:<id>`, never `production`.
    #[test]
    fn the_sandboxs_ledger_target_is_its_schema() {
        let writer = WriterRef::app("store").expect("a writer");
        let schema = schema();
        let run = SandboxOltp {
            app_id: Uuid::nil(),
            org_id: Uuid::nil(),
            writer: &writer,
            schema: &schema,
        };
        assert_eq!(run.target().as_key(), "schema:app_store__dev_a1");
    }

    /// Unqualified files — and a function definition, which a `ctx.oltp`
    /// statement may not carry but a migration may — are applied; one naming
    /// staging's schema or another sandbox's is the author's to fix.
    #[test]
    fn a_pending_file_naming_another_schema_of_the_app_is_refused() {
        let ok = [
            file("1.sql", "create table notes (id int primary key)"),
            file(
                "2.sql",
                "create function bump() returns int language sql as $$ select 1 $$",
            ),
            file(
                "3.sql",
                "alter table app_store__dev_a1.notes add column x int",
            ),
        ];
        let pending: Vec<&DeclaredMigration> = ok.iter().collect();
        assert!(refuse_other_schemas(&schema(), &pending).is_ok());

        for sql in [
            "alter table app_store.notes add column x int",
            "insert into app_store__dev_b2.notes values (1)",
            "select setval('app_store.notes_id_seq', 1)",
        ] {
            let bad = [file("9.sql", sql)];
            let pending: Vec<&DeclaredMigration> = bad.iter().collect();
            let err = refuse_other_schemas(&schema(), &pending).unwrap_err();
            assert!(err.is_author_fault(), "{sql}: {err}");
            assert!(err.to_string().contains("9.sql"), "{sql}: {err}");
            assert!(
                err.to_string().contains("write table names unqualified"),
                "{sql}: {err}"
            );
        }
    }
}
