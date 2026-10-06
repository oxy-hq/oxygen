//! `POST /api/auth/tokens/revoke-leaked` — a secret scanner reports tokens it
//! found exposed, and each new-format one is revoked (API-tokens design §8
//! Phase 5).
//!
//! **Public, and rate-limited per client** (`public_rate_limit`): the report
//! is the credential. Each value is judged offline first
//! (`oxy_auth::token::leak::classify`), so a lookalike answers `unknown` and
//! a legacy key `ignored_legacy` without a database read; only a well-formed
//! new-format token is looked up at all.
//!
//! **A legacy key is never revoked here** (§3.5) — not one presented in its
//! own `oxy_<hex>` form, nor a token the legacy endpoint minted. Only its
//! owner can end one.
//!
//! A revocation is the token's `token.revoked` event with reason `leaked` and
//! the report's `source` and `url`, in every org the token reaches, written as
//! the system in the transaction that revokes it. Then the owner — the org's
//! admins, for a service-account token — is mailed. The mail is best-effort:
//! the token is revoked and audited either way.

use axum::Json;
use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use entity::api_tokens;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::{self, AuditContext};
use oxy_auth::token::format::display_prefix;
use oxy_auth::token::leak::{self, Decision, LeakStatus, MAX_REPORTS, Presented};
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::audit::{self as token_audit, Event};
use super::error::TokenError;
use super::handlers::parse;
use super::recipients;
use super::service::reach_of;
use crate::emails::token_leaked::{LeakedEmail, send_leaked_email};
use crate::server::api::public_rate_limit::{self, PublicRoute};

/// The longest `source` / `url` kept from a report: it is unauthenticated text.
const MAX_REPORT_TEXT: usize = 300;

#[derive(Debug, Deserialize)]
pub struct Report {
    pub token: String,
    /// Who found it — `github`, say.
    pub source: Option<String>,
    /// Where it was found.
    pub url: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Outcome {
    pub display_prefix: String,
    pub status: &'static str,
}

fn bounded(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(|s| s.chars().take(MAX_REPORT_TEXT).collect())
}

/// The answer a value gets without the database, or `None` when it is a
/// well-formed new-format token that must be looked up. Takes no connection:
/// what it answers, it answers without one.
pub(crate) fn offline(report: &Report) -> Option<Outcome> {
    let status = match leak::classify(&report.token) {
        Presented::NewFormat => return None,
        Presented::Legacy => LeakStatus::IgnoredLegacy,
        Presented::Unknown => LeakStatus::Unknown,
    };
    Some(Outcome {
        display_prefix: display_prefix(report.token.trim()),
        status: status.as_str(),
    })
}

fn context(headers: &HeaderMap) -> AuditContext {
    AuditContext {
        ip: audit::client_ip(headers),
        user_agent: audit::user_agent(headers),
        request_id: None,
    }
}

/// Revoke and audit in one transaction. `None` when another report won.
async fn revoke(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    row: &api_tokens::Model,
    report: &Report,
) -> Result<Option<api_tokens::Model>, TokenError> {
    let txn = db.begin().await?;
    let Some(revoked) = leak::revoke(&txn, row.id).await? else {
        return Ok(None);
    };
    Event {
        action: token_audit::REVOKED,
        token: &revoked,
        orgs: reach_of(&txn, &revoked).await?,
        detail: json!({
            "reason": leak::REVOKE_REASON,
            "leak_source": bounded(&report.source),
            "leak_url": bounded(&report.url),
        }),
        change: None,
    }
    .record_as_system(&txn, &context(headers))
    .await?;
    txn.commit().await?;
    oxy_auth::token::cache::invalidate_token(revoked.id);
    Ok(Some(revoked))
}

/// Tell whoever answers for the token. Best-effort, and logged.
async fn notify(db: &DatabaseConnection, row: &api_tokens::Model, report: &Report) {
    let to = match recipients::for_token(db, row).await {
        Ok(to) => to,
        Err(e) => {
            tracing::warn!(token_id = %row.id, error = ?e, "leak revocation: no one to mail");
            return;
        }
    };
    let audience = to.audience();
    let mail = LeakedEmail {
        token_name: &row.name,
        display_prefix: &row.display_prefix,
        audience,
        source: report.source.as_deref(),
        url: report.url.as_deref(),
        settings_url: audience.settings_url(),
    };
    for email in &to.emails {
        if let Err(e) = send_leaked_email(email, &mail).await {
            tracing::warn!(token_id = %row.id, error = %e, "leak revocation: mail not sent");
        }
    }
}

/// A well-formed new-format value: look it up and act.
async fn online(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    report: &Report,
) -> Result<Outcome, TokenError> {
    let row = leak::find(db, &report.token).await?;
    let prefix = row.as_ref().map_or_else(
        || display_prefix(report.token.trim()),
        |r| r.display_prefix.clone(),
    );
    let status = match (leak::decide(row.as_ref()), row) {
        (Decision::Answer(status), _) => status,
        (Decision::Revoke, Some(row)) => match revoke(db, headers, &row, report).await? {
            Some(revoked) => {
                notify(db, &revoked, report).await;
                LeakStatus::Revoked
            }
            None => LeakStatus::AlreadyRevoked,
        },
        (Decision::Revoke, None) => LeakStatus::Unknown,
    };
    Ok(Outcome {
        display_prefix: prefix,
        status: status.as_str(),
    })
}

async fn run(headers: &HeaderMap, body: &Bytes) -> Result<Vec<Outcome>, TokenError> {
    let reports: Vec<Report> = parse(body)?;
    if reports.len() > MAX_REPORTS {
        return Err(TokenError::Invalid(format!(
            "a report carries at most {MAX_REPORTS} tokens"
        )));
    }
    let mut db: Option<DatabaseConnection> = None;
    let mut out = Vec::with_capacity(reports.len());
    for report in &reports {
        if let Some(answer) = offline(report) {
            out.push(answer);
            continue;
        }
        if db.is_none() {
            db = Some(establish_connection().await?);
        }
        let db = db.as_ref().expect("connected above");
        out.push(online(db, headers, report).await?);
    }
    Ok(out)
}

/// Revoke reported new-format tokens
pub async fn revoke_leaked(headers: HeaderMap, body: Bytes) -> Response {
    if let Some(retry_after_secs) = public_rate_limit::check(PublicRoute::RevokeLeaked, &headers) {
        return TokenError::RateLimited { retry_after_secs }.into_response();
    }
    match run(&headers, &body).await {
        Ok(outcomes) => Json(outcomes).into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_auth::token::format::generate_personal;

    fn report(token: &str) -> Report {
        Report {
            token: token.to_string(),
            source: None,
            url: None,
        }
    }

    #[test]
    fn a_lookalike_and_a_legacy_key_are_answered_without_a_connection() {
        let legacy = format!("oxy_{}", "ab".repeat(16));
        assert_eq!(
            offline(&report(&legacy)),
            Some(Outcome {
                display_prefix: "oxy_".to_string(),
                status: "ignored_legacy",
            })
        );
        let forged = format!("{}0", &generate_personal().plaintext[..42]);
        assert_eq!(offline(&report(&forged)).unwrap().status, "unknown");
        assert_eq!(offline(&report("ghp_whatever")).unwrap().status, "unknown");
        // A real one needs the lookup.
        assert!(offline(&report(&generate_personal().plaintext)).is_none());
    }

    #[test]
    fn report_text_is_bounded() {
        let long = Some("u".repeat(MAX_REPORT_TEXT * 2));
        assert_eq!(bounded(&long).unwrap().chars().count(), MAX_REPORT_TEXT);
        assert_eq!(bounded(&None), None);
    }
}
