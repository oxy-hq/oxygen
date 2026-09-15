//! The plan against a real writer.

use std::collections::HashMap;

use chrono::Utc;
use entity::custom_app_migrations;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use super::plan::plan;
use super::types::{Applied, DeclaredMigration, MigrationError, STORE_OLTP};

/// `filename -> checksum` for one app's files in one store.
pub(super) async fn read_ledger(
    db: &DatabaseConnection,
    app_id: Uuid,
    store: &str,
) -> Result<HashMap<String, String>, MigrationError> {
    Ok(custom_app_migrations::Entity::find()
        .filter(custom_app_migrations::Column::AppId.eq(app_id))
        .filter(custom_app_migrations::Column::Store.eq(store))
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
    // below under the lock.
    if plan(declared, &read_ledger(db, app_id, STORE_OLTP).await?)?.is_empty() {
        return Ok(Applied {
            applied: Vec::new(),
            already_applied: declared.len(),
        });
    }

    // The writer is DERIVED from the slug, exactly as `ctx.oltp` derives it
    // (`custom_apps_functions/host.rs`). The manifest gets no say: it may
    // declare *that* there are migrations, never *where* they land.
    let writer_name = oxy_oltp::schema::app_writer_name(app_slug).ok_or_else(|| {
        MigrationError::NoSchema(format!(
            "the app's slug '{app_slug}' cannot back an OLTP schema (a slug must start with a \
             letter, be at most {max} characters, and use only lowercase letters, digits and \
             hyphens — a `_` is refused because it would collide with the hyphenated form)",
            max = oxy_oltp::schema::MAX_NAME_LEN,
        ))
    })?;
    let writer = oxy_oltp::schema::WriterRef::app(&writer_name)
        .map_err(|e| MigrationError::NoSchema(e.to_string()))?;
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

    // `search_path` is already pinned to the writer's schema by the resolver, so
    // an unqualified `CREATE TABLE orders` lands in `app_<writer>` and a
    // reference to anything outside it fails on grants rather than on trust.
    let mut client = oxy_oltp::connect::connect(&conn.dsn, "custom app migration")
        .await
        .map_err(|e| MigrationError::Connect(e.to_string()))?;

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

    // Re-read under the lock. The pre-flight above is an optimisation and a
    // fast refusal; THIS is the plan that runs.
    let ledger = read_ledger(db, app_id, STORE_OLTP).await?;
    let pending = plan(declared, &ledger)?;
    let mut outcome = Applied {
        applied: Vec::new(),
        already_applied: declared.len() - pending.len(),
    };

    info!(
        schema = %conn.schema,
        pending = pending.len(),
        "applying custom-app schema migrations"
    );

    for m in pending {
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
        })?;

        // The ledger lives in the CONTROL database and the DDL just committed in
        // the TENANT database, so these two cannot be one transaction. The
        // ordering is chosen for which failure is louder: recording second means
        // a crash in this window re-attempts the file on the next promote, where
        // non-idempotent DDL fails with Postgres's own `already exists`. The
        // other order would record a file that never ran and silently skip it
        // forever. Loud and recoverable beats silent and wrong.
        custom_app_migrations::ActiveModel {
            app_id: Set(app_id),
            store: Set(STORE_OLTP.to_string()),
            filename: Set(m.filename.clone()),
            checksum: Set(m.checksum.clone()),
            applied_at: Set(Utc::now().fixed_offset()),
            applied_by_build: Set(Some(build_pk)),
        }
        .insert(db)
        .await
        .map_err(|e| {
            warn!(filename = %m.filename, error = %e, "migration applied but not recorded");
            MigrationError::LedgerWriteFailed {
                filename: m.filename.clone(),
                message: e.to_string(),
            }
        })?;

        info!(filename = %m.filename, schema = %conn.schema, "applied custom-app migration");
        outcome.applied.push(m.filename.clone());
    }

    // Best-effort: the lock is session-scoped and the client is dropped on
    // return anyway.
    let _ = client
        .execute("SELECT pg_advisory_unlock($1)", &[&lock_key])
        .await;

    Ok(outcome)
}
