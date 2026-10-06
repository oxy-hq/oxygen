//! The org token policy, as a pure decision (API-tokens design §5).
//!
//! An org may cap how long a token reaching it lives, and refuse all-access
//! personal tokens. A token that violates its policy is **inert for that org,
//! never revoked**: it reaches nothing there, and everything it reaches
//! elsewhere stands. That is GitHub's behaviour, and it means one org's admin
//! cannot destroy a person's access to another org.
//!
//! **New-format tokens only.** A legacy key — any row mirroring `api_keys` —
//! is never judged here (§3.5): [`violation`] answers `None` for it, whatever
//! the policy says.
//!
//! The database side (loading policies, the orgs a token reaches) is
//! [`super::policy_store`]; this module decides from values, so the request
//! path, the token list and the extend check cannot disagree.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use entity::{api_tokens, org_token_policies};
use uuid::Uuid;

use super::credential::StoredKind;

/// The longest cap an org may set, in days (ten years). The shortest is 1.
pub const MAX_LIFETIME_DAYS_LIMIT: i32 = 3650;

/// What an org asks of the tokens that reach it. [`Default`] is what an org
/// with no row asks: no cap, all-access allowed, environments required.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrgPolicy {
    /// `expires_at - created_at` may not exceed this many days; a token with
    /// no expiry exceeds any cap. `None` = no cap.
    pub max_lifetime_days: Option<i32>,
    pub allow_all_access_tokens: bool,
    pub require_environment_on_trust_policies: bool,
}

impl Default for OrgPolicy {
    fn default() -> Self {
        Self {
            max_lifetime_days: None,
            allow_all_access_tokens: true,
            require_environment_on_trust_policies: true,
        }
    }
}

impl OrgPolicy {
    /// The policy a stored row says, or the defaults for no row.
    pub fn of(row: Option<&org_token_policies::Model>) -> Self {
        row.map_or_else(Self::default, |r| Self {
            max_lifetime_days: r.max_lifetime_days,
            allow_all_access_tokens: r.allow_all_access_tokens,
            require_environment_on_trust_policies: r.require_environment_on_trust_policies,
        })
    }

    /// Whether this policy can make any token inert. The defaults cannot.
    pub fn restricts(&self) -> bool {
        self.max_lifetime_days.is_some() || !self.allow_all_access_tokens
    }

    /// `Err(reason)` for a policy the API refuses: a cap outside 1–3650.
    pub fn validate(&self) -> Result<(), String> {
        match self.max_lifetime_days {
            Some(days) if !(1..=MAX_LIFETIME_DAYS_LIMIT).contains(&days) => Err(format!(
                "'max_lifetime_days' must be between 1 and {MAX_LIFETIME_DAYS_LIMIT}, or null"
            )),
            _ => Ok(()),
        }
    }
}

/// Why a policy makes a token inert. The wire strings are
/// `Token.blocked_orgs[].reason` and the inventory's `blocked_by_policy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Violation {
    /// The token lives longer than the org's cap, or never expires under one.
    MaxLifetime,
    /// An all-access personal token, in an org that refuses them.
    AllAccessDisallowed,
}

impl Violation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MaxLifetime => "max_lifetime",
            Self::AllAccessDisallowed => "all_access_disallowed",
        }
    }
}

/// What a policy reads of a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenShape {
    pub kind: StoredKind,
    /// Mirrors an `api_keys` row: never judged.
    pub legacy: bool,
    pub all_access: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl TokenShape {
    /// `None` for a kind this release does not enforce — admission refuses
    /// such a row before a policy could matter.
    pub fn of(row: &api_tokens::Model) -> Option<Self> {
        let kind = StoredKind::parse(&row.kind)?;
        Some(Self {
            kind,
            legacy: kind == StoredKind::LegacyKey || row.legacy_api_key_id.is_some(),
            all_access: row.all_access,
            created_at: row.created_at.into(),
            expires_at: row.expires_at.map(Into::into),
        })
    }

    /// An all-access **personal** token: the one shape the all-access rule
    /// is about. An account's token is grant-bound by construction.
    pub fn all_access_personal(&self) -> bool {
        !self.legacy && self.kind == StoredKind::Personal && self.all_access
    }
}

/// Whether a token living from `created_at` to `expires_at` outlives a cap of
/// `cap_days`. A token with no expiry outlives every cap.
pub fn exceeds_lifetime(
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    cap_days: i32,
) -> bool {
    match expires_at {
        None => true,
        Some(at) => at - created_at > Duration::days(i64::from(cap_days)),
    }
}

/// How `policy` judges `token`: `None` when the token may reach the org. A
/// legacy key is never judged. When both rules are broken the all-access one
/// is named: shortening the token would not lift it.
pub fn violation(token: &TokenShape, policy: &OrgPolicy) -> Option<Violation> {
    if token.legacy {
        return None;
    }
    if !policy.allow_all_access_tokens && token.all_access_personal() {
        return Some(Violation::AllAccessDisallowed);
    }
    match policy.max_lifetime_days {
        Some(cap) if exceeds_lifetime(token.created_at, token.expires_at, cap) => {
            Some(Violation::MaxLifetime)
        }
        _ => None,
    }
}

/// The orgs among `reach` whose policy makes `token` inert there, in `reach`'s
/// order. An org with no entry in `policies` holds the defaults, which never
/// block.
pub fn blocked_in(
    token: &TokenShape,
    reach: &[Uuid],
    policies: &HashMap<Uuid, OrgPolicy>,
) -> Vec<(Uuid, Violation)> {
    reach
        .iter()
        .filter_map(|org| {
            let policy = policies.get(org)?;
            violation(token, policy).map(|v| (*org, v))
        })
        .collect()
}

/// The tightest lifetime cap among `policies`, in days.
pub fn tightest_cap<'a>(policies: impl IntoIterator<Item = &'a OrgPolicy>) -> Option<i32> {
    policies
        .into_iter()
        .filter_map(|p| p.max_lifetime_days)
        .min()
}

/// Whether a token may be given `expires_at`, under the tightest cap of the
/// orgs it holds grants in: `Err(cap)` when it would outlive it.
///
/// Only a **grant-bound** new-format token is refused. A legacy key is never
/// capped (§3.5), and an all-access token is not refused either — it simply
/// becomes inert in the capping orgs, since it reaches every org its owner is
/// in and refusing would let one org decide its expiry everywhere.
pub fn check_expiry(
    token: &TokenShape,
    expires_at: Option<DateTime<Utc>>,
    cap: Option<i32>,
) -> Result<(), i32> {
    if token.legacy || token.all_access_personal() {
        return Ok(());
    }
    match cap {
        Some(cap) if exceeds_lifetime(token.created_at, expires_at, cap) => Err(cap),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
