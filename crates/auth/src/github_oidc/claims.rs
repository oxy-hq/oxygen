//! The GitHub Actions OIDC claims either exchange reads.

use serde::Deserialize;
use serde_json::{Value, json};

/// GitHub's OIDC issuer + JWKS. Constants, not config: there is exactly one
/// GitHub Actions OIDC provider.
pub const GITHUB_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";
pub const GITHUB_JWKS_URL: &str = "https://token.actions.githubusercontent.com/.well-known/jwks";

/// The audience the legacy trusted-publishing exchange requires. GitHub's
/// default audience is the repo owner's URL; pinning our own value and
/// rejecting others stops any workflow in the org replaying an unrelated token
/// into us.
pub const AUDIENCE_PUBLISH: &str = "oxy-publish";

/// The stem of the audience trusted access requires, and the whole of it on a
/// deployment with no public URL. A deployment that has one requires
/// `oxy:<host>` ([`super::audience::deployment_audience`]), so a token asked
/// for one deployment is refused by every other. It is deliberately not
/// [`AUDIENCE_PUBLISH`]: a token requested for a publish can never be traded
/// for a broader credential, nor the reverse.
pub const AUDIENCE_OXY: &str = "oxy";

/// `runner_environment` of a runner GitHub hosts.
pub const GITHUB_HOSTED: &str = "github-hosted";

/// The event of a fork PR running with the base repo's permissions — never an
/// identity to mint for.
pub const PULL_REQUEST_TARGET: &str = "pull_request_target";

/// The subset of GitHub Actions OIDC claims we read. Every custom claim GitHub
/// emits is a **string**, including the numeric-looking ids.
///
/// The claims the legacy exchange matched on are required, as they always
/// were. The ones trusted access added are optional here so a token that lacks
/// one still decodes on the legacy route; trusted access refuses a token
/// without the ones it needs.
#[derive(Clone, Debug, Deserialize)]
pub struct GithubOidcClaims {
    /// "owner/repo" — case-insensitive.
    pub repository: String,
    pub repository_owner: String,
    /// GitHub's NUMERIC account id, as a string. The account-resurrection defence:
    /// a deleted-and-recreated owner with the same name gets a new id.
    pub repository_owner_id: String,
    /// GitHub's NUMERIC repository id, as a string. A repo deleted and
    /// recreated under the same name, or transferred away and squatted, gets a
    /// new one.
    #[serde(default)]
    pub repository_id: Option<String>,
    /// e.g. "owner/repo/.github/workflows/oxy-publish.yml@refs/heads/main". For
    /// a reusable workflow it names the *called* workflow, in its own repo.
    pub job_workflow_ref: String,
    /// The deployment environment — what lets a job be gated behind required
    /// reviewers.
    pub environment: Option<String>,
    pub event_name: String,
    /// "github-hosted" | "self-hosted".
    pub runner_environment: String,
    /// One-time id; burned by the replay store on the signature path.
    pub jti: String,
    /// Issued-at, seconds since the epoch. Required by being non-optional:
    /// `jsonwebtoken`'s `set_required_spec_claims` silently ignores `"iat"`
    /// (it checks only `exp`, `nbf`, `sub`, `iss` and `aud`). GitHub signs
    /// every Actions token with one, so the legacy exchange loses nothing.
    pub iat: i64,
    /// e.g. "refs/heads/main".
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub run_attempt: Option<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

impl GithubOidcClaims {
    /// The numeric owner id, when the claim is one.
    pub fn owner_id(&self) -> Option<i64> {
        self.repository_owner_id.parse().ok()
    }

    /// The numeric repository id, when the claim is present and is one.
    pub fn repo_id(&self) -> Option<i64> {
        self.repository_id.as_deref()?.parse().ok()
    }

    /// The path part of `job_workflow_ref`: everything before the `@<ref>`.
    pub fn workflow_path(&self) -> &str {
        self.job_workflow_ref
            .split_once('@')
            .map(|(path, _ref)| path)
            .unwrap_or(&self.job_workflow_ref)
    }

    /// The verified claims a minted token records, and the audit row carries.
    /// Never the JWT, and never `sub`.
    pub fn recorded(&self) -> Value {
        json!({
            "jti": self.jti,
            "run_id": self.run_id,
            "run_attempt": self.run_attempt,
            "sha": self.sha,
            "ref": self.git_ref,
            "environment": self.environment,
            "job_workflow_ref": self.job_workflow_ref,
            "actor_id": self.actor_id,
            "repository_id": self.repository_id,
            "repository": self.repository,
            "repository_owner_id": self.repository_owner_id,
            "event_name": self.event_name,
            "runner_environment": self.runner_environment,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_without_the_newer_claims_still_decodes() {
        // The legacy exchange reads none of them, and must not start refusing
        // a token over a claim it never asked for.
        let claims: GithubOidcClaims = serde_json::from_value(json!({
            "repository": "acme/app",
            "repository_owner": "acme",
            "repository_owner_id": "12345",
            "job_workflow_ref": "acme/app/.github/workflows/p.yml@refs/heads/main",
            "event_name": "push",
            "runner_environment": "github-hosted",
            "jti": "j",
            // Registered, not newer: GitHub signs every token with an `iat`.
            "iat": 1_700_000_000,
        }))
        .expect("the legacy claim set decodes");
        assert_eq!(claims.owner_id(), Some(12345));
        assert_eq!(claims.repo_id(), None);
        assert_eq!(claims.environment, None);
        assert_eq!(claims.git_ref, None);
    }

    #[test]
    fn the_ids_parse_as_numbers_or_not_at_all() {
        let mut claims = crate::github_oidc::test_support::claims();
        assert_eq!(claims.repo_id(), Some(987));
        claims.repository_id = Some("987; drop".into());
        assert_eq!(claims.repo_id(), None);
        claims.repository_owner_id = "acme".into();
        assert_eq!(claims.owner_id(), None);
    }

    #[test]
    fn the_workflow_path_drops_the_ref() {
        let claims = crate::github_oidc::test_support::claims();
        assert_eq!(
            claims.workflow_path(),
            "acme/app/.github/workflows/release.yml"
        );
    }

    #[test]
    fn what_is_recorded_names_the_run_and_never_the_subject() {
        let recorded = crate::github_oidc::test_support::claims().recorded();
        for key in [
            "jti",
            "run_id",
            "run_attempt",
            "sha",
            "ref",
            "environment",
            "job_workflow_ref",
            "actor_id",
            "repository_id",
        ] {
            assert!(recorded.get(key).is_some(), "{key} is recorded");
        }
        assert!(recorded.get("sub").is_none());
    }
}
