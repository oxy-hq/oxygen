//! Which of a batch of event bodies a store still keeps, in one query per
//! chunk: replay's check (`app_events::api::replay`), which used to read
//! every candidate's body, one connection each (ruling R45(5)).

use std::collections::HashSet;

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{BY_KEY, EventsError, OutboxKey, OutboxTarget, key_arrays, store};
use crate::platform::EVENTS_OUTBOX_TABLE;

/// The most keys one statement names.
pub const EXISTING_CHUNK: usize = 500;

/// Of `keys`, those whose body `target`'s outbox log still holds — waiting or
/// relayed — read as the owner on one short connection, one statement per
/// [`EXISTING_CHUNK`] keys. Reads no body: only that a row is there. A store
/// the org no longer has (a branch gone with its sandbox) holds none, and is
/// not connected to when `keys` is empty.
pub async fn existing_bodies(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    keys: &[OutboxKey],
) -> Result<HashSet<OutboxKey>, EventsError> {
    let mut found = HashSet::new();
    if keys.is_empty() {
        return Ok(found);
    }
    // Like a read, no reconcile: a body is only in a store the outbox reached.
    let Some(client) = super::store::owner_client(control, org_id, target, false).await? else {
        return Ok(found);
    };
    for chunk in keys.chunks(EXISTING_CHUNK) {
        let (roles, ids) = key_arrays(chunk);
        let rows = client
            .query(
                &format!("SELECT writer_role, id FROM {EVENTS_OUTBOX_TABLE} WHERE {BY_KEY}"),
                &[&roles, &ids],
            )
            .await
            .map_err(|e| store("check which event bodies are kept", &e))?;
        found.extend(rows.iter().map(|row| OutboxKey {
            writer_role: row.get(0),
            id: row.get(1),
        }));
    }
    Ok(found)
}
