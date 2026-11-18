//! The event a delivery hands its listener, rebuilt from the delivery's own
//! records (app events design §6.1, §6.6): its `app_events` row, the apps
//! that publish and listen, and the schema revision. Never from the task's
//! copy of the envelope, which the relay wrote once.
//!
//! The body is not in any of those: the platform holds an event's routing
//! metadata only, and the body stays in the org's OLTP outbox log under
//! `(writer_role, id)` (§6.2). [`load_event_data`] is the one place a delivery
//! reads it, from the org's store, at run time.

use chrono::Utc;
use entity::{app_event_deliveries as deliveries, app_event_schemas, app_events, apps};
use oxy_custom_apps::events::OxyEvent;
use oxy_custom_apps::events::table::{TableRef, is_table_event_type};
use oxy_oltp::events::{EventsError, OutboxTarget, read_body};
use sea_orm::{DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait};
use serde_json::Value;
use uuid::Uuid;

use super::stores::store_of;

/// What a delivery's envelope is built from: its event, the slugs of its
/// listener's and its publisher's apps, and the revision it was published
/// against.
pub(super) struct Records {
    pub(super) event: app_events::Model,
    /// The listener app's slug.
    pub(super) listener: String,
    source: String,
    revision: Option<i32>,
}

impl Records {
    /// `None` when one is gone. Deleting the event or either app deletes the
    /// delivery (cascade), so that is a delivery about to go: a skip, not a
    /// guess.
    pub(super) async fn read(
        txn: &DatabaseTransaction,
        delivery: &deliveries::Model,
    ) -> Result<Option<Self>, DbErr> {
        let Some(event) = app_events::Entity::find_by_id(delivery.event_id)
            .one(txn)
            .await?
        else {
            return Ok(None);
        };
        let Some(listener) = app_slug(txn, delivery.app_id).await? else {
            return Ok(None);
        };
        // A table event's publisher is the platform, which `source` names by
        // the table; an app's event by the app.
        let source = match event.source_app_id {
            Some(app) => match app_slug(txn, app).await? {
                Some(slug) => event_source(&slug),
                None => return Ok(None),
            },
            None => table_source(&event.r#type),
        };
        let revision = match event.schema_revision_id {
            Some(id) => app_event_schemas::Entity::find_by_id(id)
                .one(txn)
                .await?
                .map(|s| s.revision),
            None => None,
        };
        Ok(Some(Self {
            event,
            listener,
            source,
            revision,
        }))
    }

    /// The envelope a listener receives, as the relay built it
    /// (`relay::read_row`), carrying `data`, which the relay's copy leaves
    /// null.
    pub(super) fn envelope(self, data: Value) -> OxyEvent {
        let event = self.event;
        OxyEvent {
            specversion: "1.0".to_string(),
            id: event.id,
            dataschema: event_dataschema(&event.r#type, self.revision),
            source: self.source,
            time: event.published_at.with_timezone(&Utc),
            data,
            key: event.key,
            environment: event.environment,
            invocation: event.invocation_id,
            actor: event.actor,
            causationid: event.causation_id,
            rootid: event.root_id,
            depth: event.depth,
            event_type: event.r#type,
        }
    }
}

/// Why [`load_event_data`] could not read a body that should be there (a body
/// that is gone is `Ok(None)`). Neither message ever holds the body.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BodyError {
    /// No attempt will ever read it: the delivery is dead, with this reason.
    #[error("{0}")]
    Permanent(String),
    /// Reading it failed this time (the org's store could not be reached,
    /// say): the attempt is an error, and the queue retries.
    #[error("could not read the event's body: {0}")]
    Transient(String),
}

impl BodyError {
    /// What a failed read of `event_id`'s body from the org's store means.
    /// A body there that Rust cannot decode (a number past `f64`) is
    /// permanent; a store not reached or a tenant not active may pass. Every
    /// variant is named, so a new one is a decision, never a silent retry.
    fn from_store(event_id: Uuid, e: EventsError) -> Self {
        match e {
            EventsError::Undecodable(why) => Self::Permanent(format!(
                "the body of event {event_id} cannot be decoded ({why}); no attempt can read it"
            )),
            e @ (EventsError::Store(_) | EventsError::NotProvisioned { .. }) => {
                Self::Transient(e.to_string())
            }
            // A read finds nothing in a branch that is gone (`Ok(None)`), so
            // this is never a read's; were it one, the body went with it.
            e @ EventsError::NoBranch { .. } => Self::Permanent(format!(
                "the body of event {event_id} went with its store ({e}); no attempt can read it"
            )),
        }
    }
}

/// The store `event`'s body is in: the outbox log of the store its
/// publisher's environment's events wait in (production's, the staging branch
/// for staging and the schema sandboxes, or a sandbox's own branch;
/// `app_events::stores`). Reading the publisher's planes may fail and pass.
async fn body_store(
    db: &DatabaseConnection,
    event: &app_events::Model,
) -> Result<OutboxTarget, BodyError> {
    store_of(db, event.source_app_id, &event.environment)
        .await
        .map_err(|e| BodyError::Transient(format!("find the event's store: {e}")))?
        .ok_or_else(|| {
            BodyError::Permanent(format!(
                "event {} is in `{}`, whose outbox the platform does not relay: its body \
                 cannot be read",
                event.id, event.environment
            ))
        })
}

/// The body of `event`, read from the org's OLTP store by `(writer_role, id)`
/// (§6.2): `None` when it is no longer kept (retention deleted it). The one
/// place a delivery reads it; the platform never holds it. Opens one short
/// connection to the org's store, which may be cold, so call it with no lock
/// and no platform transaction held.
pub async fn load_event_data(
    db: &DatabaseConnection,
    event: &app_events::Model,
) -> Result<Option<Value>, BodyError> {
    let target = body_store(db, event).await?;
    read_body(db, event.org_id, &target, &event.writer_role, event.id)
        .await
        .map_err(|e| BodyError::from_store(event.id, e))
}

/// `/apps/<slug>`: an event's `source`.
pub fn event_source(publisher_slug: &str) -> String {
    format!("/apps/{publisher_slug}")
}

/// The `dataschema` of every table event: its `data` has one shape.
pub const TABLE_DATASCHEMA: &str = "oxy/table.change@1";

/// `/tables/<table>`: a table event's `source`.
fn table_source(event_type: &str) -> String {
    TableRef::from_event_type(event_type)
        .map(|table| table.source())
        .unwrap_or_else(|| "/tables".to_string())
}

/// `<type>@<revision>`: an event's `dataschema`, empty without a revision;
/// [`TABLE_DATASCHEMA`] for a table event.
pub fn event_dataschema(event_type: &str, revision: Option<i32>) -> String {
    if is_table_event_type(event_type) {
        return TABLE_DATASCHEMA.to_string();
    }
    revision
        .map(|r| format!("{event_type}@{r}"))
        .unwrap_or_default()
}

async fn app_slug(txn: &DatabaseTransaction, app_id: Uuid) -> Result<Option<String>, DbErr> {
    Ok(apps::Entity::find_by_id(app_id)
        .one(txn)
        .await?
        .map(|app| app.slug))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dataschema_names_the_revision() {
        assert_eq!(event_dataschema("a/b.c", Some(3)), "a/b.c@3");
        assert_eq!(event_dataschema("a/b.c", None), "");
        assert_eq!(event_source("billing"), "/apps/billing");
    }

    fn event(environment: &str) -> app_events::Model {
        let id = Uuid::new_v4();
        app_events::Model {
            id,
            org_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            environment: environment.into(),
            source_app_id: Some(Uuid::new_v4()),
            r#type: "billing/statement.uploaded".into(),
            key: "s-1".into(),
            data_bytes: 2,
            writer_role: "app_billing_rw".into(),
            schema_revision_id: None,
            actor: None,
            causation_id: None,
            root_id: id,
            depth: 0,
            invocation_id: None,
            delay_seconds: 0,
            published_at: Utc::now().fixed_offset(),
            relayed_at: Utc::now().fixed_offset(),
            relay_error: None,
            mirrored_at: None,
        }
    }

    /// A body the store holds but Rust cannot decode will never be read: the
    /// reason names the event and says so. Anything else from the store (not
    /// reached, no active tenant, a failed query) may pass on a later attempt.
    #[test]
    fn only_an_undecodable_body_is_permanent() {
        let id = Uuid::new_v4();
        let undecodable = BodyError::from_store(
            id,
            EventsError::Undecodable("number out of range at line 1 column 9".into()),
        );
        let BodyError::Permanent(reason) = &undecodable else {
            panic!("permanent: {undecodable:?}");
        };
        assert!(reason.contains(&id.to_string()), "{reason}");
        assert!(reason.contains("cannot be decoded"), "{reason}");

        for e in [
            EventsError::Store("open the org's OLTP store: error connecting to server".into()),
            EventsError::NotProvisioned { org_id: id },
        ] {
            let text = e.to_string();
            let mapped = BodyError::from_store(id, e);
            assert_eq!(mapped, BodyError::Transient(text), "retryable");
            assert!(
                mapped
                    .to_string()
                    .starts_with("could not read the event's body: "),
                "{mapped}"
            );
        }
    }

    /// Every environment's body is in a store the platform reads: production's,
    /// the staging branch, or a sandbox's own branch (`stores`, which reads a
    /// sandbox's planes). An event in a name that is no environment has none:
    /// dead, and no store is ever touched (a disconnected handle would fail
    /// any query).
    #[tokio::test]
    async fn an_environment_with_no_relayed_store_is_permanent() {
        let nowhere = event("nowhere");
        let got = load_event_data(&DatabaseConnection::default(), &nowhere).await;
        let Err(BodyError::Permanent(reason)) = got else {
            panic!("permanent: {got:?}");
        };
        assert!(reason.contains(&nowhere.id.to_string()), "{reason}");
        assert!(reason.contains("`nowhere`"), "{reason}");
        let db = DatabaseConnection::default();
        assert_eq!(
            body_store(&db, &event("production")).await,
            Ok(OutboxTarget::Production)
        );
        assert_eq!(
            body_store(&db, &event("staging")).await,
            Ok(OutboxTarget::Branch("staging".to_string()))
        );
    }
}
