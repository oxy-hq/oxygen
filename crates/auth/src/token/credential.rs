//! The request marker: the credential an authenticated request carries.
//!
//! The pure check that admits a stored token row into one is
//! [`super::admission`]. Its items are re-exported here, where they used to
//! live, so every `credential::` path still resolves.

use chrono::{DateTime, Utc};
use entity::service_accounts;
use oxy_authz::{TokenGrant, TokenReach};
use uuid::Uuid;

use super::format::TokenFormat;

pub use super::admission::{
    LegacyLink, Links, ReadGrants, Refusal, admit, blocked_orgs, read_grants, readable_grants,
};

/// Where a row came from. Text in the table; constants here so the writers
/// agree.
pub mod source {
    /// Copied from `api_keys` by the migration.
    pub const LEGACY_BACKFILL: &str = "legacy_backfill";
    /// Copied from `api_keys` at first use (a key an older pod minted).
    pub const LEGACY_LAZY: &str = "legacy_lazy";
    /// Minted by `POST /api/{workspace_id}/api-keys`.
    pub const LEGACY_ENDPOINT: &str = "legacy_endpoint";
    /// Minted by `POST /api/user/tokens` — Account → Personal access tokens.
    pub const UI: &str = "ui";
    /// Minted by `POST /api/auth/cli/exchange` — `oxyc login`.
    pub const OXYC_LOGIN: &str = "oxyc_login";
    /// Minted by `POST /api/auth/oidc/exchange` — trusted access.
    pub const OIDC: &str = "oidc";
}

/// What a token's service account is, read from its `service_accounts` row:
/// the one org it belongs to, and whether it stands there as an admin. There
/// is no owner — an account is never one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountStanding {
    pub org_id: Uuid,
    pub admin: bool,
}

/// The state of the `service_accounts` row behind a token's principal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountLink {
    /// The principal is a person: no account row was looked for.
    NotAccount,
    Active(AccountStanding),
    /// `disabled_at` is set: every token of the account is refused.
    Disabled,
    /// No row, or a row whose `org_role` this release does not know.
    Missing,
}

impl AccountLink {
    /// The link a looked-up row gives. An `org_role` other than `member` or
    /// `admin` is refused rather than guessed at.
    pub fn of(row: Option<&service_accounts::Model>) -> Self {
        let Some(row) = row else {
            return Self::Missing;
        };
        if row.disabled_at.is_some() {
            return Self::Disabled;
        }
        let admin = match row.org_role.as_str() {
            service_accounts::ROLE_MEMBER => false,
            service_accounts::ROLE_ADMIN => true,
            _ => return Self::Missing,
        };
        Self::Active(AccountStanding {
            org_id: row.org_id,
            admin,
        })
    }
}

/// The stored kinds this release enforces. Any other `api_tokens.kind` —
/// `legacy_publish`, or one not invented yet — is refused, because this
/// release implements none of their restrictions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StoredKind {
    /// `oxy_pat_…`
    Personal,
    /// `oxy_<32 hex>` (or any value an `api_keys` row matched).
    LegacyKey,
    /// `oxy_sat_…` — owned by an org through a service account.
    ServiceAccount,
    /// `oxy_ci_…` — minted for one CI run by a trust policy, acting as the
    /// policy's service account for 15 minutes.
    Ci,
}

impl StoredKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::LegacyKey => "legacy_key",
            Self::ServiceAccount => "service_account",
            Self::Ci => "ci",
        }
    }

    /// Whether a token of this kind acts as a service account rather than as
    /// a person: its standing is the account's row, and it is always
    /// grant-bound inside the account's org.
    pub fn acts_as_account(self) -> bool {
        matches!(self, Self::ServiceAccount | Self::Ci)
    }

    /// `None` for every kind this release does not enforce.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "personal" => Some(Self::Personal),
            "legacy_key" => Some(Self::LegacyKey),
            "service_account" => Some(Self::ServiceAccount),
            "ci" => Some(Self::Ci),
            _ => None,
        }
    }

    /// The stored kind a presented credential must resolve to. A legacy key is
    /// also what any non-prefixed `X-API-Key` value is looked up as. `None`
    /// for a format this release has no stored kind for.
    pub(super) fn expected_for(presented: TokenFormat) -> Option<Self> {
        match presented {
            TokenFormat::Personal => Some(Self::Personal),
            TokenFormat::LegacyKey => Some(Self::LegacyKey),
            TokenFormat::ServiceAccount => Some(Self::ServiceAccount),
            TokenFormat::Ci => Some(Self::Ci),
            TokenFormat::LegacyPublish => None,
        }
    }
}

/// One live `app_publish` grant: the token may publish this app, and on the
/// custom-apps surface reaches nothing but it (design §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppPublishGrant {
    pub org_id: Uuid,
    pub app_id: Uuid,
}

/// Request-extension marker: this request authenticated with an API token or
/// key, not a browser session. Sits next to the unchanged
/// `AuthenticatedUser`, so no existing handler changes.
///
/// Its presence is what a session-only route refuses, and what later phases
/// narrow by. It never carries the token itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialContext {
    /// `api_tokens.id`. For a legacy key this equals its `api_keys.id`.
    pub token_id: Uuid,
    pub kind: StoredKind,
    pub principal_user_id: Uuid,
    pub all_access: bool,
    pub platform: bool,
    pub partner: bool,
    /// For audit metadata (design §3.7): the name and the non-secret prefix.
    pub name: String,
    pub display_prefix: String,
    /// The mirrored `api_keys` row, while that table exists.
    pub legacy_api_key_id: Option<Uuid>,
    /// The live workspace grants of a token with `all_access = false`. Empty
    /// for an all-access token and for every legacy credential.
    pub grants: Vec<TokenGrant>,
    /// The live `app_publish` grants of a token with `all_access = false`.
    /// Empty for an all-access token and for every legacy credential.
    pub app_publish: Vec<AppPublishGrant>,
    /// Orgs this token reaches nothing in: ones that ended its reach (a
    /// revoked org-wide grant), and — added by the store after admission —
    /// ones whose token policy it violates (`super::policy`). Always empty for
    /// a legacy credential: only its owner can end a legacy key (§3.5).
    pub blocked_orgs: Vec<Uuid>,
    /// Set for a token that acts as a service account (`service_account` or
    /// `ci`): the account's standing, read from its row when the request
    /// authenticated — never from the cache.
    pub service_account: Option<AccountStanding>,
    /// When the credential expires; `None` for never. What the
    /// `X-Oxy-Token-Expiration` response header reports.
    pub expires_at: Option<DateTime<Utc>>,
}

impl CredentialContext {
    /// A legacy credential: an `oxy_<hex>` key, or a token the legacy endpoint
    /// minted — anything that mirrors an `api_keys` row. It keeps exactly the
    /// reach and behaviour it had before tokens could be narrowed (§3.5):
    /// all-access, both standings, and the browser's assume-role session.
    pub fn is_legacy(&self) -> bool {
        self.kind == StoredKind::LegacyKey || self.legacy_api_key_id.is_some()
    }

    /// What this credential narrows its bearer to. `None` for a legacy
    /// credential — it narrows nothing, and decides as a session does.
    pub fn reach(&self) -> Option<TokenReach> {
        if self.is_legacy() {
            return None;
        }
        // A service account is grant-bound and holds no standing, whatever its
        // row says: admission refuses a row that claims otherwise, and this
        // does not rely on it having done so.
        let account = self.is_service_account();
        Some(TokenReach {
            all_access: self.all_access && !account,
            platform: self.platform && !account,
            partner: self.partner && !account,
            grants: self.grants.clone(),
            blocked_orgs: self.blocked_orgs.clone(),
        })
    }

    /// Whether the credential acts as an org's service account rather than as
    /// a person: an account's own token, or one a trust policy minted for it.
    pub fn is_service_account(&self) -> bool {
        self.kind.acts_as_account()
    }

    /// Whether the credential holds any `app_publish` grant. One that does is
    /// confined, on the custom-apps surface, to the apps it names.
    pub fn holds_app_publish(&self) -> bool {
        !self.app_publish.is_empty()
    }

    /// Whether an `app_publish` grant names this app.
    pub fn publishes_app(&self, app_id: Uuid) -> bool {
        self.app_publish.iter().any(|g| g.app_id == app_id)
    }
}
