//! Why an exchange was refused: the answer on the wire, the metric, and the
//! audit row (API-tokens design §3.7).
//!
//! One enum, three renderings, so they cannot disagree about a reason:
//!
//! | Status | `code` |
//! | --- | --- |
//! | 400 | — (a body that names no token) |
//! | 400 | `service_account_required` (no account id: absent, or not a UUID) |
//! | 401 | `invalid_token` `wrong_audience` `expired` `replayed` |
//! | 403 | `pull_request_target` `self_hosted_runner` `missing_environment` `no_matching_policy` |
//!
//! A refused exchange usually matches no org, so its audit row is **unchained**
//! and best-effort. It carries the reason and, once the token itself verified,
//! the run's verified claims — never the JWT.

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use oxy_app_core::audit::{self, ActorType, AuditContext, AuditEntry};
use oxy_auth::github_oidc::ClaimReject;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};

const REJECTED: &str = "oidc.exchange_rejected";
/// The actor of a refused exchange: no user authenticated, by definition.
const ACTOR: &str = "github-actions-oidc";
const MALFORMED: &str = "malformed_request";
const WRONG_AUDIENCE: &str = "wrong_audience";
const ACCOUNT_REQUIRED: &str = "service_account_required";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Rejection {
    /// The request itself is unusable: not JSON, or no token.
    Malformed(&'static str),
    /// The token did not verify. The code is one of the envelope's four.
    Envelope(&'static str),
    /// The request carries no service account id — none at all, or something
    /// that is not a UUID, `<org_slug>/<name>` included. Nothing is matched for
    /// it: the workflow says which account it trusts, by id, or no policy is
    /// read at all.
    ServiceAccountRequired,
    /// The token verified, and no trust policy of the named account admits
    /// the run.
    Claims(ClaimReject),
}

impl Rejection {
    /// The reason: the wire `code`, and the metric's one label. Always a
    /// member of `OIDC_EXCHANGE_REJECT_REASONS`.
    pub(super) fn reason(&self) -> &'static str {
        match self {
            Self::Malformed(_) => MALFORMED,
            Self::ServiceAccountRequired => ACCOUNT_REQUIRED,
            Self::Envelope(code) => *code,
            Self::Claims(reject) => reject.code(),
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Malformed(_) | Self::ServiceAccountRequired => StatusCode::BAD_REQUEST,
            Self::Envelope(_) => StatusCode::UNAUTHORIZED,
            Self::Claims(_) => StatusCode::FORBIDDEN,
        }
    }

    fn message(&self) -> &'static str {
        match self {
            Self::Malformed(message) => message,
            Self::ServiceAccountRequired => {
                "name the service account this run acts as, by its id: 'service_account' is the \
                 account's id (a UUID, not '<org_slug>/<name>'), shown under Organization → API \
                 access → the service account"
            }
            Self::Envelope("wrong_audience") => {
                "the OIDC token was not requested for this deployment's audience, which is \
                 'oxy:<host>' of the address it answers to (see 'audience'): point the client \
                 at that address"
            }
            Self::Envelope("expired") => "the OIDC token has expired",
            Self::Envelope("replayed") => "the OIDC token has already been used",
            Self::Envelope(_) => "the OIDC token is not valid",
            Self::Claims(ClaimReject::PullRequestTarget) => {
                "a pull_request_target run cannot be granted access"
            }
            Self::Claims(ClaimReject::SelfHostedRunner) => {
                "the run is on a self-hosted runner, which no matching trust policy allows"
            }
            Self::Claims(ClaimReject::MissingEnvironment) => {
                "the job names no deployment environment, which the trust policy requires"
            }
            Self::Claims(ClaimReject::PolicyWithoutEnvironment) => {
                "the organization requires a trust policy to name a deployment environment, and \
                 the policy that matches this run names none: an org admin sets one on the policy"
            }
            Self::Claims(ClaimReject::NoMatchingPolicy) => {
                "no trust policy of the named service account matches this run"
            }
        }
    }

    fn body(&self) -> Value {
        match self {
            // A plain 400, as every malformed body on these routes is.
            Self::Malformed(_) => json!({ "error": self.message() }),
            // Says which audience it takes — the one place the value is on
            // the wire. Not so that a client can ask for it instead: a client
            // derives its audience from the URL it talks to, and this tells
            // the person reading the failure which address that should be.
            Self::Envelope(WRONG_AUDIENCE) => json!({
                "error": self.message(),
                "code": self.reason(),
                "audience": super::audience::audience(),
            }),
            _ => json!({ "error": self.message(), "code": self.reason() }),
        }
    }

    fn audit_entry(&self, headers: &HeaderMap, claims: Option<&Value>) -> AuditEntry {
        let mut metadata = json!({ "reason": self.reason() });
        if let Some(claims) = claims {
            metadata["claims"] = claims.clone();
        }
        let mut entry = AuditEntry::new(ACTOR, REJECTED)
            .failure(self.reason())
            .metadata(metadata)
            .context(AuditContext {
                ip: audit::client_ip(headers),
                user_agent: audit::user_agent(headers),
                request_id: None,
            });
        entry.actor_type = ActorType::System;
        entry
    }

    /// Count the refusal, and write its audit row. A body that named no token
    /// or no account is counted and not audited: it is refused before the
    /// token is read, so there is nothing in it to record.
    pub(super) async fn record(
        &self,
        db: &DatabaseConnection,
        headers: &HeaderMap,
        claims: Option<&Value>,
    ) {
        oxy_telemetry::metrics::record::oidc_exchange_rejected(self.reason());
        tracing::warn!(reason = self.reason(), "oidc exchange refused");
        if matches!(self, Self::Malformed(_) | Self::ServiceAccountRequired) {
            return;
        }
        audit::record_best_effort(db, self.audit_entry(headers, claims)).await;
    }
}

impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        (self.status(), Json(self.body())).into_response()
    }
}

#[cfg(test)]
mod tests {
    use oxy_telemetry::metrics::record::OIDC_EXCHANGE_REJECT_REASONS;

    use super::*;

    fn every_rejection() -> Vec<Rejection> {
        vec![
            Rejection::Malformed("no token"),
            Rejection::ServiceAccountRequired,
            Rejection::Envelope("invalid_token"),
            Rejection::Envelope("wrong_audience"),
            Rejection::Envelope("expired"),
            Rejection::Envelope("replayed"),
            Rejection::Claims(ClaimReject::PullRequestTarget),
            Rejection::Claims(ClaimReject::SelfHostedRunner),
            Rejection::Claims(ClaimReject::MissingEnvironment),
            Rejection::Claims(ClaimReject::PolicyWithoutEnvironment),
            Rejection::Claims(ClaimReject::NoMatchingPolicy),
        ]
    }

    #[test]
    fn every_reason_is_one_the_metric_was_seeded_with() {
        let mut seen: Vec<&str> = every_rejection().iter().map(Rejection::reason).collect();
        for reason in &seen {
            assert!(
                OIDC_EXCHANGE_REJECT_REASONS.contains(reason),
                "{reason} would be born un-seeded"
            );
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            OIDC_EXCHANGE_REJECT_REASONS.len(),
            "the label set is exactly the refusals there are"
        );
    }

    #[test]
    fn each_refusal_answers_with_the_contracts_status_and_code() {
        for (rejection, status, code) in [
            (Rejection::Envelope("invalid_token"), 401, "invalid_token"),
            (Rejection::Envelope("wrong_audience"), 401, "wrong_audience"),
            (Rejection::Envelope("expired"), 401, "expired"),
            (Rejection::Envelope("replayed"), 401, "replayed"),
            (
                Rejection::Claims(ClaimReject::PullRequestTarget),
                403,
                "pull_request_target",
            ),
            (
                Rejection::Claims(ClaimReject::SelfHostedRunner),
                403,
                "self_hosted_runner",
            ),
            (
                Rejection::Claims(ClaimReject::MissingEnvironment),
                403,
                "missing_environment",
            ),
            (
                Rejection::Claims(ClaimReject::PolicyWithoutEnvironment),
                403,
                "missing_environment",
            ),
            (
                Rejection::Claims(ClaimReject::NoMatchingPolicy),
                403,
                "no_matching_policy",
            ),
        ] {
            assert_eq!(rejection.status().as_u16(), status, "{rejection:?}");
            assert_eq!(rejection.body()["code"], code, "{rejection:?}");
        }
    }

    #[test]
    fn a_run_that_names_no_account_is_told_to_name_one() {
        let rejection = Rejection::ServiceAccountRequired;
        assert_eq!(rejection.status(), StatusCode::BAD_REQUEST);
        let body = rejection.body();
        assert_eq!(body["code"], "service_account_required");
        assert!(body.get("candidates").is_none(), "it lists no accounts");
        // It asks for the id, and says where the id is.
        let said = body["error"].as_str().unwrap_or_default();
        assert!(said.contains("account's id"), "{said}");
        assert!(said.contains("API access"), "{said}");
    }

    #[test]
    fn a_policy_with_no_environment_shares_the_code_and_says_who_fixes_it() {
        // The job may well name an environment: what is missing is the
        // policy's. Same code, so a client needs no new case; its own words,
        // so the person reading them is not sent to edit the workflow.
        let org = Rejection::Claims(ClaimReject::PolicyWithoutEnvironment);
        let job = Rejection::Claims(ClaimReject::MissingEnvironment);
        assert_eq!(org.reason(), job.reason());
        assert_ne!(org.message(), job.message());
        assert!(org.message().contains("organization requires"));
    }

    #[test]
    fn a_wrong_audience_is_told_the_right_one() {
        let body = Rejection::Envelope("wrong_audience").body();
        assert_eq!(body["code"], "wrong_audience");
        assert_eq!(body["audience"], super::super::audience::audience());
        // And no other refusal carries it.
        for other in every_rejection() {
            if other != Rejection::Envelope("wrong_audience") {
                assert!(other.body().get("audience").is_none(), "{other:?}");
            }
        }
    }

    #[test]
    fn a_malformed_body_is_a_plain_400() {
        let rejection = Rejection::Malformed("no token");
        assert_eq!(rejection.status(), StatusCode::BAD_REQUEST);
        assert!(rejection.body().get("code").is_none());
    }

    #[test]
    fn the_audit_row_is_unchained_and_carries_the_reason_and_the_claims() {
        let claims = json!({ "repository": "acme/app", "jti": "j" });
        let entry = Rejection::Claims(ClaimReject::NoMatchingPolicy)
            .audit_entry(&HeaderMap::new(), Some(&claims));
        assert_eq!(entry.action, REJECTED);
        assert_eq!(entry.org_id, None, "it matched no org: no chain to join");
        assert_eq!(entry.actor_user_id, None);
        assert_eq!(entry.reason.as_deref(), Some("no_matching_policy"));
        assert_eq!(entry.metadata["reason"], "no_matching_policy");
        assert_eq!(entry.metadata["claims"], claims);
    }

    #[test]
    fn a_token_that_did_not_verify_records_no_claims() {
        // Nothing in an unverified token is ours to repeat.
        let entry = Rejection::Envelope("invalid_token").audit_entry(&HeaderMap::new(), None);
        assert!(entry.metadata.get("claims").is_none());
    }
}
