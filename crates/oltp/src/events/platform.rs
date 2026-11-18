//! A body the platform publishes itself, not an app: the rows a table change
//! carries (app events design §7).
//!
//! An app's event body waits in `oxy_events.outbox` until the relay moves its
//! metadata out. A table event's metadata is stored by the tracker in the same
//! platform transaction that moves the table's cursor, so the body is written
//! first, already relayed: the relay never claims it, and retention deletes it
//! with its event like any other body. The writer role is [`PLATFORM_WRITER`],
//! which no app's writer role can equal (`app_<slug>`), so the key
//! `(writer_role, id)` never meets an app's.
//!
//! The write is idempotent on the id: a pass that wrote the body and then lost
//! the platform transaction writes the same body again, a no-op.

use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use super::{EVENTS_OUTBOX_TABLE, EventsError, OutboxTarget, store};

/// The writer role of every body the platform publishes.
pub const PLATFORM_WRITER: &str = "oxy_table_watch";

/// One platform body: the event's id, environment, type, key and `data`.
pub struct PlatformBody<'a> {
    pub id: Uuid,
    pub environment: &'a str,
    pub name: &'a str,
    pub key: &'a str,
    pub data: &'a Value,
    pub created_at: DateTime<Utc>,
}

/// Write `body` to `target`'s outbox log as the owner, marked relayed.
pub async fn write_platform_body(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    body: &PlatformBody<'_>,
) -> Result<(), EventsError> {
    let client = store::required_client(control, org_id, target, true).await?;
    client
        .execute(
            &format!(
                "INSERT INTO {EVENTS_OUTBOX_TABLE} \
                   (id, writer_role, environment, name, key, data, root_id, depth, \
                    delay_seconds, created_at, relayed_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $1, 0, 0, $7, now()) \
                 ON CONFLICT (writer_role, id) DO NOTHING"
            ),
            &[
                &body.id,
                &PLATFORM_WRITER,
                &body.environment,
                &body.name,
                &body.key,
                body.data,
                &body.created_at,
            ],
        )
        .await
        .map_err(|e| super::store(&format!("write the {} body", body.name), &e))?;
    Ok(())
}

/// The bodies of `ids` written by `writer_role`, from `target`'s outbox log, as
/// the owner on one connection: for the event log mirror, which copies a
/// batch. An id with no row is missing from the map; a branch the org does not
/// have holds none, and a body that cannot be decoded is left out.
pub async fn read_bodies(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    keys: &[(String, Uuid)],
) -> Result<std::collections::HashMap<Uuid, Value>, EventsError> {
    if keys.is_empty() {
        return Ok(Default::default());
    }
    let Some(client) = store::owner_client(control, org_id, target, false).await? else {
        return Ok(Default::default());
    };
    let (roles, ids): (Vec<&str>, Vec<Uuid>) = keys.iter().map(|(r, i)| (r.as_str(), *i)).unzip();
    let rows = client
        .query(
            &format!(
                "SELECT id, data FROM {EVENTS_OUTBOX_TABLE} \
                  WHERE (writer_role, id) IN (SELECT * FROM unnest($1::text[], $2::uuid[]))"
            ),
            &[&roles, &ids],
        )
        .await
        .map_err(|e| super::store("read event bodies", &e))?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some((
                r.try_get::<_, Uuid>(0).ok()?,
                r.try_get::<_, Value>(1).ok()?,
            ))
        })
        .collect())
}
