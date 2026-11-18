//! A **service-account token** asked for through `oxyc`: what the mint asks
//! for, checked for shape (`oxyc tokens create --org-service-account`).
//!
//! The org's API access page creates a service account and mints its
//! `oxy_sat_` token in two clicks, under an org admin's browser session. This
//! is the same pair of writes asked for from the CLI: oxyc opens the approval
//! page, the org admin approves once under their own session, and the CLI
//! redeems a one-time code for the token — the PKCE exchange `oxyc login` and
//! the agent token use.
//!
//! The mint names:
//!
//! - `org` — the org, by slug or id. What is stored once the approval has
//!   checked it is the id;
//! - `account` — the service account's name, a slug. An account of that name
//!   in the org is reused; otherwise one is created;
//! - `role` — `member` (the default) or `admin`, never `owner`. An existing
//!   account whose role differs is refused, never changed: the approver
//!   changes a role on the API access page, where it is said what that does to
//!   the account's other tokens;
//! - `name` — the token's name ([`default_name`] when the CLI sent none);
//! - `expires_in_days` — 1 to [`MAX_LIFETIME_DAYS`], or the default lifetime
//!   of a token made on the API access page.
//!
//! The token carries **no grant the mint names**: it gets the one org-wide
//! grant at the account's role that `account_access::account_grants` gives a
//! token created with none.
//!
//! This module is the mint's shape and nothing else — no database. Who may
//! approve it, the account, the audit rows and the token are the handler's.

use serde_json::{Map, Value, json};

use super::access::{Invalid, MAX_LIFETIME_DAYS, NAME_MAX_CHARS, clean_name, invalid};
use super::account_access::{AccountRole, clean_account_name};
use super::personal::DEFAULT_LIFETIME_DAYS;

/// `kind` in the `mint` object that asks for this token.
pub const KIND: &str = "service_account";

/// The longest `org` a mint may carry: a slug or a UUID, with room to spare.
pub const ORG_MAX_CHARS: usize = 100;

/// Every field a mint may carry. Anything else is refused rather than quietly
/// ignored: a field that did nothing would read as a narrowing that held.
const FIELDS: [&str; 6] = ["kind", "org", "account", "role", "name", "expires_in_days"];

/// What a mint asks for, checked for shape and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintRequest {
    /// The org as the mint names it: a slug or an id. A stored mint's is the id.
    pub org: String,
    /// The service account's name.
    pub account: String,
    /// The role a new account is created with, and the one an existing
    /// account must already have.
    pub role: AccountRole,
    /// The token's name.
    pub name: String,
    /// 1 to [`MAX_LIFETIME_DAYS`].
    pub days: i64,
}

/// The name a token gets when its mint sent none.
pub fn default_name(hostname: &str) -> String {
    format!("minted by oxyc on {hostname}")
}

/// Whether a `mint` object asks for a service-account token. Anything else —
/// another kind, no kind, not an object — is not this module's to read.
pub fn asked_for(mint: &Value) -> bool {
    matches!(mint.get("kind"), Some(Value::String(kind)) if kind == KIND)
}

/// The first field of `body` this mint does not take, with why.
fn stray_field(body: &Map<String, Value>) -> Option<String> {
    let field = body.keys().find(|key| !FIELDS.contains(&key.as_str()))?;
    Some(match field.as_str() {
        "grants" | "all_access" => format!(
            "'{field}' does not apply to a service-account token minted this way: it has one \
             org-wide grant at the account's role"
        ),
        "platform" | "partner" | "standing" => {
            format!("'{field}' does not apply to a service-account token: it carries no standing")
        }
        "expires_at" | "expires_in_hours" => {
            format!("'{field}' does not apply here: send 'expires_in_days'")
        }
        _ => format!("'{field}' is not a field of a service-account token's mint"),
    })
}

fn string_of<'a>(body: &'a Map<String, Value>, field: &str) -> Result<&'a str, Invalid> {
    match body.get(field) {
        Some(Value::String(text)) => Ok(text),
        _ => invalid(format!("'{field}' must be a string")),
    }
}

fn org_of(body: &Map<String, Value>) -> Result<String, Invalid> {
    let org = string_of(body, "org")?.trim();
    if org.is_empty() || org.chars().count() > ORG_MAX_CHARS || org.chars().any(char::is_control) {
        return invalid(format!(
            "'org' must be an organization's slug or id, at most {ORG_MAX_CHARS} characters"
        ));
    }
    Ok(org.to_string())
}

fn account_of(body: &Map<String, Value>) -> Result<String, Invalid> {
    clean_account_name(string_of(body, "account")?)
        .map_err(|Invalid(why)| Invalid(why.replace("'name'", "'account'")))
}

fn role_of(body: &Map<String, Value>) -> Result<AccountRole, Invalid> {
    match body.get("role") {
        None | Some(Value::Null) => Ok(AccountRole::Member),
        Some(Value::String(raw)) => match AccountRole::parse(raw) {
            Some(role) => Ok(role),
            None if raw == "owner" => invalid("a service account cannot be an owner"),
            None => invalid(format!("'role' must be member or admin, not '{raw}'")),
        },
        Some(_) => invalid("'role' must be member or admin"),
    }
}

fn name_of(body: &Map<String, Value>, default_name: Option<&str>) -> Result<String, Invalid> {
    match (body.get("name"), default_name) {
        (Some(Value::String(name)), _) => clean_name(name),
        // The default is built from the hostname, which may run to 255
        // characters: say that a name is needed, not that one was too long.
        (None | Some(Value::Null), Some(default)) => clean_name(default).map_err(|_| {
            Invalid(format!(
                "the default name '{default}' is over {NAME_MAX_CHARS} characters: send 'name'"
            ))
        }),
        _ => invalid(format!(
            "'name' must be a string of 1 to {NAME_MAX_CHARS} characters"
        )),
    }
}

fn days_of(body: &Map<String, Value>) -> Result<i64, Invalid> {
    let days = match body.get("expires_in_days") {
        None | Some(Value::Null) => return Ok(DEFAULT_LIFETIME_DAYS),
        Some(value) => value.as_i64(),
    };
    match days {
        Some(days) if (1..=MAX_LIFETIME_DAYS).contains(&days) => Ok(days),
        _ => invalid(format!(
            "'expires_in_days' must be an integer from 1 to {MAX_LIFETIME_DAYS}"
        )),
    }
}

/// Parse a mint: the `mint` object of `POST /api/auth/cli/authorize` with
/// `kind: "service_account"`, or what a code stored of one
/// ([`MintRequest::stored`]). `default_name` names the token when the mint
/// does not; `None` makes `name` required, as it is for a stored mint.
pub fn parse(mint: &Value, default_name: Option<&str>) -> Result<MintRequest, Invalid> {
    let Value::Object(body) = mint else {
        return invalid("'mint' must be a JSON object");
    };
    if !asked_for(mint) {
        return invalid(format!("'kind' must be \"{KIND}\""));
    }
    if let Some(why) = stray_field(body) {
        return invalid(why);
    }
    Ok(MintRequest {
        org: org_of(body)?,
        account: account_of(body)?,
        role: role_of(body)?,
        name: name_of(body, default_name)?,
        days: days_of(body)?,
    })
}

impl MintRequest {
    /// The request as a code or a device-code request stores it once the
    /// approval has checked it: the org by `org_id`, the name and the lifetime
    /// resolved. [`parse`] reads it back.
    pub fn stored(&self, org_id: uuid::Uuid) -> Value {
        json!({
            "kind": KIND,
            "org": org_id.to_string(),
            "account": self.account,
            "role": self.role.as_str(),
            "name": self.name,
            "expires_in_days": self.days,
        })
    }
}

#[cfg(test)]
#[path = "service_account_mint_tests.rs"]
mod tests;
