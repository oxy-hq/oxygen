//! What a request asks a service account, or one of its tokens, to be — the
//! bodies of `/api/orgs/{org_id}/service-accounts`, parsed and checked with no
//! database (API-tokens design §3.3).
//!
//! The rules that decide how much an account or its token can do live here:
//!
//! - an account's role is `member` or `admin`. **Never `owner`** — there is no
//!   variant to parse it into;
//! - a token is always grant-bound and never all-access. No `grants` (or an
//!   empty list) means one org-wide grant at the account's own role;
//! - a grant's ceiling may not exceed the account's role, and `owner` is never
//!   valid: the role is the ceiling of anything a grant can give;
//! - every grant is in the account's own org. The body need not name it, and
//!   may not name another.
//!
//! Whether a named workspace belongs to the org needs the database and is the
//! handler's.

use chrono::{DateTime, Utc};
use entity::service_accounts;
use oxy_authz::RoleCeiling;
use serde::Deserialize;
use uuid::Uuid;

use super::access::{GrantInput, Invalid, clean_name, expiry, invalid, present};
use super::personal::GrantSpec;

/// A service-account name is a slug of 2–40 characters (tokens HTTP contract).
pub const ACCOUNT_NAME_MIN_CHARS: usize = 2;
pub const ACCOUNT_NAME_MAX_CHARS: usize = 40;
/// A description is free text, bounded so a row stays a row.
pub const DESCRIPTION_MAX_CHARS: usize = 500;

const KIND_WORKSPACE: &str = "workspace";
const KIND_APP_PUBLISH: &str = "app_publish";

/// A service account's standing in its org. There is no `Owner`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountRole {
    Member,
    Admin,
}

impl AccountRole {
    /// Stable id: `service_accounts.org_role`, and the wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Member => service_accounts::ROLE_MEMBER,
            Self::Admin => service_accounts::ROLE_ADMIN,
        }
    }

    /// `None` for anything else — `owner` included.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            service_accounts::ROLE_MEMBER => Some(Self::Member),
            service_accounts::ROLE_ADMIN => Some(Self::Admin),
            _ => None,
        }
    }

    /// The highest ceiling a grant of this account may carry.
    pub fn ceiling(self) -> RoleCeiling {
        match self {
            Self::Member => RoleCeiling::Member,
            Self::Admin => RoleCeiling::Admin,
        }
    }

    fn from_body(raw: &str) -> Result<Self, Invalid> {
        match Self::parse(raw) {
            Some(role) => Ok(role),
            None if raw == "owner" => invalid("a service account cannot be an owner"),
            None => invalid(format!("'org_role' must be member or admin, not '{raw}'")),
        }
    }
}

/// `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`: lowercase segments joined by single
/// hyphens, the first starting with a letter.
fn is_slug(name: &str) -> bool {
    let lower_or_digit = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let mut segments = name.split('-');
    let first_ok = segments.next().is_some_and(|first| {
        first.starts_with(|c: char| c.is_ascii_lowercase()) && first.chars().all(lower_or_digit)
    });
    first_ok && segments.all(|s| !s.is_empty() && s.chars().all(lower_or_digit))
}

/// A service-account name: a slug of 2–40 characters. Not trimmed or
/// lowercased for the caller — what is stored is what was sent, or nothing.
pub fn clean_account_name(raw: &str) -> Result<String, Invalid> {
    let len = raw.chars().count();
    if !(ACCOUNT_NAME_MIN_CHARS..=ACCOUNT_NAME_MAX_CHARS).contains(&len) || !is_slug(raw) {
        return invalid(format!(
            "'name' must be {ACCOUNT_NAME_MIN_CHARS}–{ACCOUNT_NAME_MAX_CHARS} characters: \
             lowercase letters and digits, starting with a letter, with single hyphens between"
        ));
    }
    Ok(raw.to_string())
}

/// A description: trimmed; blank is none.
fn clean_description(raw: Option<&str>) -> Result<Option<String>, Invalid> {
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if text.chars().count() > DESCRIPTION_MAX_CHARS {
        return invalid(format!(
            "'description' must be at most {DESCRIPTION_MAX_CHARS} characters"
        ));
    }
    Ok(Some(text.to_string()))
}

/// `POST /api/orgs/{org_id}/service-accounts`.
#[derive(Clone, Debug, Deserialize)]
pub struct CreateAccountBody {
    pub name: String,
    pub description: Option<String>,
    pub org_role: Option<String>,
}

/// A new account, checked for shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountWant {
    pub name: String,
    pub description: Option<String>,
    pub role: AccountRole,
}

impl CreateAccountBody {
    pub fn want(&self) -> Result<AccountWant, Invalid> {
        Ok(AccountWant {
            name: clean_account_name(&self.name)?,
            description: clean_description(self.description.as_deref())?,
            role: match self.org_role.as_deref() {
                None => AccountRole::Member,
                Some(raw) => AccountRole::from_body(raw)?,
            },
        })
    }
}

/// `PATCH /api/orgs/{org_id}/service-accounts/{sa_id}`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PatchAccountBody {
    /// Not editable: tokens, audit rows and (Phase 4) `org/name` selectors
    /// name the account by it. Present only so sending it is refused, not
    /// ignored.
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub description: Option<Option<String>>,
    pub org_role: Option<String>,
    pub disabled: Option<bool>,
}

/// An edit, checked for shape. `None` = the body did not touch the field.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountEdit {
    /// `Some(None)` clears it.
    pub description: Option<Option<String>>,
    pub role: Option<AccountRole>,
    pub disabled: Option<bool>,
}

impl PatchAccountBody {
    pub fn edit(&self) -> Result<AccountEdit, Invalid> {
        if self.name.is_some() {
            return invalid("a service account cannot be renamed");
        }
        Ok(AccountEdit {
            description: match &self.description {
                None => None,
                Some(text) => Some(clean_description(text.as_deref())?),
            },
            role: self
                .org_role
                .as_deref()
                .map(AccountRole::from_body)
                .transpose()?,
            disabled: self.disabled,
        })
    }
}

/// `POST /api/orgs/{org_id}/service-accounts/{sa_id}/tokens`.
#[derive(Clone, Debug, Deserialize)]
pub struct CreateAccountTokenBody {
    pub name: String,
    pub grants: Option<Vec<GrantInput>>,
    pub expires_in_days: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub expires_at: Option<Option<String>>,
}

impl CreateAccountTokenBody {
    pub fn name(&self) -> Result<String, Invalid> {
        clean_name(&self.name)
    }

    /// The grants the token is minted with, for an account of `role` in
    /// `org_id`. Never empty.
    pub fn grants(&self, org_id: Uuid, role: AccountRole) -> Result<Vec<GrantSpec>, Invalid> {
        account_grants(org_id, role, self.grants.as_deref().unwrap_or(&[]))
    }

    /// When the token expires; `None` = never. An omitted expiry is 90 days.
    pub fn expires_at(&self, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, Invalid> {
        let at = self.expires_at.as_ref().map(|at| at.as_deref());
        expiry(self.expires_in_days, at, now)
    }
}

/// One grant of a service-account token, checked against the account.
fn account_grant(
    org_id: Uuid,
    role: AccountRole,
    input: &GrantInput,
) -> Result<GrantSpec, Invalid> {
    match input.kind.as_deref().unwrap_or(KIND_WORKSPACE) {
        KIND_WORKSPACE => {}
        KIND_APP_PUBLISH => {
            return invalid("an app_publish grant cannot be put on a service-account token yet");
        }
        other => {
            return invalid(format!(
                "grant 'kind' must be workspace or app_publish, not '{other}'"
            ));
        }
    }
    if input.org_id.is_some_and(|named| named != org_id) {
        return invalid("a service account's grants are in its own organization");
    }
    let ceiling = match input.role_ceiling.as_deref() {
        None => role.ceiling(),
        Some("owner") => return invalid("'owner' is never a valid ceiling for a service account"),
        Some(raw) => match RoleCeiling::parse(raw) {
            Some(ceiling) => ceiling,
            None => {
                return invalid(format!(
                    "'role_ceiling' must be viewer, member or admin, not '{raw}'"
                ));
            }
        },
    };
    if ceiling > role.ceiling() {
        return invalid(format!(
            "'role_ceiling' {} is above the account's org_role {}",
            ceiling.as_str(),
            role.as_str()
        ));
    }
    Ok(GrantSpec {
        org_id,
        workspace_id: input.workspace_id,
        ceiling,
    })
}

/// The grants of a service-account token: what the body names, one per
/// target, or — when it names none — one org-wide grant at the account's role.
/// Naming a target twice keeps the higher ceiling.
pub fn account_grants(
    org_id: Uuid,
    role: AccountRole,
    inputs: &[GrantInput],
) -> Result<Vec<GrantSpec>, Invalid> {
    if inputs.is_empty() {
        return Ok(vec![GrantSpec {
            org_id,
            workspace_id: None,
            ceiling: role.ceiling(),
        }]);
    }
    let mut out: Vec<GrantSpec> = Vec::with_capacity(inputs.len());
    for input in inputs {
        let grant = account_grant(org_id, role, input)?;
        match out
            .iter_mut()
            .find(|g| g.workspace_id == grant.workspace_id)
        {
            Some(have) => have.ceiling = have.ceiling.max(grant.ceiling),
            None => out.push(grant),
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "account_access_tests.rs"]
mod tests;
