//! A grant as the row that stores it.
//!
//! A token's grants live in `api_token_grants` and a trust policy's in
//! `oidc_trust_policy_grants`. The two tables say what is granted in the same
//! five columns and differ only in the parent a row hangs off (a token's grant
//! can also be revoked). [`GrantRow`] is that shared part, read from a grant in
//! one place, so the three writers — a personal token, a `ci` token and a trust
//! policy — cannot come to store the same grant differently.

use chrono::{DateTime, FixedOffset};
use entity::{api_token_grants, oidc_trust_policy_grants};
use sea_orm::Set;
use uuid::Uuid;

use super::personal::GrantSpec;
use super::trust_policy_access::PolicyGrant;

/// What a grant stores, whichever table holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GrantRow {
    kind: &'static str,
    org_id: Uuid,
    workspace_id: Option<Uuid>,
    role_ceiling: Option<String>,
    app_id: Option<Uuid>,
}

impl GrantRow {
    /// A workspace grant: an org, one workspace or all of them, and a ceiling.
    pub(super) fn workspace(spec: &GrantSpec) -> Self {
        Self {
            kind: api_token_grants::KIND_WORKSPACE,
            org_id: spec.org_id,
            workspace_id: spec.workspace_id,
            role_ceiling: Some(spec.ceiling.as_str().to_string()),
            app_id: None,
        }
    }

    /// Either kind a trust policy, and the `ci` token it mints, can hold.
    pub(super) fn of(grant: &PolicyGrant) -> Self {
        match grant {
            PolicyGrant::Workspace(spec) => Self::workspace(spec),
            PolicyGrant::AppPublish { org_id, app_id } => Self {
                kind: api_token_grants::KIND_APP_PUBLISH,
                org_id: *org_id,
                workspace_id: None,
                role_ceiling: None,
                app_id: Some(*app_id),
            },
        }
    }

    /// The one grant a sandbox agent token holds: an app, in the org that
    /// owns it. No workspace and no ceiling — admission derives both.
    pub(super) fn app_sandbox(org_id: Uuid, app_id: Uuid) -> Self {
        Self {
            kind: api_token_grants::KIND_APP_SANDBOX,
            org_id,
            workspace_id: None,
            role_ceiling: None,
            app_id: Some(app_id),
        }
    }

    /// The grant that may sit beside an app's `app_sandbox` one, on a sandbox
    /// agent token minted with `staging`: the same app, in the same org.
    pub(super) fn app_staging(org_id: Uuid, app_id: Uuid) -> Self {
        Self {
            kind: api_token_grants::KIND_APP_STAGING,
            ..Self::app_sandbox(org_id, app_id)
        }
    }

    /// The row under a token. A new grant is live: not revoked.
    pub(super) fn for_token(
        self,
        token_id: Uuid,
        created_at: DateTime<FixedOffset>,
    ) -> api_token_grants::ActiveModel {
        api_token_grants::ActiveModel {
            id: Set(Uuid::new_v4()),
            token_id: Set(token_id),
            kind: Set(self.kind.to_string()),
            org_id: Set(self.org_id),
            workspace_id: Set(self.workspace_id),
            role_ceiling: Set(self.role_ceiling),
            app_id: Set(self.app_id),
            created_at: Set(created_at),
            revoked_at: Set(None),
            revoked_by: Set(None),
        }
    }

    /// The row under a trust policy.
    pub(super) fn for_policy(
        self,
        policy_id: Uuid,
        created_at: DateTime<FixedOffset>,
    ) -> oidc_trust_policy_grants::ActiveModel {
        oidc_trust_policy_grants::ActiveModel {
            id: Set(Uuid::new_v4()),
            policy_id: Set(policy_id),
            kind: Set(self.kind.to_string()),
            org_id: Set(self.org_id),
            workspace_id: Set(self.workspace_id),
            role_ceiling: Set(self.role_ceiling),
            app_id: Set(self.app_id),
            created_at: Set(created_at),
        }
    }
}

#[cfg(test)]
#[path = "grant_row_tests.rs"]
mod tests;
