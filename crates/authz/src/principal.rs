//! The principal and credential vocabulary: who a request names, and the
//! plain facts about the token it arrived with.
//!
//! These are data plus pure methods. They moved here from `oxy-auth`
//! (`oxy_auth::types` and `oxy_auth::token::credential`), which re-exports
//! them for one release, so a contract crate can name them from the kernel.
//! What reads the database to build them stays in `oxy-auth`: the row
//! mapping (`AccountLink::of`), the presented-format mapping
//! (`credential::presented_as` / `credential::expected_for`, which need
//! `TokenFormat`), and the request credential `CredentialContext`.

use uuid::Uuid;

// Simple identity structure for email-based identity linking
#[derive(Debug, Clone)]
pub struct Identity {
    /// The user this credential names, when the credential carries an id.
    ///
    /// A session JWT always has one — `Claims.sub` has been the user id since
    /// tokens were introduced — and it is the ONLY identifier that works for a
    /// frontline worker, whose `users.email` is NULL. Resolution prefers this
    /// and falls back to [`Self::email`], which keeps every token minted before
    /// this field existed working unchanged.
    ///
    /// `None` for a provider identity (Google, Okta, magic link): at that point
    /// the address is all we have, and collapsing it onto a row is exactly what
    /// `get_or_create_user` is for.
    pub user_id: Option<uuid::Uuid>,
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
}

/// What a token's service account is, read from its `service_accounts` row:
/// the one org it belongs to, and whether it stands there as an admin. There
/// is no owner — an account is never one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountStanding {
    pub org_id: Uuid,
    pub admin: bool,
}

/// The stored kinds this release enforces. Any other `api_tokens.kind` —
/// `legacy_publish`, or one not invented yet — is refused, because this
/// release implements none of their restrictions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StoredKind {
    /// `oxy_pat_…`
    Personal,
    /// `oxy_<32 hex>` — **removed**. No presented format resolves to this and
    /// admission refuses it, but the kind stays so the rows left in
    /// `api_tokens` can still be listed, audited and explained.
    LegacyKey,
    /// `oxy_sat_…` — owned by an org through a service account.
    ServiceAccount,
    /// `oxy_ci_…` — minted for one CI run by a trust policy, acting as the
    /// policy's service account for 15 minutes.
    Ci,
    /// `oxy_sbx_…` — minted by a staff member for an agent: it reaches the
    /// sandboxes it creates of the apps it names, as its minter, for hours.
    SandboxAgent,
    /// `oxysvc_…` — a service configured in `OXY_SERVICE_TOKENS`
    /// (`oxy_auth::token::service`): platform standing from its entry, no
    /// stored row.
    Service,
    /// A custom app's own access (`oxy_auth::token::app_access`): the actions
    /// an org admin approved for the app, acting as the app's service account.
    /// Its row has **no secret**: no presented format resolves to it and
    /// admission refuses it. The function host builds the credential
    /// in-process, per host op, from the row and its live grants.
    App,
}

impl StoredKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::LegacyKey => "legacy_key",
            Self::ServiceAccount => "service_account",
            Self::Ci => "ci",
            Self::SandboxAgent => "sandbox_agent",
            Self::Service => "service",
            Self::App => "app",
        }
    }

    /// Whether a token of this kind acts as a service account rather than as
    /// a person: its standing is the account's row, and it is always
    /// grant-bound inside the account's org.
    pub fn acts_as_account(self) -> bool {
        matches!(self, Self::ServiceAccount | Self::Ci | Self::App)
    }

    /// `None` for every kind this release does not enforce.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "personal" => Some(Self::Personal),
            "legacy_key" => Some(Self::LegacyKey),
            "service_account" => Some(Self::ServiceAccount),
            "ci" => Some(Self::Ci),
            "sandbox_agent" => Some(Self::SandboxAgent),
            "app" => Some(Self::App),
            _ => None,
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

/// One live `app_sandbox` grant: a sandbox agent token may run the sandbox
/// loop on this app, in sandboxes it created itself (sandbox agent credential
/// design §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppSandboxGrant {
    pub org_id: Uuid,
    pub app_id: Uuid,
    /// Whether a live `app_staging` grant of the same token names this app
    /// too: the token may also publish a draft to, and exercise, the app's
    /// **staging** environment. `false` for a token minted without `staging`.
    pub staging: bool,
}
