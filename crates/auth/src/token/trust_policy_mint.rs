//! A **trust policy** asked for through `oxyc init-ci`: what the approval asks
//! for, checked for shape.
//!
//! `oxyc init-ci` writes a GitHub Actions workflow that publishes a custom app
//! with no stored secret: the job trades its OIDC token for a fifteen-minute
//! token acting as a service account. What lets it is a trust policy on that
//! account naming the repository, the workflow file and the job's environment.
//! Creating the account and the policy is browser-session-only, so `init-ci`
//! asks for both through the same approval `oxyc` mints its tokens with
//! (`/cli-auth`, then the PKCE exchange or a device-code poll) — and **no token
//! is minted**: the exchange answers with the account and the policy.
//!
//! The approval names:
//!
//! - `org` — the org, by slug or id; stored as the id once checked;
//! - `account` — the service account's name. With `create_account`, an account
//!   of that name is reused or else created as a `member`; without it, it must
//!   exist;
//! - `app` — the one app of that org the policy may publish, by id or by
//!   slug (an asker who may not list the org's apps knows only the slug);
//!   stored as the id once checked. The policy's only grant is `app_publish`
//!   on it: the narrowest that does the job, and not the asker's choice;
//! - `repository`, `workflow_path`, `environment` — what a run must be to act
//!   as the account, held to the trust-policy body rules.
//!
//! **Never the repository's ids.** A run is matched on GitHub's numeric ids,
//! not on the name, and the approver is shown the name only — so ids the asker
//! chose would decide which repository is trusted with nobody having checked
//! them. The deployment reads them from GitHub; a repository it cannot look up
//! is refused, and an admin registers that one on the account's Trusted access
//! page, typing the ids in their own session.
//!
//! This module is the shape and nothing else — no database, no GitHub.

use serde_json::{Map, Value};
use uuid::Uuid;

use super::access::{GrantInput, Invalid, invalid};
use super::trust_policy_access::CreatePolicyBody;

/// `kind` in the `mint` object that asks for a trust policy.
pub const KIND: &str = "trust_policy";

/// The longest `org` or `app` an approval may carry: a slug or a UUID, with room to spare.
pub const ORG_MAX_CHARS: usize = 100;

const KIND_APP_PUBLISH: &str = "app_publish";

/// Every field the approval may carry. Anything else is refused rather than
/// quietly ignored: a field that did nothing would read as a narrowing that held.
const FIELDS: [&str; 8] = [
    "kind",
    "org",
    "account",
    "create_account",
    "app",
    "repository",
    "workflow_path",
    "environment",
];

/// What an approval asks for, checked for shape and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyRequest {
    /// The org as named: a slug or an id.
    pub org: String,
    pub account: String,
    /// Create the account when the org has none of that name.
    pub create_account: bool,
    /// The app the policy may publish, as named: an id or a slug.
    pub app: String,
    pub repository: String,
    pub workflow_path: String,
    pub environment: Option<String>,
}

/// Whether a `mint` object asks for a trust policy. Anything else — another
/// kind, no kind, not an object — is not this module's to read.
pub fn asked_for(mint: &Value) -> bool {
    matches!(mint.get("kind"), Some(Value::String(kind)) if kind == KIND)
}

fn stray_field(body: &Map<String, Value>) -> Option<String> {
    let field = body.keys().find(|key| !FIELDS.contains(&key.as_str()))?;
    Some(match field.as_str() {
        "grants" => format!(
            "'{field}' does not apply: the policy's one grant is publishing the app it names"
        ),
        "role" | "name" | "expires_in_days" | "expires_in_hours" => {
            format!("'{field}' does not apply: approving a trust policy mints no token")
        }
        "repository_id" | "repository_owner_id" => format!(
            "'{field}' is never taken from an approval: the deployment reads the \
             repository's ids from GitHub, since the approver is shown only its name"
        ),
        _ => format!("'{field}' is not a field of a trust policy's approval"),
    })
}

fn text<'a>(body: &'a Map<String, Value>, field: &str) -> Result<&'a str, Invalid> {
    match body.get(field) {
        Some(Value::String(text)) => Ok(text),
        _ => invalid(format!("'{field}' must be a string")),
    }
}

fn org_of(body: &Map<String, Value>) -> Result<String, Invalid> {
    let org = text(body, "org")?.trim();
    if org.is_empty() || org.chars().count() > ORG_MAX_CHARS || org.chars().any(char::is_control) {
        return invalid(format!(
            "'org' must be an organization's slug or id, at most {ORG_MAX_CHARS} characters"
        ));
    }
    Ok(org.to_string())
}

fn account_of(body: &Map<String, Value>) -> Result<String, Invalid> {
    super::account_access::clean_account_name(text(body, "account")?)
        .map_err(|Invalid(why)| Invalid(why.replace("'name'", "'account'")))
}

fn create_of(body: &Map<String, Value>) -> Result<bool, Invalid> {
    match body.get("create_account") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(create)) => Ok(*create),
        Some(_) => invalid("'create_account' must be true or false"),
    }
}

fn app_of(body: &Map<String, Value>) -> Result<String, Invalid> {
    let app = text(body, "app")?.trim();
    if app.is_empty() || app.chars().count() > ORG_MAX_CHARS || app.chars().any(char::is_control) {
        return invalid(format!(
            "'app' must be an app's slug or id, at most {ORG_MAX_CHARS} characters"
        ));
    }
    Ok(app.to_string())
}

fn environment_of(body: &Map<String, Value>) -> Result<Option<String>, Invalid> {
    match body.get("environment") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(env)) => Ok(Some(env.clone())),
        Some(_) => invalid("'environment' must be a string"),
    }
}

/// Parse an approval: the `mint` object of `POST /api/auth/cli/authorize` with
/// `kind: "trust_policy"`. The repository, workflow and environment are held
/// to the trust-policy body rules, as the create route holds them. What a code
/// stores once the approval is carried out is not this: it is the rows the
/// approval wrote, read back by `trust_policy_approval::redeem`.
pub fn parse(mint: &Value) -> Result<PolicyRequest, Invalid> {
    let Value::Object(body) = mint else {
        return invalid("'mint' must be a JSON object");
    };
    if !asked_for(mint) {
        return invalid(format!("'kind' must be \"{KIND}\""));
    }
    if let Some(why) = stray_field(body) {
        return invalid(why);
    }
    let request = PolicyRequest {
        org: org_of(body)?,
        account: account_of(body)?,
        create_account: create_of(body)?,
        app: app_of(body)?,
        repository: text(body, "repository")?.to_string(),
        workflow_path: text(body, "workflow_path")?.to_string(),
        environment: environment_of(body)?,
    };
    // The body rules, with an org, a role and an app that cannot change them:
    // the shape is checked here, and again against the account by the create.
    request
        .policy_body(Uuid::nil())
        .want(Uuid::nil(), super::account_access::AccountRole::Member)?;
    Ok(request)
}

impl PolicyRequest {
    /// The create body of the policy this asks for, for the app it names
    /// resolved to `app_id`: its one grant is publishing that app, nothing
    /// about refs or runners is loosened, and the repository's ids are left for
    /// the deployment to read from GitHub.
    pub fn policy_body(&self, app_id: Uuid) -> CreatePolicyBody {
        CreatePolicyBody {
            repository: self.repository.clone(),
            workflow_path: self.workflow_path.clone(),
            environment: self.environment.clone(),
            ref_pattern: None,
            allow_self_hosted: None,
            grants: Some(vec![GrantInput {
                kind: Some(KIND_APP_PUBLISH.to_string()),
                app_id: Some(app_id),
                ..GrantInput::default()
            }]),
            repository_id: None,
            repository_owner_id: None,
        }
    }
}

#[cfg(test)]
#[path = "trust_policy_mint_tests.rs"]
mod tests;
