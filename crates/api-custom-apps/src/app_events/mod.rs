//! App events on the platform side of the publish: each event's schema stored
//! as an immutable revision, checked for compatibility and used to validate a
//! payload; who listens to what; the relay that moves published events out
//! of an org's outbox and onto the task queue; and each delivery's attempts.
//! The pure shapes (names, catalogue, envelope) are in
//! `oxy_custom_apps::events`.

pub mod api;
pub mod deliveries;
pub mod directed;
pub mod envelope;
pub mod gate;
pub mod marks;
pub mod mirror;
pub mod provenance;
pub mod relay;
pub mod retention;
pub mod schema;
pub(crate) mod stores;
pub mod subscriptions;
pub mod table_watch;

use oxy_custom_apps::events::catalog::EventCatalog;
use sea_orm::{DatabaseConnection, DbErr};
use serde_json::Value;
use uuid::Uuid;

use crate::custom_apps_function_schema::MAX_SCHEMA_BYTES;

/// Whether `app_events` is on for `org_id`. Off, app events are inert: a
/// publish is refused, the gate refuses and warns nothing, and the relay, its
/// sweep and retention pass the org by. Subscriptions and schema revisions
/// still reconcile — platform metadata only — so turning the flag on works at
/// once.
pub fn enabled_for(org_id: Uuid) -> bool {
    use oxy_feature_flags::{APP_EVENTS, FlagContext, is_enabled_for};
    is_enabled_for(APP_EVENTS, &FlagContext::org(org_id))
}

/// The longest error message, in characters, a delivery or a delivery attempt
/// stores (`app_event_deliveries.last_error`, `app_event_delivery_attempts.error`):
/// what went wrong, never a dump of what the handler held.
pub const MAX_ERROR_CHARS: usize = 1_000;

/// `error` as a delivery or attempt stores it: at most [`MAX_ERROR_CHARS`]
/// characters, the last of them `…` when it was cut.
pub fn cap_error(error: &str) -> String {
    if error.chars().nth(MAX_ERROR_CHARS).is_none() {
        return error.to_string();
    }
    let kept: String = error.chars().take(MAX_ERROR_CHARS - 1).collect();
    format!("{kept}…")
}

/// The events a build's manifest declares, each schema checked. Names the
/// event in the message, so a publish refused for a bad schema says which.
///
/// Each schema is at most [`MAX_SCHEMA_BYTES`] serialized, the cap a
/// function's schema has: a schema is stored with every build, compiled for
/// every publish of the event, and compared with its revisions by the gate
/// ([`schema::incompatibilities`]), so every document those read is bounded.
pub fn declared_events(manifest: Option<&Value>) -> Result<EventCatalog, String> {
    let catalog = EventCatalog::from_manifest(manifest)?;
    for (name, decl) in &catalog.events {
        let size = serde_json::to_vec(&decl.schema).map_or(usize::MAX, |b| b.len());
        if size > MAX_SCHEMA_BYTES {
            return Err(format!(
                "event `{name}`: its schema is {size} bytes; an event's schema may be at most \
                 {MAX_SCHEMA_BYTES} bytes"
            ));
        }
        schema::check_schema(&decl.schema).map_err(|e| format!("event `{name}`: {e}"))?;
    }
    Ok(catalog)
}

/// Record the revision of every event the build declares (a schema already
/// stored is reused, a changed one becomes the next revision). Run for every
/// publish, in any environment: a revision is immutable and says nothing
/// about which environment serves it.
pub async fn record_revisions(
    db: &DatabaseConnection,
    app_id: Uuid,
    manifest: Option<&Value>,
) -> Result<(), DbErr> {
    // Checked at admission (`declared_events`); a manifest that fails it never gets here.
    let Ok(catalog) = EventCatalog::from_manifest(manifest) else {
        return Ok(());
    };
    for (name, decl) in &catalog.events {
        schema::revision_for(db, app_id, name, &decl.schema).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_is_capped_on_a_character_boundary() {
        assert_eq!(cap_error("boom"), "boom");
        let exact = "é".repeat(MAX_ERROR_CHARS);
        assert_eq!(cap_error(&exact), exact, "at the cap, kept whole");
        let capped = cap_error(&"é".repeat(MAX_ERROR_CHARS + 50));
        assert_eq!(capped.chars().count(), MAX_ERROR_CHARS);
        assert!(capped.ends_with('…'), "{capped}");
    }

    /// R40: an event's schema is held to the 16 KiB a function's schema is
    /// (`MAX_SCHEMA_BYTES`), at admission, naming the event: every schema
    /// comparison and validation then reads a bounded document.
    #[test]
    fn an_event_schema_over_the_size_cap_is_refused_at_admission() {
        use crate::custom_apps_function_schema::MAX_SCHEMA_BYTES;
        let schema = |description_len: usize| serde_json::json!({"type":"object","description":"x".repeat(description_len)});
        let base = serde_json::to_vec(&schema(0)).unwrap().len();
        let manifest =
            |schema: Value| serde_json::json!({"events":{"order.placed":{"schema":schema}}});

        let at_cap = schema(MAX_SCHEMA_BYTES - base);
        assert_eq!(serde_json::to_vec(&at_cap).unwrap().len(), MAX_SCHEMA_BYTES);
        declared_events(Some(&manifest(at_cap))).expect("at the cap");

        let over = schema(MAX_SCHEMA_BYTES - base + 1);
        let error = declared_events(Some(&manifest(over))).expect_err("over the cap");
        assert_eq!(
            error,
            format!(
                "event `order.placed`: its schema is {} bytes; an event's schema may be at most \
                 {MAX_SCHEMA_BYTES} bytes",
                MAX_SCHEMA_BYTES + 1
            )
        );
    }

    #[test]
    fn the_dead_sync_stores_a_capped_error() {
        assert!(
            retention::DEAD_SQL.contains(&format!(
                "left(COALESCE(r.error_message, d.last_error, \
                 'task ' || q.queue_status), {MAX_ERROR_CHARS})"
            )),
            "{}",
            retention::DEAD_SQL
        );
    }
}
