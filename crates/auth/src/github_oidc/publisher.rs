//! The pure decision for the legacy trusted-publishing exchange: which
//! `app_publishers` rows a token's claims match.
//!
//! The rules that get platforms owned are exactly the ones NOT to leave out:
//! never match `sub` (immutable-format changeover), require the `environment`
//! claim, reject `pull_request_target`, require a github-hosted runner, and
//! match on the numeric `repository_owner_id` (the account-resurrection
//! defence).
//!
//! Trusted access decides with [`super::matcher`], which adds the numeric
//! repository id, the ref pattern and the per-policy runner and environment
//! rules. This one is kept as it shipped, because `app_publishers` rows and
//! the workflows registered against them keep working unchanged.

use uuid::Uuid;

use super::claims::{GITHUB_HOSTED, GithubOidcClaims, PULL_REQUEST_TARGET};

/// One publisher config to match against — the fields of an `app_publishers` row
/// that participate in the decision, plus the app it authorizes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublisherConfig {
    pub app_id: Uuid,
    pub repo_owner: String,
    pub repo_owner_id: i64,
    pub repo_name: String,
    /// Just the workflow path, e.g. ".github/workflows/oxy-publish.yml".
    pub workflow_ref: String,
    pub environment: String,
}

/// Why a token was refused. The token-envelope reasons (`bad signature`, `wrong
/// aud`, `expired`, `replayed jti`) are handled on the signature path; these are
/// the claim-matching reasons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OidcReject {
    /// A fork PR running with base-repo permissions — never a publish identity.
    PullRequestTarget,
    /// A self-hosted runner is a standing token-minting box inside the partner's
    /// network; we only trust github-hosted runners.
    SelfHostedRunner,
    /// The token carries no `environment` claim, so no publisher (which all require
    /// one) can match it.
    MissingEnvironment,
    /// No publisher config for this repo matched the token's claims.
    NoMatchingPublisher,
}

/// The pure decision. Returns the app ids whose publisher config matches the
/// token — usually one, more than one only for a monorepo that publishes several
/// apps from the same repo+workflow+environment.
///
/// `publishers` is expected to already be the set of configs for this repo (the
/// caller narrows by `repository_owner_id` + `repository` at the DB layer); every
/// rule is nonetheless re-checked here so the decision stands alone.
pub fn verify_claims(
    claims: &GithubOidcClaims,
    publishers: &[PublisherConfig],
) -> Result<Vec<Uuid>, OidcReject> {
    // Token-level gates first — these reject regardless of any publisher.
    if claims.event_name == PULL_REQUEST_TARGET {
        return Err(OidcReject::PullRequestTarget);
    }
    if claims.runner_environment != GITHUB_HOSTED {
        return Err(OidcReject::SelfHostedRunner);
    }
    let Some(token_env) = claims.environment.as_deref() else {
        return Err(OidcReject::MissingEnvironment);
    };

    let matches: Vec<Uuid> = publishers
        .iter()
        .filter(|p| publisher_matches(claims, p, token_env))
        .map(|p| p.app_id)
        .collect();

    if matches.is_empty() {
        Err(OidcReject::NoMatchingPublisher)
    } else {
        Ok(matches)
    }
}

/// Exact, case-insensitive equality on every claim — never a prefix, never a
/// wildcard, never `sub`.
fn publisher_matches(claims: &GithubOidcClaims, p: &PublisherConfig, token_env: &str) -> bool {
    let expected_repo = format!("{}/{}", p.repo_owner, p.repo_name);
    let expected_workflow_path = format!("{}/{}/{}", p.repo_owner, p.repo_name, p.workflow_ref);

    eq_ci(&claims.repository, &expected_repo)
        // Numeric owner id — the resurrection defence. Claim is a string.
        && claims.repository_owner_id == p.repo_owner_id.to_string()
        && eq_ci(claims.workflow_path(), &expected_workflow_path)
        && eq_ci(token_env, &p.environment)
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The verified identity a machine publish is attributed to, e.g.
/// `github-oidc:acme/app/.github/workflows/oxy-publish.yml@refs/heads/main env=production`.
///
/// Written as the minted token's `name`; the publish that token authenticates
/// copies it onto the build as `app_builds.published_via`. Only verified
/// claims go in: `job_workflow_ref` already carries repo, workflow and ref.
pub fn machine_identity(claims: &GithubOidcClaims) -> String {
    let env = claims.environment.as_deref().unwrap_or("-");
    format!("github-oidc:{} env={env}", claims.job_workflow_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Uuid {
        Uuid::from_u128(1)
    }

    fn publisher() -> PublisherConfig {
        PublisherConfig {
            app_id: app(),
            repo_owner: "acme-consulting".into(),
            repo_owner_id: 42,
            repo_name: "northwind-dashboard".into(),
            workflow_ref: ".github/workflows/oxy-publish.yml".into(),
            environment: "oxy-publish".into(),
        }
    }

    fn claims() -> GithubOidcClaims {
        GithubOidcClaims {
            repository: "acme-consulting/northwind-dashboard".into(),
            repository_owner: "acme-consulting".into(),
            repository_owner_id: "42".into(),
            repository_id: Some("4242".into()),
            job_workflow_ref:
                "acme-consulting/northwind-dashboard/.github/workflows/oxy-publish.yml@refs/heads/main"
                    .into(),
            environment: Some("oxy-publish".into()),
            event_name: "push".into(),
            runner_environment: "github-hosted".into(),
            jti: "abc123".into(),
            iat: 1_700_000_000,
            git_ref: Some("refs/heads/main".into()),
            sha: None,
            run_id: None,
            run_attempt: None,
            actor_id: None,
        }
    }

    #[test]
    fn machine_identity_names_the_verified_workflow_and_environment() {
        assert_eq!(
            machine_identity(&claims()),
            "github-oidc:acme-consulting/northwind-dashboard/.github/workflows/oxy-publish.yml@refs/heads/main env=oxy-publish"
        );
    }

    #[test]
    fn exact_match_returns_the_app() {
        assert_eq!(verify_claims(&claims(), &[publisher()]), Ok(vec![app()]));
    }

    #[test]
    fn case_insensitive_repo_and_env() {
        let mut c = claims();
        c.repository = "Acme-Consulting/Northwind-Dashboard".into();
        c.environment = Some("OXY-PUBLISH".into());
        c.job_workflow_ref =
            "Acme-Consulting/Northwind-Dashboard/.github/workflows/oxy-publish.yml@refs/heads/main"
                .into();
        assert_eq!(verify_claims(&c, &[publisher()]), Ok(vec![app()]));
    }

    #[test]
    fn wrong_repo_name_does_not_match() {
        let mut c = claims();
        c.repository = "acme-consulting/globex-dashboard".into();
        assert_eq!(
            verify_claims(&c, &[publisher()]),
            Err(OidcReject::NoMatchingPublisher)
        );
    }

    #[test]
    fn same_repo_name_different_owner_id_is_rejected() {
        // The resurrection attack: a new account named "acme-consulting" (new
        // numeric id) must not match a publisher registered to the old one, even
        // though the `repository` string is identical.
        let mut c = claims();
        c.repository_owner_id = "999".into();
        assert_eq!(
            verify_claims(&c, &[publisher()]),
            Err(OidcReject::NoMatchingPublisher)
        );
    }

    #[test]
    fn wrong_workflow_ref_does_not_match() {
        // A different workflow file in the same repo — e.g. an attacker's PR adding
        // `.github/workflows/evil.yml` — must not publish.
        let mut c = claims();
        c.job_workflow_ref =
            "acme-consulting/northwind-dashboard/.github/workflows/evil.yml@refs/heads/main".into();
        assert_eq!(
            verify_claims(&c, &[publisher()]),
            Err(OidcReject::NoMatchingPublisher)
        );
    }

    #[test]
    fn wrong_environment_does_not_match() {
        let mut c = claims();
        c.environment = Some("staging".into());
        assert_eq!(
            verify_claims(&c, &[publisher()]),
            Err(OidcReject::NoMatchingPublisher)
        );
    }

    #[test]
    fn missing_environment_is_rejected_outright() {
        let mut c = claims();
        c.environment = None;
        assert_eq!(
            verify_claims(&c, &[publisher()]),
            Err(OidcReject::MissingEnvironment)
        );
    }

    #[test]
    fn pull_request_target_is_rejected() {
        let mut c = claims();
        c.event_name = "pull_request_target".into();
        assert_eq!(
            verify_claims(&c, &[publisher()]),
            Err(OidcReject::PullRequestTarget)
        );
    }

    #[test]
    fn self_hosted_runner_is_rejected() {
        let mut c = claims();
        c.runner_environment = "self-hosted".into();
        assert_eq!(
            verify_claims(&c, &[publisher()]),
            Err(OidcReject::SelfHostedRunner)
        );
    }

    #[test]
    fn monorepo_matches_multiple_apps() {
        // Two apps published from the same repo+workflow+environment.
        let a2 = Uuid::from_u128(2);
        let mut p2 = publisher();
        p2.app_id = a2;
        let got = verify_claims(&claims(), &[publisher(), p2]).unwrap();
        assert!(got.contains(&app()) && got.contains(&a2) && got.len() == 2);
    }

    #[test]
    fn no_publishers_never_matches() {
        assert_eq!(
            verify_claims(&claims(), &[]),
            Err(OidcReject::NoMatchingPublisher)
        );
    }
}
