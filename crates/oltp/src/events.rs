//! The events outbox, in the org's own database.
//!
//! An app publishes an event inside its own OLTP transaction, so the event
//! commits or rolls back with the app's write: a transactional outbox. The
//! outbox is `oxy_events.outbox` in the org's tenant database (platform step
//! 5), and an app reaches it only as its own **writer role**, through the
//! definer function `oxy_events.publish(...)` ([`PUBLISH_SQL`]). The function
//! records `session_user`, so one app cannot publish as another. A writer holds
//! `USAGE` on the schema and `EXECUTE` on that function, granted lazily by
//! [`ensure_publishable`], and nothing on the table: it can neither read other
//! apps' events nor write a row by hand.
//!
//! The relay reads the outbox as the tenant's **owner**: [`claim`] locks a
//! batch of waiting rows (`relayed_at IS NULL`) with `FOR UPDATE SKIP
//! LOCKED` and holds it until [`OutboxClaim::mark_relayed_and_commit`] or
//! [`OutboxClaim::release`], so two relays never hold the same row. A claim
//! reads each row's routing metadata and its payload's size, never the
//! payload.
//!
//! **The body never leaves the org's store** (data placement: the platform
//! routes on an event, the org keeps its content). A relayed row is marked,
//! not deleted: it is the body's log. A delivery reads one body with
//! [`read_body`], and retention deletes the bodies of the events it deletes
//! with [`delete_bodies`]. Each opens one short owner connection, as a claim
//! does.
//!
//! An org with no tenant has no outbox, so preparing to publish fails with
//! [`EventsError::NotProvisioned`], whose message names the operator step.
//! Provisioning stays manual (`internal-docs/per-org-oltp-postgres.md`).
//!
//! Design: `internal-docs/2026-11-17-app-events-design.md` §6.2.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use crate::connect::pg_detail;
use crate::entity::tenants::{self as oltp_tenants, TenantStatus};
use crate::migrator::{MigrateError, owner_dsn, tenant_for_org};
use crate::platform::{EVENTS_OUTBOX_TABLE, EVENTS_PUBLISH_FUNCTION, EVENTS_SCHEMA};
use crate::provisioner::reconcile_platform_schema_as_owner;
use crate::schema::{quote_ident, validate_name};
use crate::sql::PgSqlExecutor;

/// One publish, run as the app's writer role inside the app's transaction.
///
/// Parameters, in order: id, environment, name, key, data, schema revision,
/// actor, causation id, root id, depth, invocation id, delay seconds. Returns
/// the id. A writer repeating one of its own ids is `ON CONFLICT DO NOTHING`,
/// never an error, so a retried publish cannot abort the app's transaction;
/// the same id from another writer is another row (the key is
/// `(writer_role, id)`).
pub const PUBLISH_SQL: &str = "SELECT oxy_events.publish($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)";

#[derive(Debug, thiserror::Error)]
pub enum EventsError {
    /// The org has no active OLTP tenant, so it has no outbox.
    #[error(
        "EventsStoreMissing: this org's OLTP store is not provisioned; run `oxyc oltp provision --org <org>`"
    )]
    NotProvisioned { org_id: Uuid },
    /// Opening, reconciling, granting or querying the store failed.
    #[error("events store: {0}")]
    Store(String),
}

/// Which store's outbox to read: each environment's events wait in that
/// environment's OLTP store.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutboxTarget {
    /// The org's production tenant database.
    Production,
}

/// One outbox row's key: a writer's ids are its own, so a row is named by
/// `(writer_role, id)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutboxKey {
    pub writer_role: String,
    pub id: Uuid,
}

/// One waiting outbox row, as the relay reads it: the event's routing
/// metadata and its payload's size. The payload stays in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxRow {
    pub id: Uuid,
    /// The writer role that called `oxy_events.publish` — recorded by the
    /// function, never supplied by the caller.
    pub writer_role: String,
    pub environment: String,
    pub name: String,
    pub key: String,
    /// The payload's size as Postgres writes it, `octet_length(data::text)`:
    /// computed in the store, so the relay never reads the body.
    pub data_bytes: i32,
    pub schema_revision: Option<Uuid>,
    pub actor: Option<String>,
    pub causation_id: Option<Uuid>,
    pub root_id: Uuid,
    pub depth: i32,
    pub invocation_id: Option<Uuid>,
    pub delay_seconds: i32,
    pub created_at: DateTime<Utc>,
}

impl OutboxRow {
    pub fn key(&self) -> OutboxKey {
        OutboxKey {
            writer_role: self.writer_role.clone(),
            id: self.id,
        }
    }
}

/// A batch of waiting outbox rows, locked by an open owner transaction.
///
/// The locks are held until [`Self::mark_relayed_and_commit`] or
/// [`Self::release`]. Dropping a claim instead closes its connection, which
/// rolls the transaction back: the rows stay waiting for the next claim.
#[must_use = "a claim holds its rows locked until mark_relayed_and_commit or release"]
pub struct OutboxClaim {
    client: tokio_postgres::Client,
    rows: Vec<OutboxRow>,
    /// How long the oldest row the claim left waiting — held by another
    /// claim, or past its limit — had waited when the claim was taken, by the
    /// store's clock (`now() - created_at`), read in the claim statement's own
    /// snapshot: never a row committed after it. `None`: it left none.
    oldest_left: Option<std::time::Duration>,
}

impl OutboxClaim {
    pub fn rows(&self) -> &[OutboxRow] {
        &self.rows
    }

    /// Whether the claim left rows waiting: ones another claim holds (this
    /// claim skipped them, `SKIP LOCKED`) or ones past its limit. Counted in
    /// the claim statement's own snapshot, without skipping locks, so a row
    /// committed after the claim never counts — under steady publishing
    /// nearly every pass would see one, and a healthy outbox would never
    /// drain. A pass that left rows must not mark the outbox drained: the
    /// rows another pass holds may yet be left by it. A plain comparison:
    /// nothing is sent to the store, so it cannot fail the claim.
    pub fn others_waiting(&self) -> bool {
        self.oldest_left.is_some()
    }

    /// How long the oldest row the claim left waiting ([`Self::others_waiting`])
    /// had waited when the claim was taken, measured in the store as
    /// `now() - created_at` — a duration, so it is set against the platform's
    /// clock without the two clocks ever being compared. `None` when it left
    /// none. `created_at` is when the publishing transaction began, so a row
    /// whose transaction ran long reads as older than it is: never younger.
    pub fn oldest_left_waiting(&self) -> Option<std::time::Duration> {
        self.oldest_left
    }

    /// Mark the claimed rows relayed (`relayed_at = now()`) and commit: they
    /// leave the claim set and stay as their bodies' log. A row refused with a
    /// relay error is marked too. The rows in `discard` — ones no stored event
    /// points at, so nothing will ever read their body — are deleted instead.
    /// Each row is addressed by its key, `(writer_role, id)`: another
    /// writer's row with the same id is not this claim's.
    ///
    /// Should marking or the commit fail, the transaction is rolled back there
    /// and then (an explicit `ROLLBACK`, as [`Self::release`] sends), so the
    /// rows are free for the next claim before the caller reopens the mark.
    pub async fn mark_relayed_and_commit(self, discard: &[OutboxKey]) -> Result<(), EventsError> {
        let committed = match self.mark_relayed(discard).await {
            Ok(()) => self
                .client
                .batch_execute("COMMIT")
                .await
                .map_err(|e| store("commit the outbox claim", &e)),
            Err(e) => Err(e),
        };
        if committed.is_err() {
            // Moot after a failed COMMIT, which ended the transaction; never
            // allowed to mask the error that brought it here.
            let _ = self.client.batch_execute("ROLLBACK").await;
        }
        committed
    }

    /// The marks and deletes of [`Self::mark_relayed_and_commit`], uncommitted.
    async fn mark_relayed(&self, discard: &[OutboxKey]) -> Result<(), EventsError> {
        let (drop, keep): (Vec<OutboxKey>, Vec<OutboxKey>) = self
            .rows
            .iter()
            .map(OutboxRow::key)
            .partition(|k| discard.contains(k));
        if !keep.is_empty() {
            let (roles, ids) = key_arrays(&keep);
            self.client
                .execute(
                    &format!(
                        "UPDATE {EVENTS_OUTBOX_TABLE} SET relayed_at = now() \
                         WHERE relayed_at IS NULL AND {BY_KEY}"
                    ),
                    &[&roles, &ids],
                )
                .await
                .map_err(|e| store("mark outbox rows relayed", &e))?;
        }
        if !drop.is_empty() {
            let (roles, ids) = key_arrays(&drop);
            self.client
                .execute(
                    &format!("DELETE FROM {EVENTS_OUTBOX_TABLE} WHERE {BY_KEY}"),
                    &[&roles, &ids],
                )
                .await
                .map_err(|e| store("delete discarded outbox rows", &e))?;
        }
        Ok(())
    }

    /// Roll back without marking: the rows stay waiting for the next claim.
    pub async fn release(self) -> Result<(), EventsError> {
        self.client
            .batch_execute("ROLLBACK")
            .await
            .map_err(|e| store("release the outbox claim", &e))
    }
}

/// Every column a claim reads: the metadata, and the payload's size computed
/// in the store — never the payload.
const OUTBOX_COLUMNS: &str = "id, writer_role, environment, name, key, octet_length(data::text), \
     schema_revision, actor, causation_id, root_id, depth, invocation_id, delay_seconds, created_at";

/// The rows whose keys are `$1` / `$2`, parallel arrays of writer roles and
/// ids ([`key_arrays`]).
const BY_KEY: &str = "(writer_role, id) IN (SELECT * FROM unnest($1::text[], $2::uuid[]))";

/// [`BY_KEY`]'s parameters.
fn key_arrays(keys: &[OutboxKey]) -> (Vec<&str>, Vec<Uuid>) {
    keys.iter().map(|k| (k.writer_role.as_str(), k.id)).unzip()
}

fn store(what: &str, e: &tokio_postgres::Error) -> EventsError {
    EventsError::Store(format!("{what}: {}", pg_detail(e)))
}

/// The org's tenant row, refusing one that cannot be used.
async fn active_tenant(
    control: &DatabaseConnection,
    org_id: Uuid,
) -> Result<oltp_tenants::Model, EventsError> {
    let tenant = tenant_for_org(control, org_id).await.map_err(|e| match e {
        MigrateError::NotProvisioned(org_id) => EventsError::NotProvisioned { org_id },
        other => EventsError::Store(format!("find the org's OLTP store: {other}")),
    })?;
    // A failed or deleting provision: the operator's remedy is the same.
    if tenant.status != TenantStatus::Active {
        return Err(EventsError::NotProvisioned { org_id });
    }
    Ok(tenant)
}

/// [`active_tenant`], brought current so a tenant provisioned before the
/// outbox existed gets it on first use. For a publish's grant and a claim,
/// which may be the first to touch the outbox; never for a read of what is
/// already there.
async fn current_tenant(
    control: &DatabaseConnection,
    org_id: Uuid,
) -> Result<oltp_tenants::Model, EventsError> {
    let tenant = active_tenant(control, org_id).await?;
    reconcile_platform_schema_as_owner(control, &PgSqlExecutor, &tenant)
        .await
        .map_err(|e| EventsError::Store(format!("bring the org's OLTP store up to date: {e}")))
}

async fn owner_client(
    tenant: &oltp_tenants::Model,
    target: &OutboxTarget,
) -> Result<tokio_postgres::Client, EventsError> {
    let dsn = match target {
        OutboxTarget::Production => owner_dsn(tenant)
            .map_err(|e| EventsError::Store(format!("open the org's OLTP store: {e}")))?,
    };
    crate::connect::connect(&dsn, "oxy_events")
        .await
        .map_err(|e| store("open the org's OLTP store", &e))
}

/// `(org, writer role)` pairs already granted in this process.
fn publishable() -> MutexGuard<'static, HashSet<(Uuid, String)>> {
    static PUBLISHABLE: OnceLock<Mutex<HashSet<(Uuid, String)>>> = OnceLock::new();
    PUBLISHABLE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Let `writer_role` — the role's real name, qualified on a shared-cluster
/// provider — call `oxy_events.publish` in the org's store. Asked before an
/// app's first publish, the same lazy way `ctx.notify` prepares its inbox:
/// brings the tenant's Oxy-owned objects current, then grants `USAGE` on the
/// schema and `EXECUTE` on the function, through the owner.
///
/// Remembered per `(org, role)` for the life of the process; a failure is not,
/// so the next publish asks again.
pub async fn ensure_publishable(
    control: &DatabaseConnection,
    org_id: Uuid,
    writer_role: &str,
) -> Result<(), EventsError> {
    let key = (org_id, writer_role.to_string());
    let known = publishable().contains(&key);
    if known {
        return Ok(());
    }
    let grant = grant_publish_sql(writer_role)?;
    let tenant = current_tenant(control, org_id).await?;
    let client = owner_client(&tenant, &OutboxTarget::Production).await?;
    client
        .batch_execute(&grant)
        .await
        .map_err(|e| store(&format!("let {writer_role} publish events"), &e))?;
    publishable().insert(key);
    Ok(())
}

/// Drop what [`ensure_publishable`] remembers for `(org_id, writer_role)`, so
/// the next call grants again. For a publish refused `42501` (insufficient
/// privilege) though the pair was remembered: the grants went with the role
/// (a re-mint's `DROP OWNED`) or the tenant, and this process cannot tell.
pub fn forget_publishable(org_id: Uuid, writer_role: &str) {
    publishable().remove(&(org_id, writer_role.to_string()));
}

/// The grants [`ensure_publishable`] makes, as one implicit transaction.
///
/// Serialised on an advisory lock: two sessions granting on one catalog row at
/// once can fail with `tuple concurrently updated`, and every replica's first
/// publish for an app grants.
fn grant_publish_sql(writer_role: &str) -> Result<String, EventsError> {
    validate_name(writer_role).map_err(|e| EventsError::Store(e.to_string()))?;
    let role = quote_ident(writer_role);
    Ok(format!(
        "SELECT pg_advisory_xact_lock(hashtext('{EVENTS_PUBLISH_FUNCTION}')); \
         GRANT USAGE ON SCHEMA {EVENTS_SCHEMA} TO {role}; \
         GRANT EXECUTE ON FUNCTION {EVENTS_PUBLISH_FUNCTION} TO {role}"
    ))
}

/// Lock up to `limit` of the oldest waiting rows no other claim holds, in an
/// owner transaction the returned claim keeps open, and, in the same
/// statement and so the same snapshot, find the oldest waiting row it did not
/// take and its age ([`OutboxClaim::others_waiting`],
/// [`OutboxClaim::oldest_left_waiting`]). Reads no payload.
pub async fn claim(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    limit: i64,
) -> Result<OutboxClaim, EventsError> {
    let tenant = current_tenant(control, org_id).await?;
    let client = owner_client(&tenant, target).await?;
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|e| store("begin the outbox claim", &e))?;
    // One statement, one snapshot: the oldest row left is found beside the
    // claim, so one another claim holds counts (a plain read never waits on
    // a row lock) and none committed after it does. It is looked up oldest
    // first through the partial index of waiting rows, stopping at the first
    // the claim did not take, so a pass reads its batch and one row more, not
    // the whole backlog (no `count(*)`, which would scan every waiting row on
    // every pass while a backlog drains). The scalar subquery is one row,
    // null when nothing was left, and the outer join keeps it when nothing is
    // claimed.
    let rows = client
        .query(
            &format!(
                "WITH claimed AS MATERIALIZED ( \
                   SELECT {OUTBOX_COLUMNS} FROM {EVENTS_OUTBOX_TABLE} WHERE relayed_at IS NULL \
                    ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT $1), \
                 oldest_left AS ( \
                   SELECT (SELECT o.created_at FROM {EVENTS_OUTBOX_TABLE} o \
                            WHERE o.relayed_at IS NULL \
                              AND (o.writer_role, o.id) NOT IN (SELECT writer_role, id FROM claimed) \
                            ORDER BY o.created_at LIMIT 1) AS created_at) \
                 SELECT claimed.*, EXTRACT(EPOCH FROM now() - l.created_at)::float8 AS oldest_left \
                   FROM oldest_left l LEFT JOIN claimed ON true \
                  ORDER BY claimed.created_at"
            ),
            &[&limit],
        )
        .await
        .map_err(|e| store("claim outbox rows", &e))?;
    let oldest_left = rows
        .first()
        .and_then(|r| r.get::<_, Option<f64>>(14))
        .map(|secs| std::time::Duration::from_secs_f64(secs.max(0.0)));
    let rows = rows
        .iter()
        // The one row of an empty claim: the oldest row left, and no claimed row.
        .filter(|r| r.get::<_, Option<Uuid>>(0).is_some())
        .map(|r| OutboxRow {
            id: r.get(0),
            writer_role: r.get(1),
            environment: r.get(2),
            name: r.get(3),
            key: r.get(4),
            data_bytes: r.get(5),
            schema_revision: r.get(6),
            actor: r.get(7),
            causation_id: r.get(8),
            root_id: r.get(9),
            depth: r.get(10),
            invocation_id: r.get(11),
            delay_seconds: r.get(12),
            created_at: r.get(13),
        })
        .collect();
    Ok(OutboxClaim {
        client,
        rows,
        oldest_left,
    })
}

/// The body of the event `writer_role` published under `id`, read from
/// `target`'s outbox log as the owner on one short connection. `None` when
/// the row is gone (retention deleted it, or it never existed): the caller
/// decides what a missing body means. Waiting and relayed rows alike.
///
/// A read only: it never reconciles the store's platform schema. A body is
/// only ever in a store the outbox step already reached.
pub async fn read_body(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    writer_role: &str,
    id: Uuid,
) -> Result<Option<Value>, EventsError> {
    let tenant = active_tenant(control, org_id).await?;
    let client = owner_client(&tenant, target).await?;
    let row = client
        .query_opt(
            &format!("SELECT data FROM {EVENTS_OUTBOX_TABLE} WHERE writer_role = $1 AND id = $2"),
            &[&writer_role, &id],
        )
        .await
        .map_err(|e| store("read the event body", &e))?;
    row.map(|r| r.try_get::<_, Value>(0))
        .transpose()
        .map_err(|e| store("decode the event body", &e))
}

/// Delete the bodies of `keys` from `target`'s outbox log, as the owner on
/// one short connection; the rows deleted. For retention, which deletes an
/// event's body before its metadata, so no body outlives the event that
/// points at it. A key with no row is no error.
pub async fn delete_bodies(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    keys: &[OutboxKey],
) -> Result<u64, EventsError> {
    if keys.is_empty() {
        return Ok(0);
    }
    // Like a read, no reconcile: a body is only in a store the outbox reached.
    let tenant = active_tenant(control, org_id).await?;
    let client = owner_client(&tenant, target).await?;
    let (roles, ids) = key_arrays(keys);
    client
        .execute(
            &format!("DELETE FROM {EVENTS_OUTBOX_TABLE} WHERE {BY_KEY}"),
            &[&roles, &ids],
        )
        .await
        .map_err(|e| store("delete event bodies", &e))
}

/// Delete up to `limit` of the oldest rows of `target`'s outbox log relayed
/// more than `days` ago (the store's own clock, which stamped `relayed_at`),
/// as the owner on one short connection; the rows deleted. For retention,
/// which deletes bodies through their events: an app's deletion takes its
/// events (`ON DELETE CASCADE`) and leaves their bodies here, with nothing in
/// the platform pointing at them. A row still waiting (`relayed_at IS NULL`)
/// is never touched, however old.
pub async fn delete_relayed_before(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    days: i32,
    limit: i64,
) -> Result<u64, EventsError> {
    // Like a read, no reconcile: a body is only in a store the outbox reached.
    let tenant = active_tenant(control, org_id).await?;
    let client = owner_client(&tenant, target).await?;
    client
        .execute(
            &format!(
                "DELETE FROM {EVENTS_OUTBOX_TABLE} WHERE (writer_role, id) IN ( \
                   SELECT writer_role, id FROM {EVENTS_OUTBOX_TABLE} \
                    WHERE relayed_at IS NOT NULL \
                      AND relayed_at < now() - make_interval(days => $1) \
                    ORDER BY relayed_at LIMIT $2)"
            ),
            &[&days, &limit],
        )
        .await
        .map_err(|e| store("delete old relayed bodies", &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_store_says_how_to_get_one() {
        let msg = EventsError::NotProvisioned {
            org_id: Uuid::nil(),
        }
        .to_string();
        assert_eq!(
            msg,
            "EventsStoreMissing: this org's OLTP store is not provisioned; run \
             `oxyc oltp provision --org <org>`"
        );
    }

    #[test]
    fn a_claim_reads_the_payloads_size_never_the_payload() {
        let columns: Vec<&str> = OUTBOX_COLUMNS.split(',').map(str::trim).collect();
        assert!(!columns.contains(&"data"), "{OUTBOX_COLUMNS}");
        assert!(
            columns.contains(&"octet_length(data::text)"),
            "{OUTBOX_COLUMNS}"
        );
    }

    #[test]
    fn forgetting_drops_that_pair_alone() {
        let org = Uuid::new_v4();
        let other_org = Uuid::new_v4();
        for pair in [
            (org, "app_a_rw".to_string()),
            (org, "app_b_rw".to_string()),
            (other_org, "app_a_rw".to_string()),
        ] {
            publishable().insert(pair);
        }

        forget_publishable(org, "app_a_rw");

        let cache = publishable();
        assert!(!cache.contains(&(org, "app_a_rw".to_string())));
        assert!(cache.contains(&(org, "app_b_rw".to_string())));
        assert!(cache.contains(&(other_org, "app_a_rw".to_string())));
    }

    #[test]
    fn the_grant_reaches_the_function_and_the_schema_only() {
        let sql = grant_publish_sql("app_bookings_rw_1a2b3c4d").unwrap();
        assert!(
            sql.contains(r#"GRANT USAGE ON SCHEMA oxy_events TO "app_bookings_rw_1a2b3c4d""#),
            "{sql}"
        );
        assert!(
            sql.contains(&format!(
                r#"GRANT EXECUTE ON FUNCTION {EVENTS_PUBLISH_FUNCTION} TO "app_bookings_rw_1a2b3c4d""#
            )),
            "{sql}"
        );
        assert!(!sql.contains("ON TABLE"), "never the table: {sql}");
        assert!(
            !sql.contains(" ALL "),
            "never ALL PRIVILEGES or ON ALL: {sql}"
        );
    }

    #[test]
    fn a_role_name_is_validated_before_any_sql() {
        assert!(grant_publish_sql("app_x\"; DROP SCHEMA oxy_events; --").is_err());
        assert!(grant_publish_sql("").is_err());
    }

    #[test]
    fn publish_calls_the_function_the_step_creates() {
        assert!(EVENTS_PUBLISH_FUNCTION.starts_with("oxy_events.publish("));
        assert!(PUBLISH_SQL.starts_with("SELECT oxy_events.publish("));
        assert_eq!(
            PUBLISH_SQL.matches('$').count(),
            EVENTS_PUBLISH_FUNCTION.matches(',').count() + 1,
            "one parameter per argument"
        );
    }
}
