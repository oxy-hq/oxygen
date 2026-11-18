//! The event log (app events design §7.3, "Mirror events into Airhouse"):
//! every app event, copied into the workspace's own Airhouse as
//! `oxy_event_log.events`, so the event history can be queried with SQL like
//! any other table.
//!
//! **What is copied.** The event's routing fields and its body, as JSON text
//! (`data`), which only the org's own store holds; the platform reads it there
//! for this copy alone. A body already deleted leaves `data` null.
//!
//! **When.** [`mirror_pass`] runs each 30 s beside the table sweep, on whichever
//! worker holds that interval's claim: up to [`BATCH`] events with
//! `mirrored_at` null, oldest first, in orgs with `app_events` on. An event is
//! marked when its copy succeeded or its workspace has no Airhouse to hold one
//! (nothing would ever be read from there). An Airhouse that fails leaves the
//! events unmarked for the next pass.
//!
//! **Idempotent on the event id** like the usage sink: a row is appended only
//! when no row of that id is already there, so a pass that died before it
//! marked repeats harmlessly. The table has no key or index (DuckLake has none).
//!
//! **Kept [`KEEP_DAYS`] days**, pruned by the retention sweeper
//! ([`prune_pass`]), and `oxyc events purge` deletes what it purges from it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use airhouse::entity::tenants::{self, TenantStatus};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::{app_events, apps};
use oxy_cameras::airhouse::escape::{sql_opt_str, sql_str, sql_ts};
use oxy_custom_apps::events::table::TableRef;
use oxy_oltp::events::{OutboxTarget, read_bodies};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Statement,
};
use uuid::Uuid;

use super::envelope::event_source;
use super::stores::Stores;

pub const SCHEMA: &str = "oxy_event_log";
pub const TABLE: &str = "oxy_event_log.events";
/// How long a copy is kept.
pub const KEEP_DAYS: i64 = 90;
/// The most events one pass copies.
pub const BATCH: u64 = 200;

/// No `PRIMARY KEY`, `UNIQUE` or index: DuckLake has none, and a table that
/// carries one leaves its writer inert.
pub const CREATE_TABLE: &str = "CREATE TABLE IF NOT EXISTS oxy_event_log.events (\
     event_id VARCHAR NOT NULL, org_id VARCHAR NOT NULL, environment VARCHAR NOT NULL, \
     type VARCHAR NOT NULL, source VARCHAR NOT NULL, key VARCHAR NOT NULL, \
     published_at TIMESTAMPTZ NOT NULL, recorded_at TIMESTAMPTZ NOT NULL, actor VARCHAR, \
     causation_id VARCHAR, root_id VARCHAR NOT NULL, depth INTEGER NOT NULL, \
     invocation_id VARCHAR, data VARCHAR)";

/// One event as the log holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Logged {
    pub event_id: Uuid,
    pub org_id: Uuid,
    pub environment: String,
    pub event_type: String,
    pub source: String,
    pub key: String,
    pub published_at: DateTime<Utc>,
    pub actor: Option<String>,
    pub causation_id: Option<Uuid>,
    pub root_id: Uuid,
    pub depth: i32,
    pub invocation_id: Option<Uuid>,
    /// The body as JSON text; `None` once the body is gone.
    pub data: Option<String>,
}

/// Why a copy did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkError {
    /// The workspace has no Airhouse. Expected.
    NotConfigured,
    Failed(String),
}

/// Where the log is written; production is [`AirhouseLog`], a test installs a
/// fake ([`install_log`]).
#[async_trait]
pub trait EventLog: Send + Sync {
    async fn append(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
        rows: &[Logged],
    ) -> Result<(), SinkError>;
    /// Delete rows published more than `days` ago.
    async fn prune(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
        days: i64,
    ) -> Result<(), SinkError>;
    /// Delete the rows of `ids`.
    async fn delete(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
        ids: &[Uuid],
    ) -> Result<(), SinkError>;
}

static LOG: LazyLock<RwLock<Arc<dyn EventLog>>> =
    LazyLock::new(|| RwLock::new(Arc::new(AirhouseLog::default())));

/// Replace the process's log. For tests, which have no Airhouse.
pub fn install_log(log: Arc<dyn EventLog>) {
    *LOG.write().unwrap_or_else(|e| e.into_inner()) = log;
}

pub fn log() -> Arc<dyn EventLog> {
    LOG.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn row_sql(row: &Logged) -> String {
    let id = |u: Option<Uuid>| sql_opt_str(u.map(|u| u.to_string()).as_deref());
    format!(
        "({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
        sql_str(&row.event_id.to_string()),
        sql_str(&row.org_id.to_string()),
        sql_str(&row.environment),
        sql_str(&row.event_type),
        sql_str(&row.source),
        sql_str(&row.key),
        sql_ts(row.published_at),
        sql_ts(Utc::now()),
        sql_opt_str(row.actor.as_deref()),
        id(row.causation_id),
        sql_str(&row.root_id.to_string()),
        row.depth,
        id(row.invocation_id),
        sql_opt_str(row.data.as_deref()),
    )
}

const COLUMNS: &str = "event_id, org_id, environment, type, source, key, published_at, \
     recorded_at, actor, causation_id, root_id, depth, invocation_id, data";

/// The statement that appends `rows` that are not there yet.
pub fn insert_sql(rows: &[Logged]) -> Option<String> {
    let oldest = rows.iter().map(|r| r.published_at).min()?;
    let values = rows.iter().map(row_sql).collect::<Vec<_>>().join(",\n  ");
    Some(format!(
        "INSERT INTO {TABLE} ({COLUMNS})\nSELECT {COLUMNS} FROM (VALUES\n  {values}\n) AS v({COLUMNS})\n\
         WHERE NOT EXISTS (SELECT 1 FROM {TABLE} AS t WHERE t.published_at >= {} AND t.event_id = v.event_id)",
        sql_ts(oldest - chrono::Duration::minutes(5))
    ))
}

/// Production's log: the workspace's Airhouse, the schema made as Admin once
/// per process, rows written as a Writer.
#[derive(Default)]
pub struct AirhouseLog {
    ensured: Mutex<HashMap<Uuid, Arc<tokio::sync::OnceCell<()>>>>,
}

fn failed(e: impl std::fmt::Display) -> SinkError {
    SinkError::Failed(e.to_string().chars().take(500).collect())
}

/// Whether `workspace_id` has an active Airhouse tenant, and the deployment an
/// Airhouse at all: the one lookup the event log copy and the table reader
/// share.
pub(crate) async fn workspace_has_airhouse(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<bool, DbErr> {
    if airhouse::token_broker().is_none() {
        return Ok(false);
    }
    Ok(tenants::Entity::find()
        .filter(tenants::Column::WorkspaceId.eq(workspace_id))
        .filter(tenants::Column::Status.eq(TenantStatus::Active))
        .one(db)
        .await?
        .is_some())
}

impl AirhouseLog {
    async fn has_tenant(db: &DatabaseConnection, workspace_id: Uuid) -> Result<(), SinkError> {
        match workspace_has_airhouse(db, workspace_id).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(SinkError::NotConfigured),
            Err(e) => Err(failed(e)),
        }
    }

    async fn ensure(&self, workspace_id: Uuid) -> Result<(), SinkError> {
        let cell = Arc::clone(
            self.ensured
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(workspace_id)
                .or_default(),
        );
        cell.get_or_try_init(|| async {
            let admin = oxy_cameras::airhouse::connect(
                workspace_id,
                airhouse::UserRole::Admin,
                airhouse::SystemPurpose::EventLog,
                oxy_cameras::airhouse::ingest_ttl(),
            )
            .await
            .map_err(failed)?;
            admin
                .simple_query(&format!("CREATE SCHEMA IF NOT EXISTS {SCHEMA}"))
                .await
                .map_err(failed)?;
            admin.simple_query(CREATE_TABLE).await.map_err(failed)?;
            Ok(())
        })
        .await
        .map(|_| ())
    }

    async fn writer(
        workspace_id: Uuid,
    ) -> Result<Arc<oxy_cameras::airhouse::client::TenantClient>, SinkError> {
        oxy_cameras::airhouse::client::tenant_client(
            workspace_id,
            airhouse::UserRole::Writer,
            airhouse::SystemPurpose::EventLog,
            oxy_cameras::airhouse::ingest_ttl(),
            oxy_cameras::airhouse::client::Pool::Serving,
        )
        .await
        .map_err(failed)
    }
}

#[async_trait]
impl EventLog for AirhouseLog {
    async fn append(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
        rows: &[Logged],
    ) -> Result<(), SinkError> {
        let Some(sql) = insert_sql(rows) else {
            return Ok(());
        };
        Self::has_tenant(db, workspace_id).await?;
        self.ensure(workspace_id).await?;
        Self::writer(workspace_id)
            .await?
            .simple_query(&sql)
            .await
            .map_err(failed)?;
        Ok(())
    }

    async fn prune(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
        days: i64,
    ) -> Result<(), SinkError> {
        Self::has_tenant(db, workspace_id).await?;
        let sql =
            format!("DELETE FROM {TABLE} WHERE published_at < now() - INTERVAL '{days} days'");
        match Self::writer(workspace_id).await?.simple_query(&sql).await {
            Ok(_) => Ok(()),
            // Nothing was ever copied there: no table, nothing to prune.
            Err(e)
                if e.to_string().contains("does not exist")
                    || format!("{e:?}").contains("does not exist") =>
            {
                Ok(())
            }
            Err(e) => Err(failed(e)),
        }
    }

    async fn delete(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
        ids: &[Uuid],
    ) -> Result<(), SinkError> {
        if ids.is_empty() {
            return Ok(());
        }
        Self::has_tenant(db, workspace_id).await?;
        let list = ids
            .iter()
            .map(|i| sql_str(&i.to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("DELETE FROM {TABLE} WHERE event_id IN ({list})");
        match Self::writer(workspace_id).await?.simple_query(&sql).await {
            Ok(_) => Ok(()),
            Err(e) if format!("{e:?}").contains("does not exist") => Ok(()),
            Err(e) => Err(failed(e)),
        }
    }
}

/// One pass of the copy. Never fails the sweep: a problem is logged and the
/// events stay unmarked for the next pass.
pub async fn mirror_pass(db: &DatabaseConnection) {
    if let Err(e) = mirror_once(db).await {
        tracing::warn!(error = %e, "app events: the event log copy failed");
    }
}

/// What one [`mirror_once`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MirrorStats {
    pub copied: usize,
    pub skipped: usize,
}

pub async fn mirror_once(db: &DatabaseConnection) -> Result<MirrorStats, DbErr> {
    let pending = pending_events(db).await?;
    let mut stats = MirrorStats::default();
    let mut by_workspace: HashMap<(Uuid, Uuid), Vec<app_events::Model>> = HashMap::new();
    for event in pending {
        by_workspace
            .entry((event.org_id, event.workspace_id))
            .or_default()
            .push(event);
    }
    let log = log();
    for ((org_id, workspace_id), events) in by_workspace {
        let rows = logged(db, org_id, &events).await?;
        match log.append(db, workspace_id, &rows).await {
            Ok(()) => stats.copied += events.len(),
            // Nothing would ever read from there: mark, so it is not retried.
            Err(SinkError::NotConfigured) => stats.skipped += events.len(),
            Err(SinkError::Failed(e)) => {
                tracing::warn!(%org_id, %workspace_id, error = %e, "app events: the event log copy failed");
                continue;
            }
        }
        let ids: Vec<Uuid> = events.iter().map(|e| e.id).collect();
        let marked = mark(db, &ids).await?;
        // An event no longer there was purged between the read and now, and
        // its log row was just appended after the purge deleted the log's: delete
        // it again, so a purge always wins.
        let purged: Vec<Uuid> = ids.into_iter().filter(|id| !marked.contains(id)).collect();
        if !purged.is_empty()
            && let Err(SinkError::Failed(e)) = log.delete(db, workspace_id, &purged).await
        {
            tracing::warn!(%workspace_id, error = %e, "app events: a purged event stayed in the event log");
        }
    }
    Ok(stats)
}

/// Mark `ids` mirrored; the ids that were still there to mark.
/// Up to [`BATCH`] unmirrored events of orgs with `app_events` on, oldest
/// first. The flag is read in process, so the orgs are listed first (every org
/// with an app) and the flag-on ones go into the query: events of an org with
/// the flag off are never read, however many there are, and never stall the
/// orgs behind them.
async fn pending_events(db: &DatabaseConnection) -> Result<Vec<app_events::Model>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT org_id FROM apps",
        ))
        .await?;
    let mut enabled = Vec::new();
    for row in rows {
        let org: Uuid = row.try_get("", "org_id")?;
        if super::enabled_for(org) {
            enabled.push(org);
        }
    }
    if enabled.is_empty() {
        return Ok(Vec::new());
    }
    app_events::Entity::find()
        .filter(app_events::Column::MirroredAt.is_null())
        .filter(app_events::Column::RelayError.is_null())
        .filter(app_events::Column::OrgId.is_in(enabled))
        .order_by_asc(app_events::Column::RelayedAt)
        .order_by_asc(app_events::Column::Id)
        .limit(BATCH)
        .all(db)
        .await
}

async fn mark(db: &DatabaseConnection, ids: &[Uuid]) -> Result<HashSet<Uuid>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_events SET mirrored_at = now() WHERE id = ANY($1) RETURNING id",
            [ids.to_vec().into()],
        ))
        .await?;
    rows.iter().map(|r| r.try_get("", "id")).collect()
}

/// The log rows of `events`, one org and workspace's: the bodies read from
/// the org's store (one connection per store), the publishers' slugs from the
/// platform.
async fn logged(
    db: &DatabaseConnection,
    org_id: Uuid,
    events: &[app_events::Model],
) -> Result<Vec<Logged>, DbErr> {
    let app_ids: HashSet<Uuid> = events.iter().filter_map(|e| e.source_app_id).collect();
    let slugs: HashMap<Uuid, String> = apps::Entity::find()
        .filter(apps::Column::Id.is_in(app_ids))
        .all(db)
        .await?
        .into_iter()
        .map(|a| (a.id, a.slug))
        .collect();
    let mut bodies: HashMap<Uuid, serde_json::Value> = HashMap::new();
    // By the store each body is in: a sandbox on an OLTP branch of its own
    // keeps its events' bodies there, not on the staging branch.
    let mut stores = Stores::new(db);
    let mut by_store: HashMap<OutboxTarget, Vec<(String, Uuid)>> = HashMap::new();
    for e in events {
        let Some(target) = stores.of(e.source_app_id, &e.environment).await? else {
            continue;
        };
        by_store
            .entry(target)
            .or_default()
            .push((e.writer_role.clone(), e.id));
    }
    for (target, keys) in by_store {
        match read_bodies(db, org_id, &target, &keys).await {
            Ok(found) => bodies.extend(found),
            Err(e) => {
                tracing::debug!(%org_id, store = ?target, error = %e, "app events: bodies not read for the log")
            }
        }
    }
    Ok(events
        .iter()
        .map(|e| Logged {
            event_id: e.id,
            org_id: e.org_id,
            environment: e.environment.clone(),
            event_type: e.r#type.clone(),
            source: match e.source_app_id.and_then(|a| slugs.get(&a)) {
                Some(slug) => event_source(slug),
                None => TableRef::from_event_type(&e.r#type)
                    .map(|t| t.source())
                    .unwrap_or_default(),
            },
            key: e.key.clone(),
            published_at: e.published_at.with_timezone(&Utc),
            actor: e.actor.clone(),
            causation_id: e.causation_id,
            root_id: e.root_id,
            depth: e.depth,
            invocation_id: e.invocation_id,
            data: bodies.get(&e.id).map(|b| b.to_string()),
        })
        .collect())
}

/// The retention sweeper's part: prune each workspace's log of rows older than
/// [`KEEP_DAYS`], in orgs with `app_events` on. A failure is logged.
pub async fn prune_pass(db: &DatabaseConnection) {
    let tenants = match tenants::Entity::find()
        .filter(tenants::Column::Status.eq(TenantStatus::Active))
        .all(db)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "app events: the event log prune could not list workspaces");
            return;
        }
    };
    let log = log();
    for tenant in tenants {
        let Some(org) = org_of_workspace(db, tenant.workspace_id).await else {
            continue;
        };
        if !super::enabled_for(org) {
            continue;
        }
        match log.prune(db, tenant.workspace_id, KEEP_DAYS).await {
            Ok(()) | Err(SinkError::NotConfigured) => {}
            Err(SinkError::Failed(e)) => {
                tracing::warn!(workspace_id = %tenant.workspace_id, error = %e, "app events: the event log prune failed");
            }
        }
    }
}

async fn org_of_workspace(db: &DatabaseConnection, workspace_id: Uuid) -> Option<Uuid> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT org_id FROM workspaces WHERE id = $1",
            [workspace_id.into()],
        ))
        .await
        .ok()??;
    row.try_get("", "org_id").ok()
}
