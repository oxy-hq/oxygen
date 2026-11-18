//! Discard and purge, each audited.
//!
//! - **Discard** moves `pending` and `held` deliveries of events published up
//!   to `until` to `discarded`: they are never delivered. A queued task finds
//!   its delivery discarded and skips it.
//! - **Purge** hard-deletes events (their deliveries and attempts go with
//!   them) **and their bodies** in the org's OLTP store, bodies first, as
//!   retention does. When the store cannot be reached it deletes nothing and
//!   answers `503`, so no body is left with nothing pointing at it. At most
//!   [`PURGE_BATCH`] events a call.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_oltp::events::{OutboxKey, OutboxTarget, delete_bodies};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, Statement,
    TransactionTrait,
};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use super::reply::ApiError;
use crate::app_events::relay::relayed_store;
use crate::app_events::retention::has_a_store;

/// The audit action of a discard, and of a purge.
pub const DISCARDED: &str = "app.events_discarded";
pub const PURGED: &str = "app.events_purged";

/// The most events one purge deletes; `remaining` counts the rest.
const PURGE_BATCH: i64 = 5_000;

/// Which deliveries a discard takes.
pub(super) struct Discard {
    pub org_id: Uuid,
    pub event_type: Option<String>,
    /// As named, and resolved: `None` for an app the org does not have.
    pub to: Option<(String, Option<(Uuid, String)>)>,
    pub until: DateTime<Utc>,
}

/// Which events a purge takes.
pub(super) struct Purge {
    pub org_id: Uuid,
    pub event_type: String,
    pub key: Option<String>,
    pub before: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PurgeReply {
    count: u64,
    bodies_deleted: u64,
    remaining: u64,
}

#[derive(Debug, Serialize)]
pub(super) struct DiscardReply {
    count: u64,
}

const DISCARD_SQL: &str = "\
UPDATE app_event_deliveries d SET status = 'discarded', updated_at = now() \
  FROM app_events e \
 WHERE e.id = d.event_id AND e.org_id = $1 AND d.status IN ('pending', 'held') \
   AND e.published_at <= $2 \
   AND ($3::text IS NULL OR e.type = $3) \
   AND ($4::uuid IS NULL OR (d.app_id = $4 AND d.function_name = $5))";

pub(super) async fn discard(
    db: &DatabaseConnection,
    actor: &RequestActor,
    d: &Discard,
) -> Result<DiscardReply, ApiError> {
    let fail = |e| ApiError::db("discard deliveries", e);
    let (app, function) = match &d.to {
        Some((_, Some((app, function)))) => (Some(*app), Some(function.clone())),
        // A listener the org does not have has nothing to discard.
        Some((_, None)) => return Ok(DiscardReply { count: 0 }),
        None => (None, None),
    };
    let txn = db.begin().await.map_err(fail)?;
    let count = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            DISCARD_SQL,
            [
                d.org_id.into(),
                d.until.into(),
                d.event_type.clone().into(),
                app.into(),
                function.into(),
            ],
        ))
        .await
        .map_err(fail)?
        .rows_affected();
    let metadata = json!({
        "type": d.event_type,
        "to": d.to.as_ref().map(|(named, _)| named),
        "until": d.until,
        "count": count,
    });
    audited(&txn, actor, DISCARDED, d.org_id, metadata)
        .await
        .map_err(fail)?;
    txn.commit().await.map_err(fail)?;
    Ok(DiscardReply { count })
}

pub(super) async fn purge(
    db: &DatabaseConnection,
    actor: &RequestActor,
    p: &Purge,
) -> Result<PurgeReply, ApiError> {
    let fail = |e| ApiError::db("purge events", e);
    let txn = db.begin().await.map_err(fail)?;
    let (ids, bodies, total, workspaces) = lock_events(&txn, p).await.map_err(fail)?;
    let bodies_deleted = match delete_their_bodies(db, p.org_id, &bodies).await {
        Ok(deleted) => deleted,
        Err(refused) => {
            txn.rollback().await.map_err(fail)?;
            return Err(refused);
        }
    };
    // The event log's copy goes too, or what was published by mistake would
    // stay queryable there for 90 days.
    if let Err(refused) = delete_their_log_rows(db, &workspaces).await {
        txn.rollback().await.map_err(fail)?;
        return Err(refused);
    }
    let count = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM app_events WHERE id = ANY($1)",
            [ids.into()],
        ))
        .await
        .map_err(fail)?
        .rows_affected();
    let metadata = json!({
        "type": p.event_type,
        "key": p.key,
        "before": p.before,
        "count": count,
        "bodies_deleted": bodies_deleted,
    });
    audited(&txn, actor, PURGED, p.org_id, metadata)
        .await
        .map_err(fail)?;
    txn.commit().await.map_err(fail)?;
    Ok(PurgeReply {
        count,
        bodies_deleted,
        remaining: total.saturating_sub(count),
    })
}

/// The events `p` names, locked (a batch of them): their ids, their bodies'
/// keys by store, and how many match in all.
async fn lock_events(
    txn: &DatabaseTransaction,
    p: &Purge,
) -> Result<
    (
        Vec<Uuid>,
        HashMap<OutboxTarget, Vec<OutboxKey>>,
        u64,
        HashMap<Uuid, Vec<Uuid>>,
    ),
    DbErr,
> {
    let filter = "FROM app_events WHERE org_id = $1 AND type = $2 \
                  AND ($3::text IS NULL OR key = $3) \
                  AND ($4::timestamptz IS NULL OR published_at < $4)";
    let values = || -> Vec<sea_orm::Value> {
        vec![
            p.org_id.into(),
            p.event_type.clone().into(),
            p.key.clone().into(),
            p.before.into(),
        ]
    };
    let total: i64 = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!("SELECT count(*) AS n {filter}"),
            values(),
        ))
        .await?
        .map(|row| row.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let sql = format!(
        "SELECT id, environment, writer_role, workspace_id {filter} ORDER BY published_at, id \
         LIMIT {PURGE_BATCH} FOR UPDATE"
    );
    let rows = txn
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values(),
        ))
        .await?;
    let mut ids = Vec::with_capacity(rows.len());
    let mut bodies: HashMap<OutboxTarget, Vec<OutboxKey>> = HashMap::new();
    let mut workspaces: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for row in rows {
        let id: Uuid = row.try_get("", "id")?;
        let environment: String = row.try_get("", "environment")?;
        ids.push(id);
        workspaces
            .entry(row.try_get("", "workspace_id")?)
            .or_default()
            .push(id);
        if let Some(target) = relayed_store(&environment) {
            bodies.entry(target).or_default().push(OutboxKey {
                writer_role: row.try_get("", "writer_role")?,
                id,
            });
        }
    }
    Ok((ids, bodies, total as u64, workspaces))
}

/// Delete `events` (by workspace) from each workspace's event log. A
/// workspace with no Airhouse has none; one that cannot be reached refuses
/// the purge, like the org's store.
async fn delete_their_log_rows(
    db: &DatabaseConnection,
    workspaces: &HashMap<Uuid, Vec<Uuid>>,
) -> Result<(), ApiError> {
    use crate::app_events::mirror::{SinkError, log};
    let log = log();
    for (workspace_id, ids) in workspaces {
        match log.delete(db, *workspace_id, ids).await {
            Ok(()) | Err(SinkError::NotConfigured) => {}
            Err(SinkError::Failed(e)) => {
                tracing::warn!(%workspace_id, error = %e, "app events purge: the event log refused");
                return Err(ApiError::StoreUnreachable(format!(
                    "the workspace's Airhouse event log could not delete the events, so nothing \
                     was purged; retry once it can be reached ({e})"
                )));
            }
        }
    }
    Ok(())
}

/// Delete `bodies` from the org's store; how many. An org with no store has
/// none; a store that cannot be reached refuses the purge.
async fn delete_their_bodies(
    db: &DatabaseConnection,
    org_id: Uuid,
    bodies: &HashMap<OutboxTarget, Vec<OutboxKey>>,
) -> Result<u64, ApiError> {
    if bodies.is_empty()
        || !has_a_store(db, org_id)
            .await
            .map_err(|e| ApiError::db("find the org's store", e))?
    {
        return Ok(0);
    }
    let mut deleted = 0;
    for (target, keys) in bodies {
        deleted += delete_bodies(db, org_id, target, keys).await.map_err(|e| {
            tracing::warn!(%org_id, error = %e, "app events purge: the org's store refused");
            ApiError::StoreUnreachable(format!(
                "the org's OLTP store could not delete the events' bodies, so nothing was \
                 purged; retry once it can be reached ({e})"
            ))
        })?;
    }
    Ok(deleted)
}

async fn audited(
    txn: &DatabaseTransaction,
    actor: &RequestActor,
    action: &'static str,
    org_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), DbErr> {
    let entry = AuditEntry::for_request(actor, action)
        .org(org_id)
        .metadata(metadata);
    audit::record_in_txn(txn, entry).await.map(|_| ())
}
