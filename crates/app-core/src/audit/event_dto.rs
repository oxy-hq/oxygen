//! An audit row as the console reads it: the wire shape of `GET /admin/audit`'s
//! events, and of a key's Activity (`oxy-api-identity`'s `token_activity`), so
//! both answer with one shape.
//!
//! Down from `oxy-app`'s `admin::audit` in the admin split's PR 3 (it was the
//! prelude's `surface::admin_audit`).

use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize)]
pub struct AuditEventDto {
    pub id: Uuid,
    pub created_at: String,
    pub actor_email: String,
    pub actor_type: String,
    pub action: String,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub partner_id: Option<Uuid>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub target_label: Option<String>,
    pub outcome: String,
    pub reason: Option<String>,
    /// Staff/org action taken through the assume-role override — the single most
    /// important thing to spot when auditing, so it's lifted out of `metadata`.
    pub via_global_override: bool,
}

impl From<entity::audit_events::Model> for AuditEventDto {
    fn from(e: entity::audit_events::Model) -> Self {
        let via_global_override = e
            .metadata
            .get("via_global_override")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Self {
            id: e.id,
            created_at: e.created_at.to_rfc3339(),
            actor_email: e.actor_email,
            actor_type: e.actor_type,
            action: e.action,
            org_id: e.org_id,
            workspace_id: e.workspace_id,
            partner_id: e.partner_id,
            target_type: e.target_type,
            target_id: e.target_id,
            target_label: e.target_label,
            outcome: e.outcome,
            reason: e.reason,
            via_global_override,
        }
    }
}
