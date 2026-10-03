//! The plan against a real writer.

use std::collections::HashMap;

use chrono::Utc;
use entity::custom_app_migrations;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    EntityTrait, QueryFilter, TransactionTrait,
};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use super::plan::plan;
use super::types::{Applied, DeclaredMigration, MigrationError, MigrationTarget, STORE_OLTP};

/// The one target this path applies to. It connects to the app's production
/// OLTP writer, so recording any other target here would be a lie.
const TARGET: MigrationTarget = MigrationTarget::Production;

/// `filename -> checksum` for one app's files in one store and target.
///
/// The target is what [`plan`] is planning *for*: a file applied to a
/// non-production target is not applied to production, and reading it as if it
/// were is how a promote would skip production's DDL.
pub async fn read_ledger(
    db: &DatabaseConnection,
    app_id: Uuid,
    store: &str,
    target: &MigrationTarget,
) -> Result<HashMap<String, String>, MigrationError> {
    Ok(custom_app_migrations::Entity::find()
        .filter(custom_app_migrations::Column::AppId.eq(app_id))
        .filter(custom_app_migrations::Column::Store.eq(store))
        .filter(custom_app_migrations::Column::Target.eq(target.as_key()))
        .all(db)
        .await
        .map_err(|e| MigrationError::Db(e.to_string()))?
        .into_iter()
        .map(|r| (r.filename, r.checksum))
        .collect())
}

/// A stable 64-bit advisory-lock key for one app.
///
/// Per-app rather than per-tenant: two apps in the same org have disjoint
/// schemas and disjoint ledgers, so serialising them against each other would
/// only make concurrent promotes slower.
pub(super) fn app_lock_key(app_id: Uuid) -> i64 {
    let b = app_id.as_bytes();
    i64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Render a Postgres failure the way the author needs it — SQLSTATE, message,
/// detail, hint. `tokio_postgres::Error`'s own Display is just "db error".
pub(super) fn pg_detail(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => {
            let mut msg = format!("[{}] {}", db.code().code(), db.message());
            if let Some(detail) = db.detail() {
                msg.push_str(&format!(" — {detail}"));
            }
            if let Some(hint) = db.hint() {
                msg.push_str(&format!(" (hint: {hint})"));
            }
            msg
        }
        None => e.to_string(),
    }
}

/// Apply this bundle's declared migrations to the app's own OLTP schema.
///
/// Called from `publish` **before** the published pointer moves, so a failure
/// leaves the app serving its previous build. A half-migrated app whose code
/// already shipped is worse than a promote that did not happen.
///
/// Returns `Applied::default()` for the common case: an app that declared
/// nothing (`declared` empty). That path costs one branch — no ledger read, no
/// writer resolution, no tenant connection — so an app with no tables pays
/// nothing for this feature existing.
#[instrument(skip(db, declared), fields(app_id = %app_id, app_slug = %app_slug))]
pub(crate) async fn apply_on_promote(
    db: &DatabaseConnection,
    app_id: Uuid,
    app_slug: &str,
    org_id: Uuid,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
) -> Result<Applied, MigrationError> {
    if declared.is_empty() {
        return Ok(Applied::default());
    }

    // Cheap pre-flight against the control plane. Two reasons it runs before any
    // tenant work: a promote that has nothing to apply must not open a tenant
    // connection at all, and an EDITED migration must fail the promote without
    // having touched the tenant database. The authoritative plan is recomputed
    // under the lock (`apply_to`).
    if let Some(done) = nothing_pending(db, app_id, declared, &TARGET).await? {
        return Ok(done);
    }

    let writer = app_writer(app_slug)?;
    // NOT `NoSchema`: an unprovisioned or disabled OLTP store is the OPERATOR's
    // state, and nothing the author can change in the bundle fixes it. Telling
    // CI "your change is wrong" about a store nobody has provisioned sends the
    // publisher to edit SQL that is already correct.
    let conn = oxy_oltp::resolver::resolve_writer_connection_for_org(db, org_id, &writer)
        .await
        .map_err(|e| MigrationError::Infra {
            filename: String::new(),
            message: format!("the app's OLTP store is not reachable: {e}"),
        })?;
    let dest = Destination {
        conn: &conn,
        target: TARGET,
        session_setup: None,
        cut: None,
    };
    apply_to(db, app_id, build_pk, declared, &dest).await
}

/// `Some` when the ledger of `target` already holds every declared file — the
/// answer, without a tenant connection. `Err` for an edited or renamed file.
pub(super) async fn nothing_pending(
    db: &DatabaseConnection,
    app_id: Uuid,
    declared: &[DeclaredMigration],
    target: &MigrationTarget,
) -> Result<Option<Applied>, MigrationError> {
    let pending = plan(
        declared,
        &read_ledger(db, app_id, STORE_OLTP, target).await?,
    )?;
    Ok(pending.is_empty().then(|| Applied {
        already_applied: declared.len(),
        ..Applied::default()
    }))
}

/// The app's OLTP writer, DERIVED from the slug exactly as `ctx.oltp` derives
/// it (`custom_apps_functions/host.rs`). The manifest gets no say: it may
/// declare *that* there are migrations, never *where* they land.
pub(super) fn app_writer(app_slug: &str) -> Result<oxy_oltp::schema::WriterRef, MigrationError> {
    let writer_name = oxy_oltp::schema::app_writer_name(app_slug).ok_or_else(|| {
        MigrationError::NoSchema(format!(
            "the app's slug '{app_slug}' cannot back an OLTP schema (a slug must start with a \
             letter, be at most {max} characters, and use only lowercase letters, digits and \
             hyphens — a `_` is refused because it would collide with the hyphenated form)",
            max = oxy_oltp::schema::MAX_NAME_LEN,
        ))
    })?;
    oxy_oltp::schema::WriterRef::app(&writer_name)
        .map_err(|e| MigrationError::NoSchema(e.to_string()))
}

/// Where an apply runs, and where it records.
pub(super) struct Destination<'a> {
    /// A writer on the database `target` names.
    pub conn: &'a oxy_oltp::resolver::WriterConnection,
    pub target: MigrationTarget,
    /// Run on the session before anything else: a branch's lock and statement
    /// timeouts. `None` for production, which runs exactly as before.
    pub session_setup: Option<&'a str>,
    /// For a branch, the cut `conn` reaches: each file is recorded only while
    /// the branch is still that cut (`oxy_oltp::branches::still_current`).
    pub cut: Option<&'a oxy_oltp::branches::BranchCut>,
}

/// Apply `declared` through `dest.conn` and record each file under
/// `dest.target`, and only there.
pub(super) async fn apply_to(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
    dest: &Destination<'_>,
) -> Result<Applied, MigrationError> {
    let (mut client, lock_key) = open_locked(app_id, dest).await?;
    let record = Recorder {
        db,
        app_id,
        build_pk,
        target: &dest.target,
        cut: dest.cut,
    };
    let outcome = run_pending(&mut client, declared, &record, &dest.conn.schema).await;
    // Best-effort: the lock is session-scoped and the client is dropped on
    // return anyway.
    let _ = client
        .execute("SELECT pg_advisory_unlock($1)", &[&lock_key])
        .await;
    outcome
}

/// A session on `dest`, set up, holding the app's apply lock.
///
/// `search_path` is already pinned to the writer's schema by the resolver, so
/// an unqualified `CREATE TABLE orders` lands in `app_<writer>` and a
/// reference to anything outside it fails on grants rather than on trust.
pub(super) async fn open_locked(
    app_id: Uuid,
    dest: &Destination<'_>,
) -> Result<(tokio_postgres::Client, i64), MigrationError> {
    let client = oxy_oltp::connect::connect(&dest.conn.dsn, "custom app migration")
        .await
        .map_err(|e| MigrationError::Connect(e.to_string()))?;
    if let Some(setup) = dest.session_setup {
        client
            .batch_execute(setup)
            .await
            .map_err(|e| MigrationError::Connect(format!("set up the session: {e}")))?;
    }
    // Serialise promotes of the SAME app before re-reading the ledger. Without
    // this, two concurrent promotes both read "0002 unapplied" and both run its
    // DDL; the ledger's unique key would then reject only the second INSERT —
    // after the SQL had already run twice. `try`, not a blocking acquire: a
    // publish that hangs with no output is indistinguishable from a slow
    // migration, and one wedged session would block every later promote.
    let lock_key = app_lock_key(app_id);
    let got: bool = client
        .query_one("SELECT pg_try_advisory_lock($1)", &[&lock_key])
        .await
        .map_err(|e| MigrationError::Connect(format!("acquire apply lock: {e}")))?
        .get(0);
    if !got {
        return Err(MigrationError::Busy);
    }
    Ok((client, lock_key))
}

/// Plan against the ledger re-read under the lock — the pre-flight is an
/// optimisation and a fast refusal; THIS is the plan that runs — then run and
/// record each pending file in order.
async fn run_pending(
    client: &mut tokio_postgres::Client,
    declared: &[DeclaredMigration],
    record: &Recorder<'_>,
    schema: &str,
) -> Result<Applied, MigrationError> {
    let ledger = read_ledger(record.db, record.app_id, STORE_OLTP, record.target).await?;
    let pending = plan(declared, &ledger)?;
    let mut outcome = Applied {
        already_applied: declared.len() - pending.len(),
        ..Applied::default()
    };
    info!(
        schema = %schema,
        target = %record.target.as_key(),
        pending = pending.len(),
        "applying custom-app schema migrations"
    );
    for m in pending {
        run_one(client, m).await?;
        record.applied(m).await?;
        info!(filename = %m.filename, schema = %schema, "applied custom-app migration");
        outcome.applied.push(m.filename.clone());
    }
    Ok(outcome)
}

/// Run one file in its own transaction.
async fn run_one(
    client: &mut tokio_postgres::Client,
    m: &DeclaredMigration,
) -> Result<(), MigrationError> {
    let txn = client
        .transaction()
        .await
        .map_err(|e| MigrationError::Infra {
            filename: m.filename.clone(),
            message: pg_detail(&e),
        })?;
    // `batch_execute` (simple query protocol) so a file may hold several
    // statements. All of them commit together or none do, which is what
    // makes a failed migration leave nothing behind for the next promote to
    // trip over.
    txn.batch_execute(&m.sql)
        .await
        .map_err(|e| MigrationError::Failed {
            filename: m.filename.clone(),
            message: pg_detail(&e),
        })?;
    txn.commit().await.map_err(|e| MigrationError::Infra {
        filename: m.filename.clone(),
        message: format!("committing: {e}"),
    })
}

/// Writes a file's ledger row under the one target the apply connected to.
struct Recorder<'a> {
    db: &'a DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
    target: &'a MigrationTarget,
    cut: Option<&'a oxy_oltp::branches::BranchCut>,
}

impl Recorder<'_> {
    /// The ledger lives in the CONTROL database and the DDL just committed in
    /// the TENANT database, so these two cannot be one transaction. The
    /// ordering is chosen for which failure is louder: recording second means
    /// a crash in this window re-attempts the file on the next promote, where
    /// non-idempotent DDL fails with Postgres's own `already exists`. The
    /// other order would record a file that never ran and silently skip it
    /// forever. Loud and recoverable beats silent and wrong.
    async fn applied(&self, m: &DeclaredMigration) -> Result<(), MigrationError> {
        match self.cut {
            None => self.insert(self.db, m).await,
            Some(cut) => self.applied_on_branch(cut, m).await,
        }
    }

    /// On a branch, the record and the check that the branch is still the
    /// cut the file ran on share one transaction, the branch row read
    /// `FOR SHARE`. A reset or re-cut since the apply connected discarded the
    /// file with the old copy: recording it would make staging skip it on the
    /// new one forever, so it is not recorded and the apply stops.
    async fn applied_on_branch(
        &self,
        cut: &oxy_oltp::branches::BranchCut,
        m: &DeclaredMigration,
    ) -> Result<(), MigrationError> {
        let not_recorded = |e: sea_orm::DbErr| ledger_write_failed(m, e);
        let txn = self.db.begin().await.map_err(not_recorded)?;
        if !oxy_oltp::branches::still_current(&txn, cut)
            .await
            .map_err(not_recorded)?
        {
            let _ = txn.rollback().await;
            warn!(filename = %m.filename, "staging branch reset during the apply; not recorded");
            return Err(MigrationError::Infra {
                filename: m.filename.clone(),
                message: "the staging branch was reset or re-cut while this file ran, so it \
                          is not recorded; the next publish runs it on the new copy"
                    .to_string(),
            });
        }
        self.insert(&txn, m).await?;
        txn.commit().await.map_err(not_recorded)
    }

    async fn insert<C: ConnectionTrait>(
        &self,
        conn: &C,
        m: &DeclaredMigration,
    ) -> Result<(), MigrationError> {
        custom_app_migrations::ActiveModel {
            app_id: Set(self.app_id),
            store: Set(STORE_OLTP.to_string()),
            target: Set(self.target.as_key()),
            filename: Set(m.filename.clone()),
            checksum: Set(m.checksum.clone()),
            applied_at: Set(Utc::now().fixed_offset()),
            applied_by_build: Set(Some(self.build_pk)),
        }
        .insert(conn)
        .await
        .map(|_| ())
        .map_err(|e| ledger_write_failed(m, e))
    }
}

fn ledger_write_failed(m: &DeclaredMigration, e: sea_orm::DbErr) -> MigrationError {
    warn!(filename = %m.filename, error = %e, "migration applied but not recorded");
    MigrationError::LedgerWriteFailed {
        filename: m.filename.clone(),
        message: e.to_string(),
    }
}
