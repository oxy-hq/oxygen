//! Trusted access: `POST /api/auth/oidc/exchange` (API-tokens design §3.4).
//!
//! A GitHub Actions run trades the OIDC token GitHub signed for it for a
//! 15-minute `oxy_ci_` token that acts as a service account. **Public by
//! construction**: the JWT is the credential, and the claims are what gate it.
//! The repository stores no secret.
//!
//! The steps, each failing closed:
//!
//! 1. **The envelope** (`oxy_auth::github_oidc::verify`): RS256 against
//!    GitHub's keys, the issuer, **this deployment's audience** ([`audience`]:
//!    `oxy:<host>` of its public URL, which a client derives from the URL it
//!    talks to and is never told) and no other — so a token asked for another
//!    deployment cannot be replayed here, and an `oxy-publish` token,
//!    requested for a publish, can never be traded for this — `exp`/`nbf`, and
//!    the `jti` burned.
//! 2. **The policies of the account the run named**, on the run's repository
//!    by both numeric ids, that are live: not disabled, on an account that is
//!    not disabled. The account is named **by its id**, and the id is required
//!    — a request without one is 400 `service_account_required` before the
//!    token is read. No other account's policy is ever loaded, so a policy
//!    another org registered on the same repository has no say in the answer;
//!    and an id cannot be re-pointed, where `<org_slug>/<name>` could: a slug
//!    is free for anyone once its org renames or is deleted. An id that names
//!    no account answers exactly as one with no matching policy does.
//! 3. **The decision** (`oxy_auth::token::exchange`): pure, deterministic,
//!    never a union. Several matching policies of the account: the oldest.
//!    While the org requires an environment (its token policy, read here on
//!    every exchange), a policy that names none does not match: 403
//!    `missing_environment`.
//! 4. **The mint** ([`mint`], `oxy_auth::token::ci_mint`): in one transaction,
//!    the policy and its account are read again **under row locks a disable
//!    has to wait for** and the run is matched against them as they are now —
//!    so a policy disabled since step 2 mints nothing, and one disabled while
//!    this is in flight revokes the token this commits. Then the policy's
//!    grants, capped by the account's standing now; the policy and the
//!    verified claims on the row; `oidc.token_exchanged` in the org's chain.
//!
//! A refusal is counted by reason and audited unchained ([`reject`]).
//!
//! The legacy trusted-publishing exchange
//! (`POST /api/customer-apps/publish/oidc-exchange`, audience `oxy-publish`)
//! is a separate route with its own table, and is untouched by any of this.

pub mod audience;
mod mint;
mod reject;

use axum::Json;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use oxy::database::client::establish_connection;
use oxy_auth::github_oidc::{self, GithubOidcClaims, OidcError};
use oxy_auth::token::ci_mint::PolicyMint;
use oxy_auth::token::exchange::{self, Decision};
use oxy_auth::token::personal::{self, Minted};
use oxy_auth::token::trust_policy::{self, Candidate};
use oxy_auth::token::trust_policy_access::RepoIds;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use self::reject::Rejection;
use crate::server::api::github_oidc_keys::github_keys;
use crate::server::api::public_rate_limit::{self, PublicRoute};
use crate::server::api::user_tokens::dto::{GrantDto, grant_dto};
use crate::server::api::user_tokens::error::TokenError;
use crate::server::api::user_tokens::view;

#[derive(Deserialize)]
struct ExchangeBody {
    /// The GitHub Actions OIDC JWT, requested for this deployment's audience.
    token: String,
    /// The id of the account to act as. Required. Read as any JSON value so
    /// that one of the wrong type is answered as a missing id, not as an
    /// unreadable body.
    #[serde(default)]
    service_account: Value,
}

/// A body that can be answered: a token, and the account it is for.
#[derive(Debug)]
struct Request {
    token: String,
    /// The account's id — `service_accounts.user_id`, the `id` the org API
    /// returns and the audit row records as `service_account_id`.
    service_account: Uuid,
}

/// The 200 body. `token` is shown once and never stored.
#[derive(Debug, Serialize)]
pub struct ExchangeResponse {
    pub token: String,
    pub token_id: Uuid,
    pub expires_at: DateTime<Utc>,
    /// `<org_slug>/<name>` of the account the token acts as.
    pub service_account: String,
    pub grants: Vec<GrantDto>,
}

/// Why the exchange did not mint.
enum Failure {
    /// Refused, with the verified claims when the token itself held.
    Refused(Rejection, Option<Value>),
    /// A fault of ours. Logged, never returned.
    Internal(String),
}

impl<E: std::fmt::Display> From<E> for Failure {
    fn from(e: E) -> Self {
        Self::Internal(e.to_string())
    }
}

const BODY_SHAPE: &str = "the body must be {\"token\": \"<github oidc jwt>\", \"service_account\": \"<service account id>\"}";

/// The account id a body names, when it names one. Anything else — absent,
/// null, blank, not a string, or a string that is not a UUID, which is what
/// `acme/deployer` is — names none.
fn account_id(named: &Value) -> Option<Uuid> {
    Uuid::try_parse(named.as_str()?.trim()).ok()
}

/// Read the body. Runs before the envelope, so a request that could never
/// have been answered does not spend the token's `jti`.
fn parse(body: &[u8]) -> Result<Request, Failure> {
    let refuse = |message| Failure::Refused(Rejection::Malformed(message), None);
    let request: ExchangeBody = serde_json::from_slice(body).map_err(|_| refuse(BODY_SHAPE))?;
    if request.token.trim().is_empty() {
        return Err(refuse("'token' is the GitHub Actions OIDC token"));
    }
    // No account named: nothing is matched on the run's behalf. The workflow
    // says which account it trusts, by the one name for it that cannot be
    // handed to somebody else, or it gets none.
    let Some(service_account) = account_id(&request.service_account) else {
        return Err(Failure::Refused(Rejection::ServiceAccountRequired, None));
    };
    Ok(Request {
        service_account,
        token: request.token,
    })
}

/// Step 1: the envelope. A database failure burning the `jti` is ours, not a
/// refusal.
async fn verified(db: &DatabaseConnection, token: &str) -> Result<GithubOidcClaims, Failure> {
    // This deployment's audience, and only it: a token asked for another
    // deployment — whose `jti` this database has never seen — is refused.
    let audience = audience::audience();
    match github_oidc::verify_token(db, github_keys(), token.trim(), &audience).await {
        Ok(claims) => Ok(claims),
        Err(OidcError::Db(detail)) => Err(Failure::Internal(detail)),
        Err(e) => {
            tracing::warn!(error = ?e, "oidc exchange: the token did not verify");
            let code = e.code().unwrap_or("invalid_token");
            Err(Failure::Refused(Rejection::Envelope(code), None))
        }
    }
}

/// Steps 2 and 3: the policy of the `named` account this run mints from.
async fn chosen(
    db: &DatabaseConnection,
    claims: &GithubOidcClaims,
    named: Uuid,
) -> Result<Candidate, Failure> {
    let refuse = |rejection| Failure::Refused(rejection, Some(claims.recorded()));
    // A token GitHub signed always carries both ids as numbers. One that does
    // not is not a token this exchange can match on anything but names.
    let (Some(repository_id), Some(repository_owner_id)) = (claims.repo_id(), claims.owner_id())
    else {
        return Err(refuse(Rejection::Envelope("invalid_token")));
    };
    let ids = RepoIds {
        repository_id,
        repository_owner_id,
    };
    let candidates = trust_policy::candidates(db, named, ids).await?;
    // The org's say NOW, not when the policy was written: turning the
    // requirement back on stops its environment-less policies at once. One
    // account, so one org.
    let environment_required = match candidates.first() {
        Some(first) => trust_policy::environment_required(db, first.account.org_id).await?,
        None => false,
    };
    match exchange::decide(claims, candidates, environment_required) {
        Decision::Mint(candidate) => Ok(*candidate),
        Decision::Reject(reject) => Err(refuse(Rejection::Claims(reject))),
    }
}

async fn response(
    db: &DatabaseConnection,
    candidate: &Candidate,
    minted: Minted,
) -> Result<ExchangeResponse, Failure> {
    let grants = personal::grants_for(db, &[minted.row.id]).await?;
    let names = view::names_for(db, &grants)
        .await
        .map_err(|e| Failure::Internal(format!("{e:?}")))?;
    let expires_at = minted
        .row
        .expires_at
        .map(DateTime::<Utc>::from)
        .ok_or_else(|| Failure::Internal("a ci token was minted with no expiry".into()))?;
    Ok(ExchangeResponse {
        token: minted.secret,
        token_id: minted.row.id,
        expires_at,
        service_account: candidate.account_name(),
        grants: grants.iter().map(|g| grant_dto(g, &names)).collect(),
    })
}

async fn run(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<ExchangeResponse, Failure> {
    let request = parse(body)?;
    let claims = verified(db, &request.token).await?;
    let picked = chosen(db, &claims, request.service_account).await?;
    // What it minted from is what it found under lock, not what was chosen.
    let PolicyMint { minted, candidate } = mint::mint(db, headers, &claims, &picked).await?;
    tracing::info!(
        token_id = %minted.row.id,
        trust_policy_id = %candidate.policy.id,
        repository = %claims.repository,
        "oidc exchange: minted a ci token"
    );
    response(db, &candidate, minted).await
}

fn internal(detail: &str) -> Response {
    tracing::error!(error = %detail, "oidc exchange failed");
    let body = json!({ "error": "internal server error" });
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}

/// Exchange a GitHub Actions OIDC token for a 15-minute `oxy_ci_` token
pub async fn exchange(headers: HeaderMap, body: Bytes) -> Response {
    // Public: a per-client budget stands in front of the verifier.
    if let Some(retry_after_secs) = public_rate_limit::check(PublicRoute::OidcExchange, &headers) {
        return TokenError::RateLimited { retry_after_secs }.into_response();
    }
    let db = match establish_connection().await {
        Ok(db) => db,
        Err(e) => return internal(&e.to_string()),
    };
    match run(&db, &headers, &body).await {
        Ok(minted) => Json(minted).into_response(),
        Err(Failure::Internal(detail)) => internal(&detail),
        Err(Failure::Refused(rejection, claims)) => {
            rejection.record(&db, &headers, claims.as_ref()).await;
            rejection.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(body: &str) -> Option<Rejection> {
        match parse(body.as_bytes()) {
            Err(Failure::Refused(rejection, None)) => Some(rejection),
            _ => None,
        }
    }

    #[test]
    fn a_body_with_a_token_and_an_account_id_parses() {
        const ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
        for named in [ID.to_string(), format!(" {ID} "), ID.to_uppercase()] {
            let body = format!(r#"{{"token":"a.b.c","service_account":"{named}"}}"#);
            assert_eq!(
                parse(body.as_bytes()).ok().map(|b| b.service_account),
                Uuid::parse_str(ID).ok(),
                "{named:?}"
            );
        }
    }

    #[test]
    fn a_body_that_names_no_account_id_is_refused_before_the_token_is_spent() {
        // The exchange matches nothing on a run's behalf: no account, no
        // lookup. `parse` runs before the envelope, so the `jti` is kept.
        for named in [
            // Absent, null, blank.
            None,
            Some("null"),
            Some(r#""""#),
            Some(r#""  ""#),
            // Not a string at all.
            Some("7"),
            Some("true"),
            Some(r#"["3f2504e0-4f89-41d3-9a0c-0305e82c3301"]"#),
            // The readable name. It was the accepted form once, and it is the
            // one that can be re-pointed: a slug is free for anyone after its
            // org renames or is deleted. Not a second accepted form.
            Some(r#""acme/deployer""#),
            Some(r#""deployer""#),
            Some(r#""acme/""#),
            // Nearly an id.
            Some(r#""3f2504e0-4f89-41d3-9a0c""#),
            Some(r#""acme/3f2504e0-4f89-41d3-9a0c-0305e82c3301""#),
        ] {
            let body = match named {
                Some(named) => format!(r#"{{"token":"a.b.c","service_account":{named}}}"#),
                None => r#"{"token":"a.b.c"}"#.to_string(),
            };
            assert_eq!(
                refusal(&body),
                Some(Rejection::ServiceAccountRequired),
                "{body}"
            );
        }
    }

    #[test]
    fn a_body_that_names_no_token_is_malformed() {
        // Whatever it says about the account: the token is asked for first.
        for body in [
            "",
            "{}",
            r#"{"token":""}"#,
            r#"{"token":"  "}"#,
            r#"{"service_account":"3f2504e0-4f89-41d3-9a0c-0305e82c3301"}"#,
            "not json",
        ] {
            assert!(
                matches!(refusal(body), Some(Rejection::Malformed(_))),
                "{body:?}"
            );
        }
    }
}
