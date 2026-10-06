use super::*;
use crate::github_oidc::test_support::claims;

fn rule() -> PolicyRule {
    PolicyRule {
        policy_id: Uuid::from_u128(1),
        repository_owner_id: 42,
        repository_id: 987,
        workflow_path: ".github/workflows/release.yml".into(),
        environment: Some("production".into()),
        ref_pattern: None,
        allow_self_hosted: false,
    }
}

fn ids(matched: Result<Vec<&PolicyRule>, ClaimReject>) -> Result<Vec<Uuid>, ClaimReject> {
    matched.map(|rules| rules.into_iter().map(|r| r.policy_id).collect())
}

fn decide(claims: &GithubOidcClaims, rule: PolicyRule) -> Result<Vec<Uuid>, ClaimReject> {
    ids(match_policies(claims, &[rule]))
}

#[test]
fn a_run_matching_every_rule_is_admitted() {
    assert_eq!(decide(&claims(), rule()), Ok(vec![Uuid::from_u128(1)]));
}

#[test]
fn the_same_repo_name_under_a_new_owner_id_is_refused() {
    // The owner account was deleted and re-registered: same names, new id.
    let mut c = claims();
    c.repository_owner_id = "43".into();
    assert_eq!(decide(&c, rule()), Err(ClaimReject::NoMatchingPolicy));
}

#[test]
fn a_different_repository_id_is_refused() {
    // The repo was deleted and recreated, or transferred and squatted: the
    // name and the owner are the same, the repository is not.
    let mut c = claims();
    c.repository_id = Some("988".into());
    assert_eq!(decide(&c, rule()), Err(ClaimReject::NoMatchingPolicy));
}

#[test]
fn a_run_without_a_numeric_repository_id_matches_nothing() {
    let mut c = claims();
    c.repository_id = None;
    assert_eq!(decide(&c, rule()), Err(ClaimReject::NoMatchingPolicy));
    c.repository_id = Some("not-a-number".into());
    assert_eq!(decide(&c, rule()), Err(ClaimReject::NoMatchingPolicy));
}

#[test]
fn names_are_never_what_matches() {
    // A rename changes `repository` and the workflow's prefix together; the
    // ids did not move, so the policy still matches.
    let mut c = claims();
    c.repository = "acme-renamed/app-two".into();
    c.repository_owner = "acme-renamed".into();
    c.job_workflow_ref =
        "acme-renamed/app-two/.github/workflows/release.yml@refs/heads/main".into();
    assert_eq!(decide(&c, rule()), Ok(vec![Uuid::from_u128(1)]));
}

#[test]
fn another_workflow_of_the_repository_is_refused() {
    let mut c = claims();
    c.job_workflow_ref = "acme/app/.github/workflows/ci.yml@refs/heads/main".into();
    assert_eq!(decide(&c, rule()), Err(ClaimReject::NoMatchingPolicy));
}

#[test]
fn a_reusable_workflow_matches_only_the_policy_that_names_it() {
    // `acme/app` called `acme/shared`'s deploy workflow: that is what ran, and
    // `job_workflow_ref` names it — in the other repository's path.
    let mut c = claims();
    c.job_workflow_ref = "acme/shared/.github/workflows/deploy.yml@refs/tags/v1".into();

    // A policy naming the caller's own file of the same name does not match…
    let own_file = PolicyRule {
        workflow_path: ".github/workflows/deploy.yml".into(),
        ..rule()
    };
    assert_eq!(decide(&c, own_file), Err(ClaimReject::NoMatchingPolicy));
    // …nor one naming the release workflow…
    assert_eq!(decide(&c, rule()), Err(ClaimReject::NoMatchingPolicy));
    // …nor one naming a reusable workflow somewhere else.
    let elsewhere = PolicyRule {
        workflow_path: "evil/shared/.github/workflows/deploy.yml".into(),
        ..rule()
    };
    assert_eq!(decide(&c, elsewhere), Err(ClaimReject::NoMatchingPolicy));

    // Only the policy that names the reusable workflow by its full path does.
    let named = PolicyRule {
        workflow_path: "acme/shared/.github/workflows/deploy.yml".into(),
        ..rule()
    };
    assert_eq!(decide(&c, named.clone()), Ok(vec![Uuid::from_u128(1)]));

    // And that policy does not match a run of the repository's own workflow.
    assert_eq!(decide(&claims(), named), Err(ClaimReject::NoMatchingPolicy));
}

#[test]
fn a_reusable_workflow_is_still_bound_to_the_calling_repository() {
    // Another repository calling the same shared workflow is not this policy's.
    let mut c = claims();
    c.job_workflow_ref = "acme/shared/.github/workflows/deploy.yml@refs/tags/v1".into();
    c.repository = "acme/other".into();
    c.repository_id = Some("555".into());
    let named = PolicyRule {
        workflow_path: "acme/shared/.github/workflows/deploy.yml".into(),
        ..rule()
    };
    assert_eq!(decide(&c, named), Err(ClaimReject::NoMatchingPolicy));
}

#[test]
fn the_environment_is_exact_and_case_insensitive() {
    let mut c = claims();
    c.environment = Some("Production".into());
    assert_eq!(decide(&c, rule()), Ok(vec![Uuid::from_u128(1)]));
    c.environment = Some("production-eu".into());
    assert_eq!(decide(&c, rule()), Err(ClaimReject::NoMatchingPolicy));
    c.environment = Some("staging".into());
    assert_eq!(decide(&c, rule()), Err(ClaimReject::NoMatchingPolicy));
}

#[test]
fn a_run_with_no_environment_is_refused_by_a_policy_that_requires_one() {
    let mut c = claims();
    c.environment = None;
    assert_eq!(decide(&c, rule()), Err(ClaimReject::MissingEnvironment));
}

#[test]
fn a_policy_with_no_environment_admits_any_or_none() {
    let relaxed = PolicyRule {
        environment: None,
        ..rule()
    };
    let mut c = claims();
    assert_eq!(decide(&c, relaxed.clone()), Ok(vec![Uuid::from_u128(1)]));
    c.environment = None;
    assert_eq!(decide(&c, relaxed), Ok(vec![Uuid::from_u128(1)]));
}

#[test]
fn a_ref_pattern_is_a_glob_on_the_ref() {
    let tags = PolicyRule {
        ref_pattern: Some("refs/tags/v*".into()),
        ..rule()
    };
    let mut c = claims();
    assert_eq!(decide(&c, tags.clone()), Err(ClaimReject::NoMatchingPolicy));
    c.git_ref = Some("refs/tags/v1.2.0".into());
    assert_eq!(decide(&c, tags.clone()), Ok(vec![Uuid::from_u128(1)]));
    // A branch that merely looks like the tag is not the tag.
    c.git_ref = Some("refs/heads/v1.2.0".into());
    assert_eq!(decide(&c, tags.clone()), Err(ClaimReject::NoMatchingPolicy));
    // A policy that pins refs never matches a run that names none.
    c.git_ref = None;
    assert_eq!(decide(&c, tags), Err(ClaimReject::NoMatchingPolicy));

    let main_only = PolicyRule {
        ref_pattern: Some("refs/heads/main".into()),
        ..rule()
    };
    assert_eq!(
        decide(&claims(), main_only.clone()),
        Ok(vec![Uuid::from_u128(1)])
    );
    let mut c = claims();
    c.git_ref = Some("refs/heads/main-backup".into());
    assert_eq!(decide(&c, main_only), Err(ClaimReject::NoMatchingPolicy));
}

#[test]
fn globs_match_whole_refs() {
    for (pattern, text, want) in [
        ("refs/heads/main", "refs/heads/main", true),
        ("refs/heads/main", "refs/heads/main2", false),
        ("refs/heads/main", "xrefs/heads/main", false),
        ("refs/heads/*", "refs/heads/feature/x", true),
        ("refs/heads/release/*", "refs/heads/release/2026/q4", true),
        ("refs/heads/release/*", "refs/heads/released", false),
        ("refs/tags/v*", "refs/tags/v", true),
        ("refs/tags/v*.*", "refs/tags/v1.2", true),
        ("refs/tags/v*.*", "refs/tags/v1", false),
        ("*", "anything/at/all", true),
        ("*", "", true),
        ("", "", true),
        ("", "x", false),
        ("refs/*/main", "refs/heads/main", true),
        ("refs/*/main", "refs/heads/main/x", false),
        ("a*b*c", "a--b--b--c", true),
        ("a*b*c", "a--b--b--", false),
        // Case-sensitive, and `?` is a literal.
        ("refs/heads/Main", "refs/heads/main", false),
        ("refs/heads/ma?n", "refs/heads/main", false),
    ] {
        assert_eq!(glob_matches(pattern, text), want, "{pattern} on {text}");
    }
}

#[test]
fn a_self_hosted_runner_is_refused_unless_the_policy_allows_one() {
    let mut c = claims();
    c.runner_environment = "self-hosted".into();
    assert_eq!(decide(&c, rule()), Err(ClaimReject::SelfHostedRunner));
    let allowing = PolicyRule {
        allow_self_hosted: true,
        ..rule()
    };
    assert_eq!(decide(&c, allowing.clone()), Ok(vec![Uuid::from_u128(1)]));
    // Allowing self-hosted does not refuse a hosted runner.
    assert_eq!(decide(&claims(), allowing), Ok(vec![Uuid::from_u128(1)]));
}

#[test]
fn a_self_hosted_run_in_a_repository_no_policy_names_learns_nothing_about_runners() {
    let mut c = claims();
    c.runner_environment = "self-hosted".into();
    c.repository_id = Some("111".into());
    assert_eq!(decide(&c, rule()), Err(ClaimReject::NoMatchingPolicy));
}

#[test]
fn pull_request_target_is_refused_whatever_the_policy_says() {
    let mut c = claims();
    c.event_name = "pull_request_target".into();
    let anything_goes = PolicyRule {
        environment: None,
        allow_self_hosted: true,
        ..rule()
    };
    assert_eq!(
        decide(&c, anything_goes),
        Err(ClaimReject::PullRequestTarget)
    );
    // And with no policy at all: the event is refused before any lookup.
    assert_eq!(
        ids(match_policies(&c, &[])),
        Err(ClaimReject::PullRequestTarget)
    );
}

#[test]
fn no_policies_never_match() {
    assert_eq!(
        ids(match_policies(&claims(), &[])),
        Err(ClaimReject::NoMatchingPolicy)
    );
}

#[test]
fn every_matching_policy_is_returned_in_the_order_given() {
    let second = PolicyRule {
        policy_id: Uuid::from_u128(2),
        environment: None,
        ..rule()
    };
    let other_repo = PolicyRule {
        policy_id: Uuid::from_u128(3),
        repository_id: 1,
        ..rule()
    };
    let matched = ids(match_policies(&claims(), &[rule(), other_repo, second]));
    assert_eq!(matched, Ok(vec![Uuid::from_u128(1), Uuid::from_u128(2)]));
}

#[test]
fn each_refusal_has_the_contracts_code() {
    assert_eq!(ClaimReject::PullRequestTarget.code(), "pull_request_target");
    assert_eq!(ClaimReject::SelfHostedRunner.code(), "self_hosted_runner");
    assert_eq!(
        ClaimReject::MissingEnvironment.code(),
        "missing_environment"
    );
    assert_eq!(ClaimReject::NoMatchingPolicy.code(), "no_matching_policy");
}
