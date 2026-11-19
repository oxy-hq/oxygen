//! Step 2 of a relay pass: one claimed outbox row read as the event the
//! platform stores — under the environment its store vouches for and the id
//! the relay derives — and, when the row is refused, why. The body is never
//! read: the envelope's `data` is `null`.

use entity::{app_event_schemas, apps};
use oxy_custom_apps::events::{MAX_PAYLOAD_BYTES, OxyEvent, event_id, qualified_type};
use oxy_oltp::events::OutboxRow;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

use super::RelayedEvent;
use super::vouch::Vouched;
use crate::app_events::envelope::{event_dataschema, event_source};
use crate::app_events::provenance::{self, Invoker, Provenance, UNVERIFIED_PROVENANCE};
use crate::app_events::schema::live_revision;

/// What the platform holds for one event name of one app.
pub(super) struct EventSchemas {
    /// Whether the environment's live build declares the name.
    declared: bool,
    /// Every revision recorded for it, in any environment.
    revisions: Vec<app_event_schemas::Model>,
    /// A `ctx.apps.send`: nothing to declare, no revision to find.
    directed: bool,
}

impl EventSchemas {
    /// What a directed event is read against: it has no catalogue entry and
    /// no schema revision, so neither can be missing.
    pub(super) fn directed() -> Self {
        Self {
            declared: true,
            revisions: Vec::new(),
            directed: true,
        }
    }

    pub(super) async fn read(
        db: &DatabaseConnection,
        app_id: Uuid,
        environment: &str,
        name: &str,
    ) -> Result<Self, DbErr> {
        Ok(Self {
            declared: live_revision(db, app_id, environment, name)
                .await?
                .is_some(),
            revisions: app_event_schemas::Entity::find()
                .filter(app_event_schemas::Column::AppId.eq(app_id))
                .filter(app_event_schemas::Column::EventName.eq(name))
                .all(db)
                .await?,
            directed: false,
        })
    }

    /// The recorded revision `revision` names, if any.
    fn find(&self, revision: Option<Uuid>) -> Option<&app_event_schemas::Model> {
        revision.and_then(|r| self.revisions.iter().find(|s| s.id == r))
    }
}

/// The size of `row`'s payload when it is over [`MAX_PAYLOAD_BYTES`]: its
/// `jsonb` text length, as the claim read it from the store (and as the host
/// measures a publish), never the body itself.
fn oversized(row: &OutboxRow) -> Option<usize> {
    let bytes = usize::try_from(row.data_bytes).unwrap_or(0);
    (bytes > MAX_PAYLOAD_BYTES).then_some(bytes)
}

/// Row `row` of `app` as the event to store, in the environment its store
/// vouched for and under the id derived from it (§6.1), refused as
/// [`refusal`] says. The body is never read: the envelope's `data` is `null`.
pub(super) fn read_row(
    app: &apps::Model,
    row: &OutboxRow,
    vouched: Vouched,
    schemas: &EventSchemas,
    unsigned: &mut UnsignedInvokers,
) -> RelayedEvent {
    let relay_error = refusal(app, row, &vouched, schemas);
    let invoker = invoker(app, row, schemas, relay_error.as_deref(), unsigned);
    let (environment, _) = vouched;
    let event_type = qualified_type(&app.slug, &row.name);
    let id = event_id(app.org_id, &environment, &event_type, &row.key);
    let revision = schemas.find(row.schema_revision);
    RelayedEvent {
        envelope: OxyEvent {
            specversion: "1.0".to_string(),
            id,
            dataschema: event_dataschema(&event_type, revision.map(|r| r.revision)),
            event_type,
            source: event_source(&app.slug),
            time: row.created_at,
            // The body stays in the org's store.
            data: Value::Null,
            key: row.key.clone(),
            environment,
            invocation: row.invocation_id,
            actor: row.actor.clone(),
            causationid: row.causation_id,
            rootid: row.root_id,
            depth: row.depth,
        },
        org_id: app.org_id,
        workspace_id: app.project_id,
        source_app_id: Some(app.id),
        writer_role: row.writer_role.clone(),
        data_bytes: row.data_bytes,
        schema_revision_id: row.schema_revision,
        delay_seconds: row.delay_seconds,
        relay_error,
        directed: None,
        only_listener: None,
        invoker,
    }
}

/// Who a directed event's sending run ran as, when the host signed it
/// (`provenance::verified_invoker`): read only for a send (`ctx.apps.send`)
/// whose row the relay stores to deliver. An ordinary event never carries
/// one, a refused row is delivered to nobody, and a row whose invoker MAC is
/// missing or not the host's is the host's row all the same (its v1 MAC
/// verified), delivered as the system.
fn invoker(
    app: &apps::Model,
    row: &OutboxRow,
    schemas: &EventSchemas,
    relay_error: Option<&str>,
    unsigned: &mut UnsignedInvokers,
) -> Option<Invoker> {
    if !schemas.directed || relay_error.is_some() {
        return None;
    }
    let event_type = qualified_type(&app.slug, &row.name);
    let signed = Provenance::of_row(row, &event_type);
    let invoker = provenance::verified_invoker(app.org_id, &signed, row);
    if invoker.is_none() && (row.invoker.is_some() || row.invoker_provenance.is_some()) {
        unsigned.note(row.id);
    }
    invoker
}

/// The sends of one relay pass whose invoker columns or MAC were set without a
/// valid host MAC. App SQL writes the outbox through the definer's settings
/// and controls how many such rows it makes, so the relay counts them and
/// logs once per pass ([`UnsignedInvokers::log`], as `log_quarantined` does
/// for provenance), never once per row: visible, and not a warning per publish.
#[derive(Debug, Default)]
pub(super) struct UnsignedInvokers {
    count: usize,
    /// The first such row's event id, for someone to look into.
    example: Option<Uuid>,
}

impl UnsignedInvokers {
    fn note(&mut self, event_id: Uuid) {
        self.count += 1;
        self.example.get_or_insert(event_id);
    }

    /// One warning for the pass when any row was noted: the count and one
    /// example event id.
    pub(super) fn log(&self, org_id: Uuid) {
        if let Some(example) = self.example {
            tracing::warn!(
                %org_id,
                count = self.count,
                example_event_id = %example,
                "app events: sends whose invoker is not the host's were delivered as the system"
            );
        }
    }
}

/// Why `row` of `app` is stored but delivered to nobody, in this order: its
/// MAC is not the host's over its fields (R25(c): app SQL wrote it, or it
/// changed since), the stable [`UNVERIFIED_PROVENANCE`] code; its store
/// refused the environment it claims; its own id is not the one derived; its
/// payload is over [`MAX_PAYLOAD_BYTES`]; the environment's live build
/// declares no such event; the revision it was published against is recorded
/// for none (a directed event, `ctx.apps.send`, has none to find). `None`: it
/// is delivered.
fn refusal(
    app: &apps::Model,
    row: &OutboxRow,
    (environment, wrong_environment): &Vouched,
    schemas: &EventSchemas,
) -> Option<String> {
    let event_type = qualified_type(&app.slug, &row.name);
    let signed = Provenance::of_row(row, &event_type);
    if !provenance::verify(app.org_id, &signed, row.provenance.as_deref()) {
        return Some(unverified(row));
    }
    if let Some(wrong) = wrong_environment {
        return Some(wrong.clone());
    }
    let id = event_id(app.org_id, environment, &event_type, &row.key);
    if row.id != id {
        return Some(format!(
            "published under id {}, which is not the id of `{event_type}` key `{}` in \
             {environment}",
            row.id, row.key
        ));
    }
    if let Some(bytes) = oversized(row) {
        return Some(format!(
            "the payload is {bytes} bytes as jsonb text, over the {MAX_PAYLOAD_BYTES}-byte limit"
        ));
    }
    if !schemas.declared {
        return Some(format!(
            "{environment}'s build of {} declares no event `{}`",
            app.slug, row.name
        ));
    }
    (schemas.find(row.schema_revision).is_none() && !schemas.directed)
        .then(|| format!("published against a schema revision recorded for no `{event_type}`"))
}

/// The refusal of a row whose MAC is not the host's: the stable code, then
/// why, never a field's value.
fn unverified(row: &OutboxRow) -> String {
    let why = match row.provenance {
        None => {
            "the row carries no provenance MAC (app SQL calling oxy_events.publish, or a row \
             written before platform step 6)"
        }
        Some(_) => {
            "the row's provenance MAC is not the host's over its fields (app SQL calling \
             oxy_events.publish, or a row changed since it was written)"
        }
    };
    format!("{UNVERIFIED_PROVENANCE}: {why}; it is delivered to nobody")
}

#[cfg(test)]
mod tests {
    use super::UnsignedInvokers;
    use uuid::Uuid;

    /// Many rows in a pass are one count and the first row as the example,
    /// not one record per row; a pass with none notes nothing.
    #[test]
    fn unsigned_invokers_are_counted_with_the_first_row_as_the_example() {
        let mut unsigned = UnsignedInvokers::default();
        assert_eq!((unsigned.count, unsigned.example), (0, None));
        for n in 1..=5u128 {
            unsigned.note(Uuid::from_u128(n));
        }
        assert_eq!(unsigned.count, 5);
        assert_eq!(unsigned.example, Some(Uuid::from_u128(1)));
    }
}
