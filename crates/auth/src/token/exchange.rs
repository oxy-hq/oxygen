//! Which trust policy a verified run mints from (API-tokens design §3.4).
//!
//! Pure: the candidates are already loaded, the claims already verified. The
//! claim rules themselves are [`crate::github_oidc::matcher`]; this is the
//! step after — **the match is deterministic, never a union**:
//!
//! - a run always names its service account, **by id**, and the candidates
//!   are that account's policies alone. The workflow, which the repository's
//!   owner controls, says which account it trusts — so a policy another org
//!   registered on the same repository is never a candidate, and can neither
//!   mint for the run nor stand in its way. By id, not `<org_slug>/<name>`: a
//!   slug is free for anyone once its org renames or is deleted, and whoever
//!   took it could create the same name under it;
//! - several matching policies of the one account is not an error: the oldest
//!   wins. The candidates arrive oldest first, so that is the first;
//! - **while the org requires an environment, a policy that names none does
//!   not match.** The requirement is asked here, where a policy is used, and
//!   not only where one is written: an org that turns it back on stops its
//!   environment-less policies minting at once, without rewriting one. A run
//!   such a policy would have admitted is told so (`missing_environment`).

use entity::oidc_trust_policies;

use super::trust_policy::Candidate;
use crate::github_oidc::{ClaimReject, GithubOidcClaims, PolicyRule, match_policies};

/// The claim-matching fields of a stored policy.
pub fn rule_of(policy: &oidc_trust_policies::Model) -> PolicyRule {
    PolicyRule {
        policy_id: policy.id,
        repository_owner_id: policy.repository_owner_id,
        repository_id: policy.repository_id,
        workflow_path: policy.workflow_path.clone(),
        environment: policy.environment.clone(),
        ref_pattern: policy.ref_pattern.clone(),
        allow_self_hosted: policy.allow_self_hosted,
    }
}

/// What the exchange does with a verified run.
#[derive(Clone, Debug)]
pub enum Decision {
    /// Mint from this policy, as its account.
    Mint(Box<Candidate>),
    Reject(ClaimReject),
}

/// The first of `candidates` that `claims` match, in the order given.
fn first_match(
    claims: &GithubOidcClaims,
    candidates: Vec<Candidate>,
) -> Result<Candidate, ClaimReject> {
    let rules: Vec<PolicyRule> = candidates.iter().map(|c| rule_of(&c.policy)).collect();
    let first = match_policies(claims, &rules)?
        .first()
        .map(|rule| rule.policy_id);
    candidates
        .into_iter()
        .find(|c| Some(c.policy.id) == first)
        .ok_or(ClaimReject::NoMatchingPolicy)
}

/// Decide for `claims` among `candidates`: the live policies of the one
/// account the run named, on its repository, oldest first.
///
/// `environment_required` is the org's say now
/// ([`super::trust_policy::environment_required`]). While it holds, a policy
/// that names no environment is set aside before anything is matched; if one
/// of those is what would have admitted the run, the refusal says so.
pub fn decide(
    claims: &GithubOidcClaims,
    candidates: Vec<Candidate>,
    environment_required: bool,
) -> Decision {
    let (usable, set_aside): (Vec<Candidate>, Vec<Candidate>) = candidates
        .into_iter()
        .partition(|c| !environment_required || c.policy.environment.is_some());
    match first_match(claims, usable) {
        Ok(candidate) => Decision::Mint(Box::new(candidate)),
        Err(_) if first_match(claims, set_aside).is_ok() => {
            Decision::Reject(ClaimReject::PolicyWithoutEnvironment)
        }
        Err(reject) => Decision::Reject(reject),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use entity::service_accounts;
    use uuid::Uuid;

    use super::*;
    use crate::github_oidc::test_support::claims;

    const ORG: Uuid = Uuid::from_u128(0xA);

    fn candidate(policy: u128, account: u128, name: &str) -> Candidate {
        let now = Utc::now().fixed_offset();
        Candidate {
            policy: oidc_trust_policies::Model {
                id: Uuid::from_u128(policy),
                org_id: ORG,
                service_account_id: Uuid::from_u128(account),
                provider: "github_actions".into(),
                repository_owner_id: 42,
                repository_id: 987,
                repository: "acme/app".into(),
                workflow_path: ".github/workflows/release.yml".into(),
                environment: Some("production".into()),
                ref_pattern: None,
                allow_self_hosted: false,
                created_by: None,
                created_at: now,
                last_used_at: None,
                disabled_at: None,
            },
            account: service_accounts::Model {
                user_id: Uuid::from_u128(account),
                org_id: ORG,
                org_role: "member".into(),
                name: name.into(),
                description: None,
                created_by: None,
                created_at: now,
                disabled_at: None,
            },
            org_slug: "acme".into(),
        }
    }

    fn minted(decision: Decision) -> Uuid {
        match decision {
            Decision::Mint(candidate) => candidate.policy.id,
            other => panic!("expected a mint, got {other:?}"),
        }
    }

    #[test]
    fn one_matching_policy_mints() {
        let decision = decide(&claims(), vec![candidate(1, 7, "deployer")], false);
        assert_eq!(minted(decision), Uuid::from_u128(1));
    }

    #[test]
    fn no_candidates_is_no_matching_policy() {
        // What a run that names an account with no policy on its repository —
        // or an account that does not exist — is decided among.
        assert!(matches!(
            decide(&claims(), Vec::new(), false),
            Decision::Reject(ClaimReject::NoMatchingPolicy)
        ));
    }

    #[test]
    fn two_policies_of_one_account_are_never_merged_the_oldest_wins() {
        let mut relaxed = candidate(2, 7, "deployer");
        relaxed.policy.environment = None;
        // Oldest first, as the lookup returns them.
        let candidates = vec![candidate(1, 7, "deployer"), relaxed];
        assert_eq!(
            minted(decide(&claims(), candidates, false)),
            Uuid::from_u128(1)
        );
    }

    #[test]
    fn several_matching_policies_is_not_an_error_and_the_order_decides() {
        // The same two policies, in the other order: still a mint, of whichever
        // the lookup put first. Nothing here is "ambiguous".
        let mut relaxed = candidate(2, 7, "deployer");
        relaxed.policy.environment = None;
        let candidates = vec![relaxed, candidate(1, 7, "deployer")];
        assert_eq!(
            minted(decide(&claims(), candidates, false)),
            Uuid::from_u128(2)
        );
    }

    #[test]
    fn a_policy_that_does_not_match_is_passed_over() {
        let mut other_env = candidate(1, 7, "deployer");
        other_env.policy.environment = Some("staging".into());
        let candidates = vec![other_env, candidate(2, 7, "deployer")];
        assert_eq!(
            minted(decide(&claims(), candidates, false)),
            Uuid::from_u128(2)
        );
    }

    #[test]
    fn the_refusal_is_the_claim_matchers() {
        let mut run = claims();
        run.runner_environment = "self-hosted".into();
        assert!(matches!(
            decide(&run, vec![candidate(1, 7, "deployer")], false),
            Decision::Reject(ClaimReject::SelfHostedRunner)
        ));
        let mut run = claims();
        run.event_name = "pull_request_target".into();
        assert!(matches!(
            decide(&run, Vec::new(), false),
            Decision::Reject(ClaimReject::PullRequestTarget)
        ));
        let mut run = claims();
        run.environment = None;
        assert!(matches!(
            decide(&run, vec![candidate(1, 7, "deployer")], false),
            Decision::Reject(ClaimReject::MissingEnvironment)
        ));
    }

    /// A policy with no environment: only an org that relaxed the requirement
    /// could have stored it.
    fn bare(policy: u128) -> Candidate {
        let mut candidate = candidate(policy, 7, "deployer");
        candidate.policy.environment = None;
        candidate
    }

    #[test]
    fn a_policy_with_no_environment_mints_only_while_the_org_does_not_require_one() {
        // Relaxed: it admits the run, with an environment or without.
        let mut no_env = claims();
        no_env.environment = None;
        for run in [claims(), no_env] {
            assert_eq!(
                minted(decide(&run, vec![bare(1)], false)),
                Uuid::from_u128(1)
            );
            // Required again: the same policy, untouched, no longer matches —
            // and the run is told it is the policy that names no environment.
            assert!(matches!(
                decide(&run, vec![bare(1)], true),
                Decision::Reject(ClaimReject::PolicyWithoutEnvironment)
            ));
        }
    }

    #[test]
    fn a_policy_that_names_its_environment_is_unaffected_by_the_requirement() {
        // The bare policy is older, and would have won; required, it is set
        // aside and the one that names `production` mints.
        let candidates = vec![bare(1), candidate(2, 7, "deployer")];
        assert_eq!(
            minted(decide(&claims(), candidates.clone(), false)),
            Uuid::from_u128(1)
        );
        assert_eq!(
            minted(decide(&claims(), candidates, true)),
            Uuid::from_u128(2)
        );
    }

    #[test]
    fn a_bare_policy_that_would_not_have_matched_does_not_change_the_reason() {
        // The bare policy is for another workflow, so it is not what stood in
        // the way: the run hears what it would have heard without it.
        let mut elsewhere = bare(1);
        elsewhere.policy.workflow_path = ".github/workflows/other.yml".into();
        let mut run = claims();
        run.environment = None;
        let candidates = vec![elsewhere.clone(), candidate(2, 7, "deployer")];
        assert!(matches!(
            decide(&run, candidates, true),
            Decision::Reject(ClaimReject::MissingEnvironment)
        ));
        assert!(matches!(
            decide(&claims(), vec![elsewhere], true),
            Decision::Reject(ClaimReject::NoMatchingPolicy)
        ));
        // And a `pull_request_target` run is refused as one, whatever is set aside.
        let mut fork = claims();
        fork.event_name = "pull_request_target".into();
        assert!(matches!(
            decide(&fork, vec![bare(1)], true),
            Decision::Reject(ClaimReject::PullRequestTarget)
        ));
    }
}
