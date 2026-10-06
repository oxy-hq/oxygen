//! Audit rows for a service account's own lifecycle (API-tokens design §3.7):
//! `service_account.created | updated | disabled | deleted`, in the org's
//! chain, written with `record_in_txn` in the transaction that makes the
//! change.
//!
//! A service account's **tokens** are audited by the shared token events
//! (`user_tokens::audit::Event`), each written to the account's one org.

use entity::service_accounts;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};

use crate::server::api::user_tokens::error::TokenError;

pub(super) const TARGET_TYPE: &str = "service_account";

pub(super) const CREATED: &str = "service_account.created";
pub(super) const UPDATED: &str = "service_account.updated";
pub(super) const DISABLED: &str = "service_account.disabled";
pub(super) const DELETED: &str = "service_account.deleted";

/// What an account is, for `created` and the before/after of an edit.
pub(super) fn summary(row: &service_accounts::Model) -> Value {
    json!({
        "name": row.name,
        "org_role": row.org_role,
        "description": row.description,
        "disabled": row.disabled_at.is_some(),
    })
}

/// The action an edit is recorded as: `disabled` when it is what switched the
/// account off — the event a reviewer looks for — and `updated` otherwise,
/// re-enabling included (its before/after says so).
pub(super) fn edit_action(
    before: &service_accounts::Model,
    after: &service_accounts::Model,
) -> &'static str {
    if before.disabled_at.is_none() && after.disabled_at.is_some() {
        DISABLED
    } else {
        UPDATED
    }
}

fn entry(
    actor: &RequestActor,
    action: &'static str,
    row: &service_accounts::Model,
    detail: Value,
) -> AuditEntry {
    AuditEntry::for_request(actor, action)
        .org(row.org_id)
        .target(TARGET_TYPE, row.user_id.to_string(), row.name.clone())
        .metadata(detail)
}

/// Record one lifecycle event of `row`. A failed write fails the request: an
/// account never changes without the row that says who changed it.
pub(super) async fn record<C: ConnectionTrait>(
    txn: &C,
    actor: &RequestActor,
    action: &'static str,
    row: &service_accounts::Model,
    detail: Value,
    change: Option<(Value, Value)>,
) -> Result<(), TokenError> {
    let mut entry = entry(actor, action, row, detail);
    if let Some((before, after)) = change {
        entry = entry.change(before, after);
    }
    audit::record_in_txn(txn, entry).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn account(disabled: bool) -> service_accounts::Model {
        let now = Utc::now().fixed_offset();
        service_accounts::Model {
            user_id: Uuid::from_u128(7),
            org_id: Uuid::from_u128(0xA),
            org_role: "member".into(),
            name: "deploy-bot".into(),
            description: None,
            created_by: None,
            created_at: now,
            disabled_at: disabled.then_some(now),
        }
    }

    #[test]
    fn switching_an_account_off_is_its_own_event() {
        assert_eq!(edit_action(&account(false), &account(true)), DISABLED);
        // Re-enabling, and an edit of an account that stays off, are updates.
        assert_eq!(edit_action(&account(true), &account(false)), UPDATED);
        assert_eq!(edit_action(&account(true), &account(true)), UPDATED);
        assert_eq!(edit_action(&account(false), &account(false)), UPDATED);
    }

    #[test]
    fn the_row_names_the_account_in_its_orgs_chain() {
        let row = account(false);
        let actor = RequestActor::session(oxy_auth::types::AuthenticatedUser {
            id: Uuid::from_u128(1),
            email: Some("ada@acme.com".into()),
            name: "Ada".into(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        });
        let entry = entry(&actor, CREATED, &row, summary(&row));
        assert_eq!(entry.org_id, Some(row.org_id));
        assert_eq!(entry.target_type.as_deref(), Some(TARGET_TYPE));
        assert_eq!(
            entry.target_id.as_deref(),
            Some(row.user_id.to_string().as_str())
        );
        assert_eq!(entry.metadata["org_role"], "member");
        assert_eq!(entry.metadata["disabled"], false);
    }
}
