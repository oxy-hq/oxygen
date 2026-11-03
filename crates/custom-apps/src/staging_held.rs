//! What a custom app's **staging** environment held, written once: the
//! `app.staging.held` audit row.
//!
//! **The writer** ([`record_held`]) is the only place an `app.staging.held`
//! audit row is built. A staging `/fn` invocation's held-write log
//! (`custom_apps_functions::host::held_log`) calls it on flush, on a late
//! note and on drop; a staging agent ask and a staging automation run call
//! it with their own `function_or_surface`. One shape, so the list
//! (`GET /api/customer-apps/{id}/staging/held`, in `oxy-api-custom-apps`) reads
//! every row the same way.

use oxy_app_core::audit::AuditEntry;
use sea_orm::DatabaseConnection;
use serde_json::json;
use uuid::Uuid;

use crate::write_record::{ACTION_STAGING_HELD, WriteRecord, actor_entry, with_first_target};

/// Who caused a held row.
#[derive(Debug, Clone)]
pub enum HeldActor {
    /// The verified user (`audit_events.actor_user_id`).
    User { id: Uuid, email: Option<String> },
    /// The platform acting for the app (`system:app:<slug>`). Only a
    /// function run (behind `custom-app-functions`) has no human caller.
    System,
}

/// One `app.staging.held` row: every call one staging invocation, ask or run
/// held or refused.
#[derive(Debug, Clone)]
pub struct HeldRow {
    pub app_id: Uuid,
    pub app_slug: String,
    pub org_id: Uuid,
    pub project_id: Uuid,
    /// `staging`, or a `dev-<handle>` slot.
    pub environment: String,
    pub actor: HeldActor,
    /// The function's name, or the surface (`agent`, `automation`).
    pub function_or_surface: String,
    /// `route` / `schedule` / `airway` / `manual`, or the surface's own.
    pub mode: String,
    pub request_id: Option<Uuid>,
    pub writes: Vec<WriteRecord>,
    pub invocation_id: Option<Uuid>,
    pub trace_id: Option<String>,
    /// The sandbox agent token behind the run, written as `metadata.token_id`
    /// when there is one. Every other row's metadata is as it was.
    pub token_id: Option<Uuid>,
}

/// The audit entry for `row`. Metadata keeps every field the host wrote
/// before (`app_slug`, `function`, `invocation_id`, `mode`, `request_id`,
/// `trace_id`, `writes`) and adds `app_id` and `actor_user_id`.
pub fn held_entry(row: &HeldRow) -> AuditEntry {
    let user = match &row.actor {
        HeldActor::User { id, email } => Some((*id, email.as_deref())),
        HeldActor::System => None,
    };
    let e = actor_entry(ACTION_STAGING_HELD, user, &row.app_slug)
        .org(row.org_id)
        .workspace(row.project_id);
    let mut metadata = json!({
        "app_id": row.app_id,
        "app_slug": row.app_slug,
        "actor_user_id": user.map(|(id, _)| id),
        "function": row.function_or_surface,
        "invocation_id": row.invocation_id,
        "mode": row.mode,
        "request_id": row.request_id,
        "trace_id": row.trace_id,
        "writes": row.writes,
    });
    if let Some(token_id) = row.token_id {
        metadata[oxy_app_core::audit::TOKEN_ID_KEY] = json!(token_id);
    }
    with_first_target(e, &row.writes)
        .metadata(metadata)
        .environment(row.environment.clone())
}

/// Write `row`, best effort. A row with nothing held writes nothing, so a
/// production caller that noted nothing never produces one.
pub async fn record_held(db: &DatabaseConnection, row: HeldRow) {
    if row.writes.is_empty() {
        return;
    }
    oxy_app_core::audit::record_best_effort(db, held_entry(&row)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(actor: HeldActor) -> HeldRow {
        HeldRow {
            app_id: Uuid::from_u128(1),
            app_slug: "orders".into(),
            org_id: Uuid::from_u128(2),
            project_id: Uuid::from_u128(3),
            environment: "staging".into(),
            actor,
            function_or_surface: "save".into(),
            mode: "route".into(),
            request_id: None,
            writes: vec![WriteRecord {
                plane: "fetch",
                namespace: "api.example.com".into(),
                verb: "POST".into(),
                table: String::new(),
                rows: None,
                statements: 1,
                op: Some("fetch"),
                note: None,
                sent: None,
            }],
            invocation_id: Some(Uuid::from_u128(4)),
            trace_id: Some("t".into()),
            token_id: None,
        }
    }

    #[test]
    fn a_held_entry_stamps_the_app_and_the_actor_column() {
        let user = Uuid::from_u128(9);
        let e = held_entry(&row(HeldActor::User {
            id: user,
            email: Some("dev@oxy.tech".into()),
        }));
        assert_eq!(e.action, "app.staging.held");
        assert_eq!(e.actor_user_id, Some(user), "the column, not only metadata");
        assert_eq!(e.actor_email, "dev@oxy.tech");
        assert_eq!(e.environment.as_deref(), Some("staging"));
        assert_eq!(e.org_id, Some(Uuid::from_u128(2)));
        assert_eq!(e.workspace_id, Some(Uuid::from_u128(3)));
        assert_eq!(e.target_id.as_deref(), Some("fetch:api.example.com"));
        let m = &e.metadata;
        assert_eq!(m["app_id"], json!(Uuid::from_u128(1)));
        assert_eq!(m["actor_user_id"], json!(user));
        for key in [
            "app_slug",
            "function",
            "invocation_id",
            "mode",
            "request_id",
            "trace_id",
            "writes",
        ] {
            assert!(m.get(key).is_some(), "keeps {key}: {m}");
        }
        assert_eq!(m["writes"][0]["op"], "fetch");
        assert!(m.get("token_id").is_none(), "no token, no key: {m}");
    }

    /// A run behind a sandbox agent token names that token on its held row;
    /// the actor stays the user, as on every other row.
    #[test]
    fn a_held_entry_names_the_sandbox_agent_token_behind_the_run() {
        let (user, token) = (Uuid::from_u128(9), Uuid::from_u128(0x70));
        let mut held = row(HeldActor::User {
            id: user,
            email: Some("dev@oxy.tech".into()),
        });
        held.token_id = Some(token);
        let e = held_entry(&held);
        assert_eq!(e.metadata["token_id"], json!(token));
        assert_eq!(e.actor_user_id, Some(user));
    }

    #[test]
    fn a_held_entry_with_no_user_is_the_platform_acting_for_the_app() {
        let e = held_entry(&row(HeldActor::System));
        assert_eq!(e.actor_user_id, None);
        assert_eq!(e.actor_email, "system:app:orders");
        assert!(e.metadata["actor_user_id"].is_null());
    }
}
