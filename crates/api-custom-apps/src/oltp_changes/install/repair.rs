//! Drift repair (OLTP change stream design §5.2, layer 3): a table's capture
//! put back as it is installed, once the relay's drift check finds its
//! triggers missing, disabled or calling another function, or its install
//! left uncommitted.
//!
//! A repair **never adds or removes capture of a table**. The relay compares
//! the store with the build production serves, and a promote installs its
//! build's capture before production moves to it: in between, a table the new
//! build removed looks uncaptured and one it added looks undeclared. So a
//! repair reinstalls only a table that has its `oxy_changes.tables` row, at
//! the version recorded there, and removes nothing; any other drift is the
//! next promote's to fix ([`Drift::repairable`]).
//!
//! The repair only tries the app's install lock (`INSTALL_TRY_LOCK_SQL`): a
//! promote installing the app's capture meanwhile is left to finish, and the
//! next pass checks again. Under the lock the drift is read once more, since
//! the install that held it may have put everything back. Every transaction
//! of the reinstall, on either session, runs under the relay's lock and
//! statement timeouts, so a table busy behind a long transaction fails the
//! repair rather than the pass.

use oxy_custom_apps::manifest::OltpChangesConfig;
use oxy_oltp::changes::{
    DeclaredTable, Drift, INSTALL_TRY_LOCK_SQL, OWNER_SESSION_PIN_SQL, drift_sql, lock_timeout_sql,
    pin_session_sql,
};
use sea_orm::DatabaseConnection;
use tokio_postgres::Client;
use uuid::Uuid;

use super::plan::{Installed, Plan};
use super::{
    InstallError, Writer, apply_plan, pg_infra, plan, plan_against, read_installed,
    writer_connection,
};
use crate::oltp_changes::owner::{current_tenant, owner_client};
use crate::oltp_changes::relay::{BASELINE_STATEMENT_TIMEOUT, BEGIN_LOCK_TIMEOUT};

/// What a repair did.
#[derive(Debug)]
pub enum Repair {
    /// Another install of the app's capture holds its lock: nothing was done.
    Busy,
    /// Under the lock, none of the app's declared tables had drifted in a way
    /// a repair puts back.
    Clean,
    /// The tables whose capture was installed again as it stands. A drifted
    /// table not among them is the next promote's to fix.
    Repaired(Vec<String>),
}

/// Under `INSTALL_TRY_LOCK_SQL`: re-run `drift_sql` for this app's declared
/// tables, then reinstall, as it stands, each one whose drift a repair puts
/// back. Nothing is captured anew and nothing removed.
pub async fn repair(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_slug: &str,
    declared: &OltpChangesConfig,
) -> Result<Repair, InstallError> {
    let declared = plan::declared_tables(Some(declared));
    let Some(writer) = Writer::of(app_slug, true)? else {
        return Ok(Repair::Clean);
    };
    let Some(tenant) = current_tenant(db, org_id, true)
        .await
        .map_err(InstallError::Infra)?
    else {
        return Ok(Repair::Clean);
    };
    let writer = writer.in_store(&tenant);
    let mut owner = owner_client(&tenant, "oxy_changes drift repair")
        .await
        .map_err(InstallError::Infra)?;
    owner
        .batch_execute(OWNER_SESSION_PIN_SQL)
        .await
        .map_err(|e| pg_infra("pin the owner's search_path", &e))?;
    if !try_lock(&owner, &writer.name).await? {
        return Ok(Repair::Busy);
    }
    let drifted = repairable(&mut owner, &writer.name, &declared).await?;
    if drifted.is_empty() {
        return Ok(Repair::Clean);
    }
    let tables = reinstall(db, org_id, &writer, owner, &drifted).await?;
    Ok(Repair::Repaired(tables))
}

/// Reinstall `drifted` as it stands, under the repair's timeouts: the tables
/// it put back.
async fn reinstall(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer: &Writer,
    owner: Client,
    drifted: &[DeclaredTable],
) -> Result<Vec<String>, InstallError> {
    let installed = read_installed(&owner, &writer.name).await?;
    let plan = as_installed(
        plan_against(&owner, writer, drifted, &installed).await?,
        &installed,
    );
    let tables: Vec<String> = plan
        .install
        .iter()
        .map(|t| t.capture.shape.table.clone())
        .collect();
    if !tables.is_empty() {
        let conn = writer_connection(db, org_id, writer).await?;
        apply_plan(conn, writer, owner, &plan, Some(timeouts_sql())).await?;
    }
    Ok(tables)
}

/// `INSTALL_TRY_LOCK_SQL` for `writer` on `owner`: whether it took the lock.
async fn try_lock(owner: &Client, writer: &str) -> Result<bool, InstallError> {
    owner
        .query_one(INSTALL_TRY_LOCK_SQL, &[&writer])
        .await
        .and_then(|row| row.try_get(0))
        .map_err(|e| pg_infra("try the app's capture install lock", &e))
}

/// The tables of `declared` whose drift, by `drift_sql` in a transaction
/// pinned as the relay's are, a repair puts back.
async fn repairable(
    owner: &mut Client,
    writer: &str,
    declared: &[DeclaredTable],
) -> Result<Vec<DeclaredTable>, InstallError> {
    let doing = "check the app's capture for drift";
    let pg = |e: tokio_postgres::Error| pg_infra(doing, &e);
    let tables: Vec<&str> = declared.iter().map(|t| t.name.as_str()).collect();
    let writers = vec![writer; tables.len()];
    let txn = owner.transaction().await.map_err(pg)?;
    txn.batch_execute(&pin_session_sql(
        BEGIN_LOCK_TIMEOUT,
        BASELINE_STATEMENT_TIMEOUT,
    ))
    .await
    .map_err(pg)?;
    let rows = txn
        .query(&drift_sql(), &[&writers, &tables])
        .await
        .map_err(pg)?;
    txn.commit().await.map_err(pg)?;
    let mut names = Vec::new();
    for row in &rows {
        let table: Option<String> = row.try_get("table_name").map_err(pg)?;
        let code: String = row.try_get("drift").map_err(pg)?;
        if Drift::from_code(&code).is_some_and(Drift::repairable) {
            names.extend(table);
        }
    }
    Ok(declared
        .iter()
        .filter(|t| names.contains(&t.name))
        .cloned()
        .collect())
}

/// `plan` cut to the tables it reinstalls as they are recorded: each with a
/// `tables` row of its oid at the version the plan gives it. Nothing is
/// captured anew, reshaped, or removed.
fn as_installed(mut plan: Plan, installed: &[Installed]) -> Plan {
    plan.install.retain(|t| {
        installed
            .iter()
            .any(|row| row.relid == t.capture.shape.relid && row.schema_version == t.schema_version)
    });
    plan.remove.clear();
    plan
}

/// What every transaction of a repair's reinstall starts with: the lock
/// timeout of a baseline's begin and the statement timeout of its steps.
fn timeouts_sql() -> String {
    format!(
        "{}; SET LOCAL statement_timeout = '{}ms'",
        lock_timeout_sql(BEGIN_LOCK_TIMEOUT),
        BASELINE_STATEMENT_TIMEOUT.as_millis()
    )
}
