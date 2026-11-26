//! The pass's drift check (OLTP change stream design §5.2, layer 3), after a
//! bump's re-stamp and before the baselines' requests: every table the org's
//! production builds declare under `oltpChanges`, checked against the capture
//! the store has, in one store query (`drift_sql`).
//!
//! - **A table whose triggers drifted**, or whose install was left
//!   uncommitted, is repaired (`install::repair`): installed again as it
//!   stands, under the app's install lock, then, after the lease is renewed,
//!   a `drift` baseline requested of it, since its rows changed uncaptured.
//!   An install running meanwhile is left to finish (`Busy`), and the next
//!   pass checks again.
//! - **Any other drift** (a declared table never captured, or dropped or
//!   renamed under its capture) is only recorded, once while it lasts: a
//!   repair never adds or removes capture of a table, since a promote
//!   installs its build's capture before production moves to that build
//!   ([`Drift::repairable`]).
//! - **No guard**, after the lease is renewed: the store's DDL guard is
//!   created, or enabled, again as the owner. One the owner may not create
//!   (`insufficient_privilege`, the local provider) is recorded once, in
//!   `stream.guardless_since`, and not tried again; any other failure is
//!   tried again next pass. A guard found again clears the record.
//!
//! Each repair, each drift recorded, and each guard put back or recorded
//! missing logs `oltp_changes_drift_repaired` with its `outcome`. Nothing here
//! fails the pass: the caller logs an error and ships anyway.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, PoisonError};

use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::env_resolve::load_environment_builds_batch;
use oxy_custom_apps::manifest::{OltpChangesConfig, oltp_changes_from_manifest};
use oxy_oltp::changes::{
    BaselineReason, Drift, GUARDED_SQL, GUARDLESS_SQL, StreamState, drift_sql, pin_session_sql,
    request_named_baseline_sql,
};
use oxy_oltp::platform::repair_guard_sql;
use oxy_oltp::schema::app_writer_name;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tokio_postgres::error::SqlState;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Row};
use uuid::Uuid;

use super::pass::{Pass, Stop, renew};
use super::store::pg;
use super::{BASELINE_STATEMENT_TIMEOUT, BEGIN_LOCK_TIMEOUT};
use crate::oltp_changes::install::repair::{Repair, repair};

/// An app whose production build declares tables under `oltpChanges`.
pub(super) struct Declared {
    pub app_slug: String,
    /// The bare writer name its slug derives.
    pub writer: String,
    pub config: OltpChangesConfig,
}

/// The org's apps, their production builds through
/// `load_environment_builds_batch` and `.resolve(&AppEnvironment::Production)`,
/// then one `app_builds` query, read through `oltp_changes_from_manifest`.
/// Three platform queries a pass. A manifest that does not parse is logged
/// and skipped.
pub(super) async fn live_declared(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<Vec<Declared>, String> {
    let apps = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .all(db)
        .await
        .map_err(|e| format!("read the org's apps: {e}"))?;
    let builds = load_environment_builds_batch(db, &apps)
        .await
        .map_err(|e| format!("read the org's apps' environments: {e}"))?;
    let production: HashMap<Uuid, &apps::Model> = apps
        .iter()
        .filter_map(|app| {
            let build = builds.get(&app.id)?;
            Some((build.resolve(&AppEnvironment::Production).build_id?, app))
        })
        .collect();
    if production.is_empty() {
        return Ok(Vec::new());
    }
    let rows = app_builds::Entity::find()
        .filter(app_builds::Column::Id.is_in(production.keys().copied()))
        .all(db)
        .await
        .map_err(|e| format!("read the org's production builds: {e}"))?;
    let mut declared: Vec<Declared> = rows
        .iter()
        .filter_map(|build| declared_by(production.get(&build.id)?, build))
        .collect();
    declared.sort_by(|a, b| a.writer.cmp(&b.writer));
    Ok(declared)
}

/// What `build`, `app`'s production build, declares; `None` for nothing, a
/// slug that backs no writer, or a block that does not parse (logged).
fn declared_by(app: &apps::Model, build: &app_builds::Model) -> Option<Declared> {
    let config = match oltp_changes_from_manifest(build.manifest_json.as_ref()?) {
        Ok(config) => config?,
        Err(error) => {
            tracing::warn!(org_id = %app.org_id, app_slug = %app.slug, build = %build.id, %error, "oltp_changes: the production build's oltpChanges does not parse; its capture is not checked for drift");
            return None;
        }
    };
    Some(Declared {
        writer: app_writer_name(&app.slug)?,
        app_slug: app.slug.clone(),
        config,
    })
}

/// What one `drift_sql` found.
#[derive(Default)]
struct Found {
    /// `(writer, table, drift)`, one per drifted pair.
    tables: Vec<(String, String, Drift)>,
    no_guard: bool,
}

/// One store query over every declared pair; then each writer with drift
/// repaired, and the guard put back or recorded missing. `Err` only for the
/// reads; a repair that fails is logged and the others go on.
pub(super) async fn check_and_repair(
    pass: &Pass<'_>,
    owner: &mut Client,
    stream: &StreamState,
) -> Result<(), String> {
    let declared = live_declared(pass.db, pass.org_id).await?;
    let pairs: Vec<(&str, &str)> = declared
        .iter()
        .flat_map(|app| {
            let writer = app.writer.as_str();
            app.config.tables.keys().map(move |t| (writer, t.as_str()))
        })
        .collect();
    let found = drifted(owner, &pairs).await?;
    forget_recorded(pass.org_id, &found.tables);
    for app in &declared {
        let tables: Vec<(&str, Drift)> = found
            .tables
            .iter()
            .filter(|(writer, ..)| *writer == app.writer)
            .map(|(_, table, drift)| (table.as_str(), *drift))
            .collect();
        if !tables.is_empty() {
            repair_app(pass, owner, app, &tables).await?;
        }
    }
    match (found.no_guard, stream.guardless) {
        (true, false) => {
            renew_lease(pass).await?;
            put_back_guard(pass, owner).await
        }
        (false, true) => {
            renew_lease(pass).await?;
            let doing = "clear the record of the store running without its DDL guard";
            pinned(owner, doing, &[GUARDED_SQL]).await
        }
        _ => Ok(()),
    }
}

/// The pass's lease renewed; `Err` when it is lost, so nothing more is
/// written.
async fn renew_lease(pass: &Pass<'_>) -> Result<(), String> {
    renew(pass).await.map_err(|stop| match stop {
        Stop::Abort(error) | Stop::Failed(error) => error,
    })
}

/// [`drift_sql`] over `pairs`, as the owner.
async fn drifted(owner: &mut Client, pairs: &[(&str, &str)]) -> Result<Found, String> {
    let doing = "check the store's capture for drift";
    let (writers, tables): (Vec<&str>, Vec<&str>) = pairs.iter().copied().unzip();
    let rows = pinned_query(owner, doing, &drift_sql(), &[&writers, &tables]).await?;
    let mut found = Found::default();
    for row in &rows {
        let code: String = row.try_get("drift").map_err(pg(doing))?;
        let drift =
            Drift::from_code(&code).ok_or_else(|| format!("{doing}: an unknown drift {code:?}"))?;
        if drift == Drift::NoGuard {
            found.no_guard = true;
            continue;
        }
        let writer: String = row.try_get("writer").map_err(pg(doing))?;
        let table: String = row.try_get("table_name").map_err(pg(doing))?;
        found.tables.push((writer, table, drift));
    }
    Ok(found)
}

/// `app`'s drifted `tables`: each one a repair puts back installed again as
/// it stands, then, after the lease is renewed, a `drift` baseline requested
/// of it; every other one recorded. Each table's outcome is logged. `Err`
/// only for a lost lease.
async fn repair_app(
    pass: &Pass<'_>,
    owner: &mut Client,
    app: &Declared,
    tables: &[(&str, Drift)],
) -> Result<(), String> {
    let (repairable, others): (Vec<_>, Vec<_>) = tables
        .iter()
        .copied()
        .partition(|(_, drift)| drift.repairable());
    for (table, drift) in others {
        record(pass, &app.writer, table, drift);
    }
    if repairable.is_empty() {
        return Ok(());
    }
    let put_back = match repair(pass.db, pass.org_id, &app.app_slug, &app.config).await {
        Ok(Repair::Repaired(put_back)) => put_back,
        Ok(Repair::Busy) => {
            tracing::debug!(org_id = %pass.org_id, writer = %app.writer, "oltp_changes: an install of the app's capture is running; its drift is checked again next pass");
            return Ok(());
        }
        Ok(Repair::Clean) => return Ok(()),
        Err(error) => {
            let outcome = format!("failed: {error}");
            for (table, drift) in repairable {
                repaired(pass, &app.writer, table, drift, &outcome);
            }
            return Ok(());
        }
    };
    renew_lease(pass).await?;
    for (table, drift) in repairable {
        if !put_back.iter().any(|t| t == table) {
            record(pass, &app.writer, table, drift);
            continue;
        }
        let outcome = match request_drift_baseline(owner, &app.writer, table).await {
            Ok(()) => "repaired".to_owned(),
            Err(error) => format!("failed: {error}"),
        };
        repaired(pass, &app.writer, table, drift, &outcome);
    }
    Ok(())
}

/// The drift logged `recorded`, as `(org, writer, table, drift)`: each is
/// logged once while it lasts, not every pass.
static RECORDED: Mutex<BTreeSet<(Uuid, String, String, &'static str)>> =
    Mutex::new(BTreeSet::new());

/// Log `writer`'s `table` as `recorded`: drift the next promote fixes, not
/// the relay. Once while it lasts ([`forget_recorded`]).
fn record(pass: &Pass<'_>, writer: &str, table: &str, drift: Drift) {
    let key = (
        pass.org_id,
        writer.to_owned(),
        table.to_owned(),
        drift.as_str(),
    );
    let first = RECORDED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key);
    if first {
        repaired(pass, writer, table, drift, "recorded");
    }
}

/// Forget each of `org_id`'s recorded drifts that is not among `drifted`,
/// so it is logged again if it comes back.
fn forget_recorded(org_id: Uuid, drifted: &[(String, String, Drift)]) {
    RECORDED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(org, writer, table, drift)| {
            *org != org_id
                || drifted
                    .iter()
                    .any(|(w, t, d)| w == writer && t == table && d.as_str() == *drift)
        });
}

/// A `drift` baseline of `writer`'s `table`, whose rows changed uncaptured.
async fn request_drift_baseline(
    owner: &mut Client,
    writer: &str,
    table: &str,
) -> Result<(), String> {
    let doing = "request the drifted table's baseline";
    let request = request_named_baseline_sql(writer, table, BaselineReason::Drift)
        .map_err(|e| format!("{doing}: {e}"))?;
    pinned(owner, doing, &request.each_ref().map(String::as_str)).await
}

/// The guard created, or enabled, again as the owner ([`repair_guard_sql`]),
/// then looked for. An owner that may not (`insufficient_privilege`) has the
/// store recorded guardless (`guardless_since`), which stops the next passes
/// trying; any other failure is logged and tried again next pass.
async fn put_back_guard(pass: &Pass<'_>, owner: &mut Client) -> Result<(), String> {
    let outcome = match create_guard(owner).await {
        Ok(()) if drifted(owner, &[]).await?.no_guard => {
            "failed: the DDL guard is still missing or disabled after creating it".to_owned()
        }
        Ok(()) => "repaired".to_owned(),
        Err(e) if e.code() == Some(&SqlState::INSUFFICIENT_PRIVILEGE) => {
            let doing = "record the store running without its DDL guard";
            pinned(owner, doing, &[GUARDLESS_SQL]).await?;
            "recorded".to_owned()
        }
        Err(e) => format!("failed: {}", oxy_oltp::connect::pg_detail(&e)),
    };
    alarm!(pass.environment, org_id = %pass.org_id, environment = pass.environment, database = pass.database, drift = Drift::NoGuard.as_str(), %outcome, "oltp_changes_drift_repaired");
    Ok(())
}

/// [`repair_guard_sql`] as the owner, pinned, its error as the server gave it.
async fn create_guard(owner: &mut Client) -> Result<(), tokio_postgres::Error> {
    let txn = owner.transaction().await?;
    txn.batch_execute(&pin_session_sql(
        BEGIN_LOCK_TIMEOUT,
        BASELINE_STATEMENT_TIMEOUT,
    ))
    .await?;
    txn.batch_execute(&repair_guard_sql()).await?;
    txn.commit().await
}

/// The `oltp_changes_drift_repaired` line of one drifted table.
fn repaired(pass: &Pass<'_>, writer: &str, table: &str, drift: Drift, outcome: &str) {
    alarm!(pass.environment, org_id = %pass.org_id, environment = pass.environment, database = pass.database, writer, table, drift = drift.as_str(), outcome, "oltp_changes_drift_repaired");
}

/// `statements` as the owner, in one transaction pinned first
/// ([`pin_session_sql`]).
async fn pinned(owner: &mut Client, doing: &str, statements: &[&str]) -> Result<(), String> {
    let txn = owner.transaction().await.map_err(pg(doing))?;
    let pin = pin_session_sql(BEGIN_LOCK_TIMEOUT, BASELINE_STATEMENT_TIMEOUT);
    txn.batch_execute(&pin).await.map_err(pg(doing))?;
    for statement in statements {
        txn.batch_execute(statement).await.map_err(pg(doing))?;
    }
    txn.commit().await.map_err(pg(doing))
}

/// `sql` as the owner, in a transaction pinned first.
async fn pinned_query(
    owner: &mut Client,
    doing: &str,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<Row>, String> {
    let txn = owner.transaction().await.map_err(pg(doing))?;
    let pin = pin_session_sql(BEGIN_LOCK_TIMEOUT, BASELINE_STATEMENT_TIMEOUT);
    txn.batch_execute(&pin).await.map_err(pg(doing))?;
    let rows = txn.query(sql, params).await.map_err(pg(doing))?;
    txn.commit().await.map_err(pg(doing))?;
    Ok(rows)
}
