//! The pure decision for trusted access: which trust policies a verified
//! run's claims match (API-tokens design §3.4).
//!
//! No network, no database. Every rule is here, and none reads `sub`:
//!
//! | Policy field | Rule |
//! | --- | --- |
//! | `repository_owner_id`, `repository_id` | numeric, exact — names are never compared |
//! | `workflow_path` | the path part of `job_workflow_ref` |
//! | `environment` | case-insensitive, exact, when the policy sets one |
//! | `ref_pattern` | a glob on `ref`, when the policy sets one |
//! | `allow_self_hosted` | off: `runner_environment` must be `github-hosted` |
//! | always | `event_name` is not `pull_request_target` |

use uuid::Uuid;

use super::claims::{GITHUB_HOSTED, GithubOidcClaims, PULL_REQUEST_TARGET};

/// The fields of one trust policy that take part in the decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyRule {
    pub policy_id: Uuid,
    pub repository_owner_id: i64,
    pub repository_id: i64,
    /// `.github/workflows/release.yml` — a workflow of the policy's own
    /// repository — or `owner/repo/.github/workflows/deploy.yml`, a reusable
    /// workflow in another one.
    pub workflow_path: String,
    /// `None` = any environment, or none. Only an org that relaxed the
    /// requirement can store one.
    pub environment: Option<String>,
    pub ref_pattern: Option<String>,
    pub allow_self_hosted: bool,
}

/// Why no policy matched. Each answers 403 with [`ClaimReject::code`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClaimReject {
    /// A fork PR running with the base repo's permissions.
    PullRequestTarget,
    /// The run is on a self-hosted runner, and no policy that otherwise
    /// matches allows one.
    SelfHostedRunner,
    /// The run names no environment, and every policy that otherwise matches
    /// requires one.
    MissingEnvironment,
    /// The org requires its trust policies to name an environment, and the
    /// policy that admits the run names none. Never returned by
    /// [`match_policies`], which does not know the org: the exchange decides
    /// it (`crate::token::exchange`). It shares [`ClaimReject::MissingEnvironment`]'s
    /// wire code — an environment is what is missing — and has its own words.
    PolicyWithoutEnvironment,
    NoMatchingPolicy,
}

impl ClaimReject {
    /// The wire code, and the metric's reason label.
    pub fn code(self) -> &'static str {
        match self {
            Self::PullRequestTarget => "pull_request_target",
            Self::SelfHostedRunner => "self_hosted_runner",
            Self::MissingEnvironment | Self::PolicyWithoutEnvironment => "missing_environment",
            Self::NoMatchingPolicy => "no_matching_policy",
        }
    }
}

/// The policies `claims` match, in the order given.
///
/// The refusals narrow in a fixed order — repository and workflow, then the
/// runner, then the environment — so the reason names the first rule that left
/// nothing: a run in a repository no policy names is `NoMatchingPolicy`
/// whatever its runner, and never learns that a runner rule exists.
pub fn match_policies<'a>(
    claims: &GithubOidcClaims,
    rules: &'a [PolicyRule],
) -> Result<Vec<&'a PolicyRule>, ClaimReject> {
    if claims.event_name == PULL_REQUEST_TARGET {
        return Err(ClaimReject::PullRequestTarget);
    }
    let named: Vec<&PolicyRule> = rules.iter().filter(|r| names_this_run(claims, r)).collect();
    if named.is_empty() {
        return Err(ClaimReject::NoMatchingPolicy);
    }
    let hosted = claims.runner_environment == GITHUB_HOSTED;
    let runner_ok: Vec<&PolicyRule> = named
        .into_iter()
        .filter(|r| hosted || r.allow_self_hosted)
        .collect();
    if runner_ok.is_empty() {
        return Err(ClaimReject::SelfHostedRunner);
    }
    let matched: Vec<&PolicyRule> = runner_ok
        .into_iter()
        .filter(|r| environment_matches(claims, r))
        .collect();
    if !matched.is_empty() {
        return Ok(matched);
    }
    Err(if claims.environment.is_none() {
        ClaimReject::MissingEnvironment
    } else {
        ClaimReject::NoMatchingPolicy
    })
}

/// The repository, by both numeric ids; the workflow; and the ref.
fn names_this_run(claims: &GithubOidcClaims, rule: &PolicyRule) -> bool {
    claims.owner_id() == Some(rule.repository_owner_id)
        && claims.repo_id() == Some(rule.repository_id)
        && workflow_matches(claims, &rule.workflow_path)
        && ref_matches(claims, rule.ref_pattern.as_deref())
}

/// A policy path that starts in `.github/` names a workflow of the run's own
/// repository, whose identity the numeric ids already pinned. Any other names
/// a workflow by its full path — a reusable workflow in another repository,
/// which is then the trusted unit.
fn workflow_matches(claims: &GithubOidcClaims, policy_path: &str) -> bool {
    let ran = claims.workflow_path();
    if is_repo_relative(policy_path) {
        let expected = format!("{}/{}", claims.repository, policy_path);
        ran.eq_ignore_ascii_case(&expected)
    } else {
        ran.eq_ignore_ascii_case(policy_path)
    }
}

/// Whether a policy's workflow path is relative to its own repository.
pub fn is_repo_relative(workflow_path: &str) -> bool {
    workflow_path.starts_with(".github/")
}

fn ref_matches(claims: &GithubOidcClaims, pattern: Option<&str>) -> bool {
    match (pattern, claims.git_ref.as_deref()) {
        (None, _) => true,
        (Some(pattern), Some(git_ref)) => glob_matches(pattern, git_ref),
        // A policy that pins refs never matches a run that names none.
        (Some(_), None) => false,
    }
}

fn environment_matches(claims: &GithubOidcClaims, rule: &PolicyRule) -> bool {
    match (rule.environment.as_deref(), claims.environment.as_deref()) {
        (None, _) => true,
        (Some(want), Some(got)) => want.eq_ignore_ascii_case(got),
        (Some(_), None) => false,
    }
}

/// Glob match where `*` is any run of characters, `/` included, and every
/// other character is itself. Case-sensitive, as git refs are.
///
/// `refs/heads/release/*` matches `refs/heads/release/2026/q4`; `refs/tags/v*`
/// matches `refs/tags/v1.2.0` and not `refs/heads/v1`.
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    // Where the last `*` was, and how much of `text` it has swallowed so far.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            // Let the last `*` take one more character and retry from there.
            star = Some((star_p, star_t + 1));
            p = star_p + 1;
            t = star_t + 1;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
#[path = "matcher_tests.rs"]
mod tests;
