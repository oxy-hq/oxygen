//! Change capture, installed at a production promote (OLTP change stream
//! design §5.2, "Triggers, installed at publish" and "Install order").
//!
//! [`install_on_promote`] runs after the app's OLTP migrations and before its
//! Airhouse migrations and the pointer move, at both production promotes: a
//! promoting publish (`custom_apps_publish::pipeline::promote`) and every
//! other move of production (`custom_apps_promote::production`). In order:
//!
//! 1. **The flag.** With `oltp_changes` off for the org it does nothing and
//!    opens no store connection, whatever the build declares.
//! 2. **What is installed**, read as the store's owner ([`INSTALLED_SQL`]),
//!    once the store is brought current so `oxy_changes` (platform step 8)
//!    exists. Nothing declared and nothing installed for the app's writer:
//!    done, after that one read.
//! 3. **Every declared table's shape** ([`SHAPE_SQL`]), each validated before
//!    anything changes. One table at fault refuses the promote with every
//!    fault listed ([`InstallError::Author`]) and leaves what is installed as
//!    it was, so the author learns it before production moves and the app
//!    keeps capturing its other tables meanwhile.
//! 4. **Every table's owner**, read and checked before anything changes
//!    (`attach`). A table any role but the app's writer or the store's owner
//!    owns refuses the promote, naming it and its owner
//!    ([`InstallError::Author`]), and leaves what is installed as it was.
//! 5. **Per declared table**: owner install, the triggers attached as the
//!    table's owner, owner commit, each in its own transaction, so the guard
//!    holds a table only once its triggers are in place.
//! 6. **Per installed table no longer declared**: owner uninstall, the
//!    triggers dropped as the table's owner, owner drop. Its shapes stay, for
//!    the history already shipped.
//!
//! The writer is the app's own, derived from its slug as its migrations'
//! writer is: the manifest names tables, never a schema. The functions and
//! the registry rows go in as the store's owner. A table's triggers go on and
//! come off as the role that owns the table, since enabling or dropping a
//! trigger is its owner's alone: the writer, for a table the app's migrations
//! made (they run as the writer); the store's owner, for one a workspace's
//! `schemas/*.sql` made in the app's schema (they run as the owner, and grant
//! the writer `ALL` on it, but not its ownership).
//!
//! The install's session as the store's owner is pinned to `search_path =
//! pg_catalog, pg_temp` ([`OWNER_SESSION_PIN_SQL`]) before its first read:
//! the owner's default path names every app's schema, where an app could
//! plant an operator or function for the owner to resolve. Each owner install
//! also makes sure of the writer's image role (`image_role_for`), which builds
//! its captured images so an app's cast to `json` runs with no privileges.
//!
//! A failure part way leaves the tables already done installed, each whole,
//! and the same promote retried installs the rest. Nothing is undone, as
//! nothing a migration did is.
//!
//! An install holds the writer's advisory lock (`INSTALL_LOCK_SQL`) on its
//! owner session from step 2 until it ends, so two installs of one app never
//! interleave: a promote waits for another, and the relay's drift repair
//! ([`repair`]) passes over an app whose install is running.

use oxy_custom_apps::manifest::OltpChangesConfig;
use oxy_oltp::changes::{
    BaselineReason, CaptureSpec, Column, DeclaredTable, INSTALL_LOCK_SQL, INSTALLED_SQL,
    OWNER_SESSION_PIN_SQL, SHAPE_SQL, ShapeRow, image_role_for, owner_commit_sql, owner_drop_sql,
    owner_install_sql, owner_uninstall_sql, shape_from_rows, writer_install_sql,
    writer_uninstall_sql,
};
use oxy_oltp::resolver::WriterConnection;
use oxy_oltp::schema::{SchemaError, WriterRef, app_writer_name};
use sea_orm::DatabaseConnection;
use tokio_postgres::Client;
use uuid::Uuid;

use super::owner::{current_tenant, owner_client};
use crate::custom_apps_migrations::MigrationError;

mod attach;
mod plan;
pub mod repair;

use attach::{Attach, Attacher, Detach, Roles};
use plan::{Change, Installed, Plan, Recorded};

/// The latest shape recorded for a table (`$1`, its oid), whether or not it is
/// captured now: `INSTALLED_SQL` joins shapes only to tables still in
/// `oxy_changes.tables`, and a table removed and declared again must continue
/// its versions.
const LATEST_SHAPE_SQL: &str = "SELECT schema_version, columns::text AS columns, primary_key \
       FROM oxy_changes.shapes WHERE relid = $1 ORDER BY schema_version DESC LIMIT 1";

/// What one promote did to the app's capture, by table name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InstallSummary {
    /// Captured from this promote on, and not before.
    pub installed: Vec<String>,
    /// Captured before; its captured columns or key changed, so it records a
    /// new `schema_version`.
    pub reshaped: Vec<String>,
    /// No longer declared: its capture was removed and its shapes kept.
    pub removed: Vec<String>,
}

impl InstallSummary {
    pub fn is_empty(&self) -> bool {
        self.installed.is_empty() && self.reshaped.is_empty() && self.removed.is_empty()
    }

    /// One line for the promote's log; empty when nothing changed, so a caller
    /// can test it and log.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = [
            ("installed on", &self.installed),
            ("reshaped on", &self.reshaped),
            ("removed from", &self.removed),
        ]
        .into_iter()
        .filter(|(_, tables)| !tables.is_empty())
        .map(|(verb, tables)| format!("{verb} {}", tables.join(", ")))
        .collect();
        if parts.is_empty() {
            String::new()
        } else {
            format!("change capture {}", parts.join("; "))
        }
    }
}

/// Why capture could not be installed. Two answers to "whose problem is
/// this", as [`MigrationError`] gives them, and surfaced the same way.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The build declares what its store cannot capture as it stands: a table
    /// missing after the migrations or without a primary key, a reserved
    /// column, a bad `exclude`, a name the capture SQL refuses. The author
    /// changes the build; the same promote again changes nothing.
    #[error("{0}")]
    Author(String),
    /// The store or the platform failed around the install: unreachable, not
    /// provisioned, a statement that failed. The same promote again may pass.
    #[error("{0}")]
    Infra(String),
}

/// What [`MigrationError::Infra`] says was being applied.
const APPLYING: &str = "change capture (oltpChanges)";

/// Onto a migration's own surfaces: an author error is refused like a bad
/// `migrations` block (`422`, `migration_failed` on a promote), an infra
/// error like a store that dropped mid-apply (`409`, retryable).
impl From<InstallError> for MigrationError {
    fn from(e: InstallError) -> Self {
        match e {
            InstallError::Author(message) => MigrationError::BadManifest(message),
            InstallError::Infra(message) => MigrationError::Infra {
                filename: APPLYING.to_string(),
                message,
            },
        }
    }
}

/// Install capture on the tables `declared` names, in the writer `app_slug`
/// derives, and remove it from the tables that writer's capture covers and
/// `declared` no longer names. See the module docs.
pub async fn install_on_promote(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_slug: &str,
    declared: Option<&OltpChangesConfig>,
) -> Result<InstallSummary, InstallError> {
    let declared = plan::declared_tables(declared);
    if !super::enabled_for(org_id) {
        if !declared.is_empty() {
            tracing::info!(
                %org_id,
                app_slug,
                tables = declared.len(),
                "oltp_changes is off for this org: change capture is not installed"
            );
        }
        return Ok(InstallSummary::default());
    }
    let Some(writer) = Writer::of(app_slug, !declared.is_empty())? else {
        return Ok(InstallSummary::default());
    };
    let Some(tenant) = current_tenant(db, org_id, !declared.is_empty())
        .await
        .map_err(InstallError::Infra)?
    else {
        return Ok(InstallSummary::default());
    };
    let writer = writer.in_store(&tenant);
    let owner = owner_client(&tenant, "oxy_changes install")
        .await
        .map_err(InstallError::Infra)?;
    owner
        .batch_execute(OWNER_SESSION_PIN_SQL)
        .await
        .map_err(|e| pg_infra("pin the owner's search_path", &e))?;
    owner
        .execute(INSTALL_LOCK_SQL, &[&writer.name])
        .await
        .map_err(|e| pg_infra("wait for the app's other capture install", &e))?;
    install_locked(db, org_id, &writer, owner, &declared).await
}

/// Steps 2 to 6 of the module docs, on `owner`, a session holding the
/// writer's install lock (`INSTALL_LOCK_SQL`), which goes with it when the
/// install drops it.
async fn install_locked(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer: &Writer,
    owner: Client,
    declared: &[DeclaredTable],
) -> Result<InstallSummary, InstallError> {
    let installed = read_installed(&owner, &writer.name).await?;
    if declared.is_empty() && installed.is_empty() {
        return Ok(InstallSummary::default());
    }
    let plan = plan_against(&owner, writer, declared, &installed).await?;
    let conn = writer_connection(db, org_id, writer).await?;
    apply_plan(conn, writer, owner, &plan, None).await
}

/// The app's writer's connection to the org's store.
async fn writer_connection(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer: &Writer,
) -> Result<WriterConnection, InstallError> {
    oxy_oltp::resolver::resolve_writer_connection_for_org(db, org_id, &writer.writer)
        .await
        .map_err(|e| infra("resolve the app's writer", e))
}

/// Every declared table read and validated against its live shape, beside
/// the latest shape recorded for it, then planned against what is installed.
/// Nothing is changed: a refusal here leaves the store as it was.
async fn plan_against(
    owner: &Client,
    writer: &Writer,
    declared: &[DeclaredTable],
    installed: &[Installed],
) -> Result<Plan, InstallError> {
    let captures = read_and_validate(owner, &writer.schema, declared).await?;
    let mut recorded = Vec::with_capacity(captures.len());
    for capture in captures {
        let latest = latest_shape(owner, capture.shape.relid).await?;
        recorded.push((capture, latest));
    }
    Ok(plan::plan(recorded, installed))
}

/// Carry out `plan`: each table installed or reshaped, as the store's owner,
/// then the table's owner, then the store's owner; then each removed table
/// taken down the same way backwards. `timeouts`, when given, starts every
/// one of those transactions, on either session. The writer's image role
/// (`Writer::image_role`) builds its images.
async fn apply_plan(
    conn: WriterConnection,
    writer: &Writer,
    owner: Client,
    plan: &Plan,
    timeouts: Option<String>,
) -> Result<InstallSummary, InstallError> {
    // Every statement is generated, and every table's owner checked, before
    // the first runs: a name the capture SQL refuses, or a table neither the
    // writer nor the store's owner owns, is the author's to fix, and is
    // refused with nothing changed.
    let tables = install_sql(plan, writer, &conn.role, &writer.image_role)?;
    let store_owner = attach::store_owner(&owner).await?;
    let roles = Roles {
        writer: &conn.role,
        owner: &store_owner,
    };
    let steps = attach::decide(&owner, roles, &tables, &plan.remove, &writer.schema).await?;
    let writer_client = oxy_oltp::connect::connect(&conn.dsn, "oxy_changes install")
        .await
        .map_err(|e| pg_infra("open the org's OLTP store as the app's writer", &e))?;
    let mut sessions = Sessions {
        owner,
        writer: writer_client,
        timeouts,
    };
    install_tables(&mut sessions, &steps.install).await?;
    remove_tables(&mut sessions, &writer.schema, &steps.remove).await?;
    Ok(plan.summary())
}

/// The install's two sessions on the store: its owner's, which writes the
/// functions and the registry rows, and the app's writer's. A table's
/// triggers go on and come off on the session of whichever owns it.
struct Sessions {
    owner: Client,
    writer: Client,
    /// What every transaction starts with: the repair's timeouts, or nothing
    /// for a promote's install.
    timeouts: Option<String>,
}

impl Sessions {
    /// `statements` in one transaction on the session `on` names.
    async fn run(
        &mut self,
        on: Attacher,
        statements: &[String],
        doing: &str,
    ) -> Result<(), InstallError> {
        let client = match on {
            Attacher::Writer => &mut self.writer,
            Attacher::Owner => &mut self.owner,
        };
        in_transaction(client, self.timeouts.as_deref(), statements, doing).await
    }
}

/// The app's writer, derived from its slug as `ctx.oltp` and its migrations
/// derive it.
struct Writer {
    /// The bare name (`bookings`): `oxy_changes.tables.writer`, and what each
    /// log row records.
    name: String,
    /// `app_bookings`: where its tables and their capture functions live.
    schema: String,
    writer: WriterRef,
    /// Its image role in the store ([`image_role_for`]), which builds its
    /// captured images: set by [`Writer::in_store`], empty before, and an
    /// empty name is refused by every install statement built from it.
    image_role: String,
}

impl Writer {
    /// `None` for a slug that backs no schema when nothing is `required`:
    /// nothing can have been captured for it. Declared tables make it the
    /// author's to fix, as a migration's `NoSchema` is.
    fn of(app_slug: &str, required: bool) -> Result<Option<Self>, InstallError> {
        let named = app_writer_name(app_slug)
            .and_then(|name| WriterRef::app(name.clone()).ok().map(|w| (name, w)));
        match named {
            Some((name, writer)) => Ok(Some(Self {
                schema: writer.schema_name(),
                name,
                writer,
                image_role: String::new(),
            })),
            None if !required => Ok(None),
            None => Err(InstallError::Author(format!(
                "the app's slug '{app_slug}' cannot back an OLTP schema, so the tables under \
                 oltpChanges cannot be captured"
            ))),
        }
    }

    /// This writer in `tenant`'s store, its image role named.
    fn in_store(self, tenant: &oxy_oltp::entity::tenants::Model) -> Self {
        Self {
            image_role: image_role_for(&tenant.provider, &tenant.database_name, &self.name),
            ..self
        }
    }
}

/// The writer's `oxy_changes.tables` rows.
async fn read_installed(owner: &Client, writer: &str) -> Result<Vec<Installed>, InstallError> {
    let doing = "read the app's installed change capture";
    let rows = owner
        .query(INSTALLED_SQL, &[&writer])
        .await
        .map_err(|e| pg_infra(doing, &e))?;
    rows.iter()
        .map(|row| -> Result<Installed, tokio_postgres::Error> {
            Ok(Installed {
                relid: row.try_get("relid")?,
                table: row.try_get("table_name")?,
                schema_version: row.try_get("schema_version")?,
                installed: row.try_get("installed")?,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| pg_infra(doing, &e))
}

/// Each declared table's shape in `schema`, all validated before any is
/// installed (`plan::validate_all`).
async fn read_and_validate(
    owner: &Client,
    schema: &str,
    declared: &[DeclaredTable],
) -> Result<Vec<oxy_oltp::changes::CaptureShape>, InstallError> {
    let mut read = Vec::with_capacity(declared.len());
    for decl in declared {
        let doing = format!("read the shape of table {:?}", decl.name);
        let rows = owner
            .query(SHAPE_SQL, &[&schema, &decl.name])
            .await
            .map_err(|e| pg_infra(&doing, &e))?;
        let rows = rows
            .iter()
            .map(ShapeRow::from_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| pg_infra(&doing, &e))?;
        read.push((decl.clone(), shape_from_rows(&rows)));
    }
    plan::validate_all(&read).map_err(InstallError::Author)
}

async fn latest_shape(owner: &Client, relid: u32) -> Result<Option<Recorded>, InstallError> {
    let doing = "read the table's recorded capture shape";
    let Some(row) = owner
        .query_opt(LATEST_SHAPE_SQL, &[&relid])
        .await
        .map_err(|e| pg_infra(doing, &e))?
    else {
        return Ok(None);
    };
    let read =
        |row: &tokio_postgres::Row| -> Result<(i32, String, Vec<String>), tokio_postgres::Error> {
            Ok((
                row.try_get("schema_version")?,
                row.try_get("columns")?,
                row.try_get("primary_key")?,
            ))
        };
    let (version, columns, primary_key) = read(&row).map_err(|e| pg_infra(doing, &e))?;
    let columns: Vec<Column> = serde_json::from_str(&columns).map_err(|e| infra(doing, e))?;
    Ok(Some(Recorded {
        version,
        columns,
        primary_key,
    }))
}

/// One table's install, as SQL for each connection. `writer_install` runs as
/// the table's owner, which `attach` reads by `relid`.
struct TableSql {
    table: String,
    relid: u32,
    owner_install: Vec<String>,
    writer_install: Vec<String>,
    owner_commit: String,
}

/// `role` is the writer's real role name (qualified on a shared-cluster
/// provider), the one `EXECUTE` is granted to; `image_role` builds images.
fn install_sql(
    plan: &Plan,
    writer: &Writer,
    role: &str,
    image_role: &str,
) -> Result<Vec<TableSql>, InstallError> {
    plan.install
        .iter()
        .map(|t| -> Result<TableSql, InstallError> {
            let table = &t.capture.shape.table;
            let spec = CaptureSpec {
                writer: &writer.name,
                capture: &t.capture,
                schema_version: t.schema_version,
            };
            Ok(TableSql {
                table: table.clone(),
                relid: t.capture.shape.relid,
                owner_install: owner_install_sql(&spec, role, image_role, baseline_for(t.change))
                    .map_err(|e| schema_refusal(table, &e))?,
                writer_install: writer_install_sql(&spec).map_err(|e| schema_refusal(table, &e))?,
                owner_commit: owner_commit_sql(t.capture.shape.relid, t.schema_version),
            })
        })
        .collect()
}

/// Why a table's install needs a baseline: a table captured anew is opted in,
/// one captured at a new shape restarts its current state; one reinstalled as
/// it stands needs none.
fn baseline_for(change: Change) -> Option<BaselineReason> {
    match change {
        Change::New => Some(BaselineReason::OptIn),
        Change::Reshaped => Some(BaselineReason::Reshape),
        Change::Unchanged => None,
    }
}

fn schema_refusal(table: &str, e: &SchemaError) -> InstallError {
    InstallError::Author(format!(
        "oltpChanges table {table:?} cannot be captured, since a name its capture is built from \
         is not one the platform accepts: {e}"
    ))
}

/// Owner install, the triggers attached as the table's owner, owner commit,
/// per table in order.
async fn install_tables(sessions: &mut Sessions, steps: &[Attach<'_>]) -> Result<(), InstallError> {
    for step in steps {
        let t = step.sql;
        let name = &t.table;
        let doing = format!("write the capture functions of {name:?} as the store's owner");
        sessions
            .run(Attacher::Owner, &t.owner_install, &doing)
            .await?;
        let by = step.by;
        let doing = format!("attach the capture triggers to {name:?} as {}", by.role());
        sessions.run(by, &t.writer_install, &doing).await?;
        let doing = format!("commit the capture of {name:?}");
        let commit = std::slice::from_ref(&t.owner_commit);
        sessions.run(Attacher::Owner, commit, &doing).await?;
    }
    Ok(())
}

/// Owner uninstall, the triggers dropped as the table's owner, owner drop,
/// per table in order. The triggers are dropped on the table the oid names
/// now, and not at all when it is gone (its triggers went with it) or left
/// the writer's schema.
async fn remove_tables(
    sessions: &mut Sessions,
    schema: &str,
    steps: &[Detach<'_>],
) -> Result<(), InstallError> {
    for step in steps {
        let row = step.row;
        let name = &row.table;
        let doing = format!("release the capture of {name:?} from the guard");
        let release = [owner_uninstall_sql(row.relid)];
        sessions.run(Attacher::Owner, &release, &doing).await?;
        if let Some((live, by)) = &step.triggers {
            let doing = format!("drop the capture triggers of {name:?} as {}", by.role());
            let sql = writer_uninstall_sql(&live.schema, &live.table);
            sessions.run(*by, &sql, &doing).await?;
        }
        let doing = format!("drop the capture functions of {name:?} as the store's owner");
        let functions = owner_drop_sql(row.relid, schema);
        sessions.run(Attacher::Owner, &functions, &doing).await?;
    }
    Ok(())
}

/// `statements` in one transaction, after `timeouts` when given, so a step
/// that fails leaves nothing of itself behind.
async fn in_transaction(
    client: &mut Client,
    timeouts: Option<&str>,
    statements: &[String],
    doing: &str,
) -> Result<(), InstallError> {
    let txn = client
        .transaction()
        .await
        .map_err(|e| pg_infra(doing, &e))?;
    if let Some(timeouts) = timeouts {
        txn.batch_execute(timeouts)
            .await
            .map_err(|e| pg_infra(doing, &e))?;
    }
    for statement in statements {
        txn.batch_execute(statement)
            .await
            .map_err(|e| pg_infra(doing, &e))?;
    }
    txn.commit().await.map_err(|e| pg_infra(doing, &e))
}

fn infra(doing: &str, e: impl std::fmt::Display) -> InstallError {
    InstallError::Infra(format!("{doing}: {e}"))
}

/// A Postgres failure with its SQLSTATE, message, detail and hint:
/// `tokio_postgres::Error`'s own `Display` is the bare "db error".
fn pg_infra(doing: &str, e: &tokio_postgres::Error) -> InstallError {
    infra(doing, oxy_oltp::connect::pg_detail(e))
}

#[cfg(test)]
mod tests;
