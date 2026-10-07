//! The **agent token**: what a mint asks for, checked for shape (API-tokens
//! design, "The agent token (2026-10-07)").
//!
//! An engineer's AI agent asks for a credential of its own and the engineer
//! approves it once in the browser, as an `oxyc login` is approved. What comes
//! back is an ordinary all-access personal token (`oxy_pat_`) that differs
//! from the login's in three ways:
//!
//! - it is the **agent's**: named `agent on <hostname>`, of source
//!   [`source::OXYC_AGENT`], and never what the CLI saves as its login;
//! - it lives **hours**: [`DEFAULT_HOURS`] unless asked, never past
//!   [`MAX_HOURS`];
//! - it is **fixed at mint**: never renamed, widened, extended or regenerated,
//!   only revoked — so a leaked one cannot be kept alive.
//!
//! It carries its owner's staff or partner standing only where the approval
//! said so (`standing`) **and** the owner holds that standing when the code is
//! redeemed. Asking for a standing one does not hold is not an error.
//!
//! This module is the mint's shape and nothing else — no database. What the
//! session may mint, and the audit row, are the handler's.

use chrono::{DateTime, Duration, Utc};
use entity::api_tokens;
use serde_json::{Map, Value, json};

use super::access::{Invalid, NAME_MAX_CHARS, clean_name};
use super::credential::{StoredKind, source};

/// `kind` in the `mint` object that asks for this token.
pub const KIND: &str = "agent";
/// The lifetime a mint gets when it does not ask for one.
pub const DEFAULT_HOURS: i64 = 8;
/// The longest lifetime a mint may ask for: seven days.
pub const MAX_HOURS: i64 = 168;

/// Every field a mint may carry. Anything else is refused rather than quietly
/// ignored: a field that did nothing would read as a narrowing that held.
const FIELDS: [&str; 4] = ["kind", "standing", "expires_in_hours", "name"];

/// The fields of a personal token's create body. An agent token's access is
/// the kind's, not the caller's choice.
const PERSONAL_FIELDS: [&str; 6] = [
    "all_access",
    "platform",
    "partner",
    "grants",
    "expires_in_days",
    "expires_at",
];

/// The field that makes a mint a sandbox agent token's.
const SANDBOX_FIELD: &str = "apps";

/// What a mint asks for, checked for shape and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintRequest {
    pub name: String,
    /// Whether the approval lets the token carry its owner's staff and partner
    /// standing. What it then carries is what the owner holds at redemption.
    pub standing: bool,
    /// 1 to [`MAX_HOURS`].
    pub hours: i64,
}

fn invalid<T>(message: impl Into<String>) -> Result<T, Invalid> {
    Err(Invalid(message.into()))
}

/// Whether a `mint` object asks for an agent token. Anything else — another
/// kind, no kind, not an object — is not this module's to read.
pub fn asked_for(mint: &Value) -> bool {
    matches!(mint.get("kind"), Some(Value::String(kind)) if kind == KIND)
}

/// The first field of `body` an agent token's mint does not take, with why.
fn stray_field(body: &Map<String, Value>) -> Option<String> {
    let field = body.keys().find(|key| !FIELDS.contains(&key.as_str()))?;
    Some(if PERSONAL_FIELDS.contains(&field.as_str()) {
        format!(
            "'{field}' does not apply to an agent token: it is all-access for hours, and \
             carries a standing only when 'standing' is true"
        )
    } else if field == SANDBOX_FIELD {
        format!(
            "'{field}' does not apply to an agent token: a token for an app's sandbox is \
             kind \"{}\"",
            super::sandbox::KIND
        )
    } else {
        format!("'{field}' is not a field of an agent token's mint")
    })
}

fn standing_of(body: &Map<String, Value>) -> Result<bool, Invalid> {
    match body.get("standing") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(standing)) => Ok(*standing),
        Some(_) => invalid("'standing' must be true or false"),
    }
}

fn hours_of(body: &Map<String, Value>) -> Result<i64, Invalid> {
    let hours = match body.get("expires_in_hours") {
        None | Some(Value::Null) => return Ok(DEFAULT_HOURS),
        Some(value) => value.as_i64(),
    };
    match hours {
        Some(hours) if (1..=MAX_HOURS).contains(&hours) => Ok(hours),
        _ => invalid(format!(
            "'expires_in_hours' must be an integer from 1 to {MAX_HOURS}"
        )),
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

/// Parse a mint: the `mint` object of `POST /api/auth/cli/authorize` with
/// `kind: "agent"`, or what a PKCE code stored of one ([`MintRequest::stored`]).
/// `default_name` names the token when the mint does not; `None` makes `name`
/// required, as it is for a stored mint.
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
        name: name_of(body, default_name)?,
        standing: standing_of(body)?,
        hours: hours_of(body)?,
    })
}

impl MintRequest {
    /// When a token minted `now` from this request expires.
    pub fn expires_at(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        now + Duration::hours(self.hours)
    }

    /// The request as the `mint` of a PKCE code stores it, its name resolved.
    /// [`parse`] reads it back.
    pub fn stored(&self) -> Value {
        json!({
            "kind": KIND,
            "name": self.name,
            "standing": self.standing,
            "expires_in_hours": self.hours,
        })
    }
}

/// Whether a row is an agent token: fixed at mint, so the routes that edit a
/// token refuse it. A personal token of any other source is not one.
pub fn is_agent_token(row: &api_tokens::Model) -> bool {
    row.kind == StoredKind::Personal.as_str() && row.source == source::OXYC_AGENT
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
