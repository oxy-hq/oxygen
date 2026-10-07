//! The pure check that admits a stored token row.
//!
//! **Refuse what you cannot enforce** (design §4.7). This release enforces a
//! personal token's narrowing — workspace grants, `app_publish` grants,
//! `all_access = false`, the `platform` / `partner` flags off, and an org's
//! block — and the two kinds that act as a service account, which are
//! grant-bound inside the account's one org: a `service_account` token an
//! admin minted, and a `ci` token a trust policy minted. So it refuses:
//!
//! - a kind it does not implement (`legacy_publish`, or one not invented yet);
//! - an account's row that claims more than an account can hold
//!   (`all_access`, `platform`, `partner`), names a grant outside its org, or
//!   whose account is disabled or gone;
//! - a grant it cannot read: a kind it does not know, a ceiling it does not
//!   know, or an `app_publish` or `app_sandbox` grant naming no app;
//! - a grant on the wrong kind of token: an `app_sandbox` grant, and the
//!   `app_staging` grant that may sit beside it, are what a **sandbox agent
//!   token** holds and nothing else does, and such a token holds no other
//!   kind — nor an `app_staging` grant whose app it holds no `app_sandbox`
//!   grant for ([`super::sandbox_admission`]);
//! - any narrowing on a row that mirrors an `api_keys` row. A legacy key, and
//!   a token the legacy endpoint minted, are all-access with both standings —
//!   always (§3.5) — and a pod one release back validates them from `api_keys`
//!   with full reach, so honouring a narrowed one here would be a restriction
//!   a revert silently drops.
//!
//! A later release that adds a restriction therefore fails closed when
//! reverted to this one, instead of silently regaining full reach.

use chrono::{DateTime, Utc};
use entity::{api_token_grants, api_tokens};
use oxy_authz::{RoleCeiling, TokenGrant};
use uuid::Uuid;

use super::credential::{
    AccountLink, AccountStanding, AppPublishGrant, AppSandboxGrant, CredentialContext, StoredKind,
};
use super::format::TokenFormat;
use super::sandbox_admission;

/// The state of a row's mirrored `api_keys` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyLink {
    /// The row mirrors no `api_keys` row.
    NotLinked,
    Active,
    /// `api_keys.is_active = false`: revoked, possibly by a pod one release
    /// back that writes only `api_keys`.
    Inactive,
    /// Linked, but the `api_keys` row is gone.
    Missing,
}

/// Why a stored row was not admitted. Every variant answers 401; the variant
/// exists for the log line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A kind this release does not enforce.
    UnknownKind(String),
    /// The row asks for a restriction this release cannot enforce.
    Narrowed(&'static str),
    /// A grant of a kind, or with a ceiling, this release cannot enforce.
    UnknownGrant(String),
    /// The presented format does not match the stored kind.
    KindMismatch,
    /// A service-account row claiming what an account never holds.
    Widened(&'static str),
    /// A sandbox-agent row claiming what the kind never holds, or lacking
    /// what it always has.
    SandboxWidened(&'static str),
    /// The token's service account is disabled.
    AccountDisabled,
    /// The token's service account is gone, or unreadable.
    AccountMissing,
    Revoked,
    Expired,
    LegacyKeyInactive,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownKind(kind) => write!(f, "token kind '{kind}' is not enforced here"),
            Self::Narrowed(what) => {
                write!(f, "token is narrowed by {what}, which is not enforced here")
            }
            Self::UnknownGrant(what) => write!(f, "token grant {what} is not enforced here"),
            Self::KindMismatch => f.write_str("token format does not match its stored kind"),
            Self::Widened(what) => {
                write!(
                    f,
                    "service-account token claims {what}, which no account holds"
                )
            }
            Self::SandboxWidened(what) => {
                write!(
                    f,
                    "sandbox agent token has {what}, which the kind never does"
                )
            }
            Self::AccountDisabled => f.write_str("token's service account is disabled"),
            Self::AccountMissing => f.write_str("token's service account is gone or unreadable"),
            Self::Revoked => f.write_str("token is revoked"),
            Self::Expired => f.write_str("token is expired"),
            Self::LegacyKeyInactive => f.write_str("token's api_keys row is revoked or gone"),
        }
    }
}

/// A row's live grants, by kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadGrants {
    pub workspace: Vec<TokenGrant>,
    pub app_publish: Vec<AppPublishGrant>,
    /// What a sandbox agent token holds, and nothing else may.
    pub app_sandbox: Vec<AppSandboxGrant>,
    /// The apps a sandbox agent token was also granted **staging** of, as
    /// `(org, app)`: one `app_staging` grant each. Folded into
    /// [`AppSandboxGrant::staging`] by `sandbox_admission::refuse_misplaced`,
    /// which refuses one that has no `app_sandbox` twin.
    pub app_staging: Vec<(Uuid, Uuid)>,
}

fn workspace_grant(grant: &api_token_grants::Model) -> Result<TokenGrant, Refusal> {
    let raw = grant.role_ceiling.as_deref().unwrap_or("");
    let ceiling =
        RoleCeiling::parse(raw).ok_or_else(|| Refusal::UnknownGrant(format!("ceiling '{raw}'")))?;
    Ok(TokenGrant {
        org_id: grant.org_id,
        workspace_id: grant.workspace_id,
        ceiling,
    })
}

fn app_publish_grant(grant: &api_token_grants::Model) -> Result<AppPublishGrant, Refusal> {
    let app_id = grant
        .app_id
        .ok_or_else(|| Refusal::UnknownGrant("app_publish naming no app".to_string()))?;
    Ok(AppPublishGrant {
        org_id: grant.org_id,
        app_id,
    })
}

fn app_sandbox_grant(grant: &api_token_grants::Model) -> Result<AppSandboxGrant, Refusal> {
    let app_id = grant
        .app_id
        .ok_or_else(|| Refusal::UnknownGrant("app_sandbox naming no app".to_string()))?;
    Ok(AppSandboxGrant {
        org_id: grant.org_id,
        app_id,
        staging: false,
    })
}

fn app_staging_grant(grant: &api_token_grants::Model) -> Result<(Uuid, Uuid), Refusal> {
    let app_id = grant
        .app_id
        .ok_or_else(|| Refusal::UnknownGrant("app_staging naming no app".to_string()))?;
    Ok((grant.org_id, app_id))
}

/// The grants of a row, as the model reads them — or the first one this
/// release cannot enforce. A grant its org revoked is skipped: it no longer
/// reaches anything, and the token's other grants stand.
pub fn read_grants(grants: &[api_token_grants::Model]) -> Result<ReadGrants, Refusal> {
    let mut out = ReadGrants::default();
    for grant in grants.iter().filter(|g| g.revoked_at.is_none()) {
        match grant.kind.as_str() {
            api_token_grants::KIND_WORKSPACE => out.workspace.push(workspace_grant(grant)?),
            api_token_grants::KIND_APP_PUBLISH => out.app_publish.push(app_publish_grant(grant)?),
            api_token_grants::KIND_APP_SANDBOX => out.app_sandbox.push(app_sandbox_grant(grant)?),
            api_token_grants::KIND_APP_STAGING => out.app_staging.push(app_staging_grant(grant)?),
            other => return Err(Refusal::UnknownGrant(format!("kind '{other}'"))),
        }
    }
    Ok(out)
}

/// The workspace grants of a row — see [`read_grants`].
///
/// An `app_sandbox` grant reads as none here, and so does the `app_staging`
/// grant beside it: the workspace grant they stand for names the workspace
/// the app is published from *now*, which is a lookup the request path makes
/// ([`sandbox_admission::place_sandbox_apps`]). The inventories that call
/// this never list a sandbox agent token.
pub fn readable_grants(grants: &[api_token_grants::Model]) -> Result<Vec<TokenGrant>, Refusal> {
    Ok(read_grants(grants)?.workspace)
}

/// A row that mirrors `api_keys` — a legacy key, or a token the legacy
/// endpoint minted — may not be narrowed at all.
fn refuse_narrowed_legacy(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
) -> Result<(), Refusal> {
    for (flag, value) in [
        ("all_access=false", row.all_access),
        ("platform=false", row.platform),
        ("partner=false", row.partner),
    ] {
        if !value {
            return Err(Refusal::Narrowed(flag));
        }
    }
    if !grants.is_empty() {
        return Err(Refusal::Narrowed("grants"));
    }
    Ok(())
}

/// The orgs that ended a token's reach into them: a **revoked org-wide**
/// workspace grant is the block (the org token inventory's *revoke grant*). It
/// is honoured for an all-access token too — which has no live grant to
/// revoke — and outlives any grant the owner adds in that org afterwards.
pub fn blocked_orgs(grants: &[api_token_grants::Model]) -> Vec<Uuid> {
    let mut orgs: Vec<Uuid> = Vec::new();
    for grant in grants {
        let block = grant.revoked_at.is_some()
            && grant.kind == api_token_grants::KIND_WORKSPACE
            && grant.workspace_id.is_none();
        if block && !orgs.contains(&grant.org_id) {
            orgs.push(grant.org_id);
        }
    }
    orgs
}

/// An account's row — a `service_account` token or a `ci` one — may not claim
/// what an account never holds, nor mirror an `api_keys` row, which would make
/// it read as a legacy key and narrow nothing.
fn refuse_widened_account(row: &api_tokens::Model) -> Result<(), Refusal> {
    if row.legacy_api_key_id.is_some() {
        return Err(Refusal::Widened("an api_keys mirror"));
    }
    for (flag, value) in [
        ("all_access", row.all_access),
        ("platform", row.platform),
        ("partner", row.partner),
    ] {
        if value {
            return Err(Refusal::Widened(flag));
        }
    }
    Ok(())
}

/// The standing of the account behind a `service_account` or `ci` token, or
/// why it has none. A grant outside the account's org is one this release
/// cannot enforce.
fn account_standing(account: AccountLink, grants: &ReadGrants) -> Result<AccountStanding, Refusal> {
    let standing = match account {
        AccountLink::Active(standing) => standing,
        AccountLink::Disabled => return Err(Refusal::AccountDisabled),
        AccountLink::Missing | AccountLink::NotAccount => return Err(Refusal::AccountMissing),
    };
    let elsewhere = grants.workspace.iter().any(|g| g.org_id != standing.org_id)
        || grants
            .app_publish
            .iter()
            .any(|g| g.org_id != standing.org_id);
    if elsewhere {
        return Err(Refusal::UnknownGrant(
            "outside the service account's org".to_string(),
        ));
    }
    Ok(standing)
}

/// The two rows a token may hang off, as the store found them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Links {
    pub legacy: LegacyLink,
    pub account: AccountLink,
}

impl Links {
    /// A personal token: it mirrors no `api_keys` row and acts as a person.
    pub const NONE: Links = Links {
        legacy: LegacyLink::NotLinked,
        account: AccountLink::NotAccount,
    };

    pub fn legacy(legacy: LegacyLink) -> Self {
        Self {
            legacy,
            ..Self::NONE
        }
    }

    pub fn account(account: AccountLink) -> Self {
        Self {
            account,
            ..Self::NONE
        }
    }
}

/// Admit a stored row for a presented credential, or say why not. `grants` are
/// the row's `api_token_grants`, revoked or not.
///
/// Order: what the row *is* (kind, restrictions) before whether it is *live*,
/// so a narrowed row reads as narrowed in the log even after it lapses.
pub fn admit(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
    presented: TokenFormat,
    links: Links,
    now: DateTime<Utc>,
) -> Result<CredentialContext, Refusal> {
    let kind =
        StoredKind::parse(&row.kind).ok_or_else(|| Refusal::UnknownKind(row.kind.clone()))?;
    if kind.acts_as_account() {
        refuse_widened_account(row)?;
    }
    if kind == StoredKind::SandboxAgent {
        sandbox_admission::refuse_widened(row)?;
    }
    let legacy = kind == StoredKind::LegacyKey || row.legacy_api_key_id.is_some();
    let blocked = if legacy {
        Vec::new()
    } else {
        blocked_orgs(grants)
    };
    let grants = if legacy {
        refuse_narrowed_legacy(row, grants)?;
        ReadGrants::default()
    } else if kind.acts_as_account() {
        read_grants(grants)?
    } else if row.all_access {
        // "Everything I can reach" is a flag, not a grant: live grants beside
        // it are not consulted, so an unreadable one cannot matter.
        ReadGrants::default()
    } else {
        read_grants(grants)?
    };
    let grants = sandbox_admission::refuse_misplaced(kind, grants)?;
    if StoredKind::expected_for(presented) != Some(kind) {
        return Err(Refusal::KindMismatch);
    }
    if row.revoked_at.is_some() {
        return Err(Refusal::Revoked);
    }
    if row
        .expires_at
        .is_some_and(|at| DateTime::<Utc>::from(at) <= now)
    {
        return Err(Refusal::Expired);
    }
    if matches!(links.legacy, LegacyLink::Inactive | LegacyLink::Missing) {
        return Err(Refusal::LegacyKeyInactive);
    }
    let service_account = match kind {
        StoredKind::ServiceAccount | StoredKind::Ci => {
            Some(account_standing(links.account, &grants)?)
        }
        StoredKind::Personal | StoredKind::LegacyKey | StoredKind::SandboxAgent => None,
    };
    Ok(CredentialContext {
        token_id: row.id,
        kind,
        principal_user_id: row.principal_user_id,
        all_access: row.all_access,
        platform: row.platform,
        partner: row.partner,
        name: row.name.clone(),
        display_prefix: row.display_prefix.clone(),
        legacy_api_key_id: row.legacy_api_key_id,
        grants: grants.workspace,
        app_publish: grants.app_publish,
        app_sandbox: grants.app_sandbox,
        blocked_orgs: blocked,
        service_account,
        expires_at: row.expires_at.map(DateTime::<Utc>::from),
    })
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "admission_ci_tests.rs"]
mod ci_tests;

#[cfg(test)]
#[path = "admission_sandbox_tests.rs"]
mod sandbox_tests;

#[cfg(test)]
#[path = "admission_staging_tests.rs"]
mod staging_tests;
