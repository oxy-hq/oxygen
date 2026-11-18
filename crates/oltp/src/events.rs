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
//! **Each environment's events wait in its own store** ([`OutboxTarget`]):
//! production's in the tenant database, staging's and every sandbox's in the
//! org's OLTP staging branch (`events::branch`), each with its own grants
//! (`events::grant`). A branch cut before the outbox existed gets it on its first
//! grant or claim.
//!
//! Design: `internal-docs/2026-11-17-app-events-design.md` §6.2.

use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use crate::connect::pg_detail;
use crate::platform::EVENTS_OUTBOX_TABLE;

mod branch;
mod grant;
mod platform;
mod store;

pub use grant::{ensure_publishable, forget_publishable};
pub use platform::{PLATFORM_WRITER, PlatformBody, read_bodies, write_platform_body};

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
    /// [`read_body`] found the body, but it cannot be decoded into the JSON
    /// Rust holds (a number past `f64`, say): reading it again never helps.
    #[error("events store: decode the event body: {0}")]
    Undecodable(String),
    /// A grant or a claim on [`OutboxTarget::Branch`] in an org with no such
    /// branch: nothing was ever published there (a publish without one is
    /// held), or it went with the org. A read or a delete there finds
    /// nothing instead.
    #[error("events store: org {org_id} has no OLTP branch hosting `{environment}`")]
    NoBranch { org_id: Uuid, environment: String },
}

/// Which store's outbox to read: each environment's events wait in that
/// environment's OLTP store.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutboxTarget {
    /// The org's production tenant database.
    Production,
    /// The OLTP branch hosting the named non-production environment's events:
    /// the org's staging branch, for staging and for every sandbox on a
    /// schema there (`events::branch`).
    Branch(String),
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
    let client = store::required_client(control, org_id, target, true).await?;
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
/// decides what a missing body means. Waiting and relayed rows alike. A body
/// that is there but cannot be decoded is [`EventsError::Undecodable`].
///
/// A read only: it never reconciles the store's platform schema. A body is
/// only ever in a store the outbox step already reached. A branch the org no
/// longer has holds no body: `None`.
pub async fn read_body(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    writer_role: &str,
    id: Uuid,
) -> Result<Option<Value>, EventsError> {
    let Some(client) = store::owner_client(control, org_id, target, false).await? else {
        return Ok(None);
    };
    let row = client
        .query_opt(
            &format!("SELECT data FROM {EVENTS_OUTBOX_TABLE} WHERE writer_role = $1 AND id = $2"),
            &[&writer_role, &id],
        )
        .await
        .map_err(|e| store("read the event body", &e))?;
    row.map(|r| r.try_get::<_, Value>(0))
        .transpose()
        .map_err(|e| EventsError::Undecodable(pg_detail(&e)))
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
    // Like a read, no reconcile: a body is only in a store the outbox reached,
    // and a branch that is gone took its bodies with it.
    let Some(client) = store::owner_client(control, org_id, target, false).await? else {
        return Ok(0);
    };
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
    let Some(client) = store::owner_client(control, org_id, target, false).await? else {
        return Ok(0);
    };
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
    use crate::platform::EVENTS_PUBLISH_FUNCTION;

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

    /// Its own variant, so a caller tells "never readable" from "not reached"
    /// without matching on text; the message keeps the old wording.
    #[test]
    fn an_undecodable_body_is_its_own_error() {
        let err = EventsError::Undecodable("number out of range".into());
        assert_eq!(
            err.to_string(),
            "events store: decode the event body: number out of range"
        );
        assert!(!matches!(err, EventsError::Store(_)));
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
