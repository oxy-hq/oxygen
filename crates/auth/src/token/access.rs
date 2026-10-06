//! What a create or an edit asks a personal token's access to be — the request
//! bodies of `/api/user/tokens`, parsed and checked with no database.
//!
//! Three rules live here because they are the ones that decide how wide a token
//! comes out, and a mistake in any of them mints more than was asked for:
//!
//! - `all_access` defaults to **true** (decided 2026-10-01), so a body that
//!   names grants beside it — by default or explicitly — is refused rather than
//!   quietly minted all-access with the grants dropped;
//! - `all_access = false` needs at least one grant: a token that reaches
//!   nothing is a mistake, not a choice;
//! - on an edit, `all_access = true` **clears** the grants, and `grants`
//!   replaces the set.
//!
//! Whether the caller may *have* what is asked — an org they can reach, a
//! standing they hold — needs the database and is the handler's.

use chrono::{DateTime, Duration, Utc};
use oxy_authz::RoleCeiling;
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use super::personal::{DEFAULT_LIFETIME_DAYS, Settings};

/// A token name is 1–100 characters (tokens HTTP contract).
pub const NAME_MAX_CHARS: usize = 100;
/// The longest lifetime a body may ask for by `expires_in_days`: ten years.
pub const MAX_LIFETIME_DAYS: i64 = 3650;

const KIND_WORKSPACE: &str = "workspace";
const KIND_APP_PUBLISH: &str = "app_publish";

/// Why a body was refused. Always a 400, with this message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid(pub String);

pub(super) fn invalid<T>(message: impl Into<String>) -> Result<T, Invalid> {
    Err(Invalid(message.into()))
}

/// Tells an absent field (`None`) from an explicit `null` (`Some(None)`).
pub(super) fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// One grant as a body names it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct GrantInput {
    /// `workspace` (the default) or `app_publish`.
    pub kind: Option<String>,
    pub org_id: Option<Uuid>,
    /// Absent or `null` = every workspace in the org.
    pub workspace_id: Option<Uuid>,
    /// Defaults to `owner` — no cap.
    pub role_ceiling: Option<String>,
    pub app_id: Option<Uuid>,
}

/// One grant, checked for shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantWant {
    Workspace {
        org_id: Uuid,
        workspace_id: Option<Uuid>,
        ceiling: RoleCeiling,
    },
    /// Only ever a grant the token already holds, sent back so an edit keeps it.
    AppPublish { app_id: Uuid },
}

impl GrantInput {
    fn want(&self) -> Result<GrantWant, Invalid> {
        match self.kind.as_deref().unwrap_or(KIND_WORKSPACE) {
            KIND_WORKSPACE => {
                let Some(org_id) = self.org_id else {
                    return invalid("a workspace grant needs 'org_id'");
                };
                let raw = self.role_ceiling.as_deref().unwrap_or("owner");
                let Some(ceiling) = RoleCeiling::parse(raw) else {
                    return invalid(format!(
                        "'role_ceiling' must be viewer, member, admin or owner, not '{raw}'"
                    ));
                };
                Ok(GrantWant::Workspace {
                    org_id,
                    workspace_id: self.workspace_id,
                    ceiling,
                })
            }
            KIND_APP_PUBLISH => match self.app_id {
                Some(app_id) => Ok(GrantWant::AppPublish { app_id }),
                None => invalid("an app_publish grant needs 'app_id'"),
            },
            other => invalid(format!(
                "grant 'kind' must be workspace or app_publish, not '{other}'"
            )),
        }
    }
}

/// The grants a body names, one per target. Naming a target twice keeps the
/// higher ceiling — what the two grants together would have reached.
pub fn parse_grants(inputs: &[GrantInput]) -> Result<Vec<GrantWant>, Invalid> {
    let mut out: Vec<GrantWant> = Vec::with_capacity(inputs.len());
    for input in inputs {
        let want = input.want()?;
        match same_target(&mut out, &want) {
            Some(GrantWant::Workspace { ceiling: have, .. }) => {
                if let GrantWant::Workspace { ceiling, .. } = want {
                    *have = (*have).max(ceiling);
                }
            }
            Some(GrantWant::AppPublish { .. }) => {}
            None => out.push(want),
        }
    }
    Ok(out)
}

fn same_target<'a>(have: &'a mut [GrantWant], want: &GrantWant) -> Option<&'a mut GrantWant> {
    have.iter_mut().find(|g| match (&**g, want) {
        (
            GrantWant::Workspace {
                org_id: a_org,
                workspace_id: a_ws,
                ..
            },
            GrantWant::Workspace {
                org_id: b_org,
                workspace_id: b_ws,
                ..
            },
        ) => a_org == b_org && a_ws == b_ws,
        (GrantWant::AppPublish { app_id: a }, GrantWant::AppPublish { app_id: b }) => a == b,
        _ => false,
    })
}

/// The access a new token is asked to have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub all_access: bool,
    pub platform: bool,
    pub partner: bool,
    /// Empty when `all_access`.
    pub grants: Vec<GrantWant>,
}

/// `POST /api/user/tokens`.
#[derive(Clone, Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    pub all_access: Option<bool>,
    pub platform: Option<bool>,
    pub partner: Option<bool>,
    pub grants: Option<Vec<GrantInput>>,
    pub expires_in_days: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub expires_at: Option<Option<String>>,
}

impl CreateBody {
    pub fn name(&self) -> Result<String, Invalid> {
        clean_name(&self.name)
    }

    pub fn access(&self) -> Result<Access, Invalid> {
        let all_access = self.all_access.unwrap_or(true);
        let grants = parse_grants(self.grants.as_deref().unwrap_or(&[]))?;
        // An `app_publish` grant is only ever one a token already holds, kept
        // by an edit ([`GrantWant::AppPublish`]). A new personal token holds
        // none, and no route puts one on it: say so here, before anything is
        // read, rather than as the edit's "holds no such grant".
        if grants
            .iter()
            .any(|g| matches!(g, GrantWant::AppPublish { .. }))
        {
            return invalid(NO_APP_PUBLISH);
        }
        match (all_access, grants.is_empty()) {
            (true, false) => {
                return invalid("'grants' narrows a token: send \"all_access\": false with them");
            }
            (false, true) => return invalid(NEEDS_GRANTS),
            _ => {}
        }
        Ok(Access {
            all_access,
            platform: self.platform.unwrap_or(false),
            partner: self.partner.unwrap_or(false),
            grants,
        })
    }

    /// When the token expires; `None` = never. An omitted expiry is 90 days.
    pub fn expires_at(&self, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, Invalid> {
        let at = self.expires_at.as_ref().map(|at| at.as_deref());
        expiry(self.expires_in_days, at, now)
    }
}

const NEEDS_GRANTS: &str =
    "a token without all access needs at least one grant: send 'grants', or \"all_access\": true";

/// What a create that names an `app_publish` grant is told.
pub const NO_APP_PUBLISH: &str = "a personal token cannot be created with an app_publish grant: \
     narrow it with workspace grants, or publish one app from CI through trusted access \
     (a service account's trust policy)";

/// What an edit does to the grant set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantsEdit {
    /// The body did not touch them.
    Keep,
    /// The token is all-access after the edit: its live grants go.
    Clear,
    /// `grants` replaces the live set.
    Replace(Vec<GrantWant>),
}

/// An edit, resolved against the token as it is stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub settings: Settings,
    pub grants: GrantsEdit,
    /// The body asked for `platform` / `partner` to be **on**. Asking needs the
    /// standing even when the token already carries the flag, so a token keeps
    /// a standing its owner lost only until its next edit.
    pub asks_platform: bool,
    pub asks_partner: bool,
}

/// `PATCH /api/user/tokens/{id}`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PatchBody {
    pub name: Option<String>,
    pub all_access: Option<bool>,
    pub platform: Option<bool>,
    pub partner: Option<bool>,
    pub grants: Option<Vec<GrantInput>>,
}

impl PatchBody {
    pub fn name(&self) -> Result<Option<String>, Invalid> {
        self.name.as_deref().map(clean_name).transpose()
    }

    pub fn edit(&self, current: &Settings) -> Result<Edit, Invalid> {
        let all_access = self.all_access.unwrap_or(current.all_access);
        let wanted = self.grants.as_deref().map(parse_grants).transpose()?;
        let grants = match (all_access, wanted) {
            (true, Some(grants)) if !grants.is_empty() => {
                return invalid("'grants' narrows a token: send \"all_access\": false with them");
            }
            (true, _) => GrantsEdit::Clear,
            (false, Some(grants)) if grants.is_empty() => return invalid(NEEDS_GRANTS),
            (false, Some(grants)) => GrantsEdit::Replace(grants),
            // Narrowing an all-access token names what it narrows to.
            (false, None) if current.all_access => return invalid(NEEDS_GRANTS),
            (false, None) => GrantsEdit::Keep,
        };
        Ok(Edit {
            settings: Settings {
                name: self.name()?.unwrap_or_else(|| current.name.clone()),
                all_access,
                platform: self.platform.unwrap_or(current.platform),
                partner: self.partner.unwrap_or(current.partner),
            },
            grants,
            asks_platform: self.platform == Some(true),
            asks_partner: self.partner == Some(true),
        })
    }
}

/// A token name: trimmed, 1–100 characters.
pub fn clean_name(raw: &str) -> Result<String, Invalid> {
    let name = raw.trim();
    match name.chars().count() {
        0 => invalid("'name' must not be empty"),
        n if n > NAME_MAX_CHARS => invalid(format!(
            "'name' must be at most {NAME_MAX_CHARS} characters"
        )),
        _ => Ok(name.to_string()),
    }
}

/// The expiry a create body asks for: `{expires_in_days}`, `{expires_at}`,
/// `{expires_at: null}` (never), or neither — 90 days.
pub fn expiry(
    expires_in_days: Option<i64>,
    expires_at: Option<Option<&str>>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Invalid> {
    match (expires_in_days, expires_at) {
        (Some(_), Some(_)) => invalid("send only one of 'expires_in_days' and 'expires_at'"),
        (None, None) => Ok(Some(now + Duration::days(DEFAULT_LIFETIME_DAYS))),
        (Some(days), None) if (1..=MAX_LIFETIME_DAYS).contains(&days) => {
            Ok(Some(now + Duration::days(days)))
        }
        (Some(_), None) => invalid(format!(
            "'expires_in_days' must be an integer from 1 to {MAX_LIFETIME_DAYS}"
        )),
        (None, Some(None)) => Ok(None),
        (None, Some(Some(raw))) => {
            let Ok(at) = DateTime::parse_from_rfc3339(raw) else {
                return invalid("'expires_at' must be an RFC 3339 timestamp or null");
            };
            let at = at.with_timezone(&Utc);
            if at <= now {
                return invalid("'expires_at' must be in the future");
            }
            Ok(Some(at))
        }
    }
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
