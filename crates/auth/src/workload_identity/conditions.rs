//! A trust policy's claim conditions, and the pure decision of which policy a
//! verified token matches.
//!
//! A condition names one **top-level scalar claim** of the token and the value
//! it must have:
//!
//! - an exact value — `arn:aws:iam::123456789012:role/ferret-task` — compared
//!   byte for byte, case included;
//! - or a prefix ending in one `*` — `repo:acme/app:ref:refs/heads/*` — which
//!   matches any value that starts with the prefix. The prefix may not be
//!   empty, so a bare `*` is refused, and `*` anywhere but last is refused
//!   (there is no escape: a value cannot end in a literal `*`).
//!
//! A string claim is compared as it is; a number or boolean as its JSON text
//! (`123`, `true`). An object, an array, or a missing claim matches nothing —
//! except `aud`, which a verified token carries as this deployment's audience
//! alone, sometimes as a one-element list.
//!
//! `iss` is never a condition: the issuer is the policy's issuer, checked when
//! the token is verified. Nor are `exp`, `iat`, `nbf` or `jti`, which differ on
//! every token. A policy needs at least one condition on something other than
//! `aud`: every token for this deployment carries that audience, so a policy on
//! it alone would admit every token the issuer signs.
//!
//! **One policy or none.** The exchange evaluates the named account's enabled
//! policies on the token's issuer. Exactly one must match ([`pick`]); when
//! several do, the exchange is refused rather than choosing one or merging
//! their grants.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The most conditions one policy may have.
pub const MAX_CONDITIONS: usize = 16;
/// The longest claim name.
pub const CLAIM_MAX_CHARS: usize = 128;
/// The longest value.
pub const VALUE_MAX_CHARS: usize = 512;
/// The one wildcard: last, after a non-empty prefix.
pub const WILDCARD: char = '*';

/// Claims no condition may name: the issuer is the policy's own, and the rest
/// differ on every token.
pub const NOT_CONDITIONS: [&str; 5] = ["iss", "exp", "iat", "nbf", "jti"];

/// `{"claim": "sub", "value": "arn:aws:iam::123456789012:role/ferret-task"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub claim: String,
    pub value: String,
}

impl Condition {
    pub fn new(claim: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            claim: claim.into(),
            value: value.into(),
        }
    }
}

fn check_claim(raw: &str) -> Result<String, String> {
    let claim = raw.trim();
    if claim.is_empty() || claim.chars().count() > CLAIM_MAX_CHARS {
        return Err(format!(
            "a condition's 'claim' must be 1 to {CLAIM_MAX_CHARS} characters"
        ));
    }
    if claim.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "the claim '{claim}' holds a space: name a top-level claim of the token"
        ));
    }
    if NOT_CONDITIONS.contains(&claim) {
        return Err(if claim == "iss" {
            "'iss' is never a condition: it is the issuer's, checked when the token is verified"
                .to_string()
        } else {
            format!("'{claim}' differs on every token, so it cannot be a condition")
        });
    }
    Ok(claim.to_string())
}

fn check_value(claim: &str, raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() || value.chars().count() > VALUE_MAX_CHARS {
        return Err(format!(
            "the value for '{claim}' must be 1 to {VALUE_MAX_CHARS} characters"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("the value for '{claim}' holds a control character"));
    }
    let wildcards = value.matches(WILDCARD).count();
    let trailing_only = wildcards == 0 || (wildcards == 1 && value.ends_with(WILDCARD));
    if !trailing_only {
        return Err(format!(
            "the value for '{claim}' may hold one '*', and only as its last character"
        ));
    }
    if value == "*" {
        return Err(format!(
            "the value for '{claim}' is a bare '*', which matches anything: give a prefix \
             before it, or leave the condition out"
        ));
    }
    Ok(value.to_string())
}

/// The conditions a policy may be written with: each claim and value checked,
/// no claim twice, at least one condition not on `aud`, and at most
/// [`MAX_CONDITIONS`]. Handed back trimmed and ordered by claim, so two equal
/// sets compare equal.
pub fn check(conditions: &[Condition]) -> Result<Vec<Condition>, String> {
    if conditions.len() > MAX_CONDITIONS {
        return Err(format!("a policy has at most {MAX_CONDITIONS} conditions"));
    }
    let mut out = Vec::with_capacity(conditions.len());
    for condition in conditions {
        let claim = check_claim(&condition.claim)?;
        let value = check_value(&claim, &condition.value)?;
        if out.iter().any(|c: &Condition| c.claim == claim) {
            return Err(format!(
                "'{claim}' is named twice: a claim has one value, so give it one condition"
            ));
        }
        out.push(Condition { claim, value });
    }
    if !out.iter().any(|c| c.claim != "aud") {
        return Err(
            "a policy needs a condition on a claim other than 'aud': every token for this \
             deployment carries its audience, so 'aud' alone would admit every token the issuer \
             signs"
                .into(),
        );
    }
    out.sort_by(|a, b| a.claim.cmp(&b.claim));
    Ok(out)
}

/// The conditions a stored policy holds. A row that does not read as a list
/// of conditions reads as none — and a policy with none matches nothing.
pub fn stored(value: &Value) -> Vec<Condition> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

/// Whether `actual` meets `pattern`: equal, or — for a pattern ending in
/// `*` — starting with what precedes it.
pub fn value_matches(pattern: &str, actual: &str) -> bool {
    match pattern.strip_suffix(WILDCARD) {
        Some(prefix) if !prefix.is_empty() => actual.starts_with(prefix),
        Some(_) => false,
        None => actual == pattern,
    }
}

/// The text a top-level scalar claim is compared as, if it is one.
fn scalar(claims: &Map<String, Value>, claim: &str) -> Option<String> {
    match claims.get(claim)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(one) if claim == "aud" && one.len() == 1 => {
            one[0].as_str().map(str::to_string)
        }
        _ => None,
    }
}

/// Whether `claims` meet every one of `conditions`. No conditions is no match.
pub fn matches(claims: &Map<String, Value>, conditions: &[Condition]) -> bool {
    !conditions.is_empty()
        && conditions.iter().all(|c| {
            scalar(claims, &c.claim).is_some_and(|actual| value_matches(&c.value, &actual))
        })
}

/// What [`pick`] decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Pick<'a, T> {
    /// Exactly this one matches.
    One(&'a T),
    /// None matches.
    None,
    /// This many match: refused, never merged.
    Ambiguous(usize),
}

/// The one policy of `policies` whose conditions `claims` meet.
pub fn pick<'a, T>(
    claims: &Map<String, Value>,
    policies: &'a [T],
    conditions_of: impl Fn(&T) -> Vec<Condition>,
) -> Pick<'a, T> {
    let mut found = policies
        .iter()
        .filter(|policy| matches(claims, &conditions_of(policy)));
    match (found.next(), found.count()) {
        (None, _) => Pick::None,
        (Some(one), 0) => Pick::One(one),
        (Some(_), more) => Pick::Ambiguous(more + 1),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn c(claim: &str, value: &str) -> Condition {
        Condition::new(claim, value)
    }

    fn claims(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("an object")
    }

    fn pod() -> Map<String, Value> {
        claims(json!({
            "iss": "https://oidc.eks.example.com/id/X",
            "sub": "system:serviceaccount:kestra:worker",
            "aud": ["oxy:app.oxygen-hq.com"],
            "repository_id": "123456",
            "run_number": 42,
            "fork": false,
            "kubernetes.io": { "namespace": "kestra" },
            "groups": ["a", "b"],
        }))
    }

    #[test]
    fn an_exact_value_matches_only_itself() {
        let sub = [c("sub", "system:serviceaccount:kestra:worker")];
        assert!(matches(&pod(), &sub));
        for other in [
            "system:serviceaccount:kestra:worker2",
            "system:serviceaccount:kestra:Worker",
            "system:serviceaccount:kestra:",
            "system:serviceaccount:kestra:work",
        ] {
            assert!(!matches(&pod(), &[c("sub", other)]), "{other}");
        }
    }

    #[test]
    fn a_trailing_star_matches_the_prefix_and_nothing_shorter() {
        assert!(value_matches("refs/heads/*", "refs/heads/main"));
        assert!(value_matches("refs/heads/*", "refs/heads/"));
        assert!(!value_matches("refs/heads/*", "refs/heads"));
        assert!(!value_matches("refs/heads/*", "refs/tags/v1"));
        assert!(matches(
            &pod(),
            &[c("sub", "system:serviceaccount:kestra:*")]
        ));
        assert!(!matches(
            &pod(),
            &[c("sub", "system:serviceaccount:other:*")]
        ));
        // A '*' is a wildcard only last; a stored bare one matches nothing.
        assert!(!value_matches("*", "anything"));
        assert!(!value_matches("refs/*/main", "refs/heads/main"));
    }

    #[test]
    fn a_bare_star_or_one_in_the_middle_is_refused() {
        for bad in ["*", " * ", "refs/*/main", "*main", "a**", "a*b*"] {
            assert!(check(&[c("sub", bad)]).is_err(), "{bad:?}");
        }
        for good in ["a*", "refs/heads/*", "exact"] {
            assert!(check(&[c("sub", good)]).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn a_policy_on_the_audience_alone_is_refused() {
        assert!(check(&[c("aud", "oxy:app.oxygen-hq.com")]).is_err());
        assert!(check(&[]).is_err());
        let both = check(&[c("aud", "oxy:app.oxygen-hq.com"), c("sub", "x")]);
        assert!(both.is_ok());
        // The verified audience, alone in a list, still meets an 'aud' condition.
        assert!(matches(
            &pod(),
            &[
                c("aud", "oxy:app.oxygen-hq.com"),
                c("sub", "system:serviceaccount:kestra:worker")
            ]
        ));
    }

    #[test]
    fn iss_and_per_token_claims_are_never_conditions() {
        for claim in ["iss", "exp", "iat", "nbf", "jti"] {
            assert!(check(&[c(claim, "x"), c("sub", "y")]).is_err(), "{claim}");
        }
        for bad in ["", " ", "has space", &"a".repeat(CLAIM_MAX_CHARS + 1)] {
            assert!(check(&[c(bad, "x")]).is_err(), "{bad:?}");
        }
        assert!(check(&[c("sub", "a"), c("sub", "b")]).is_err(), "twice");
        assert!(check(&[c("sub", "")]).is_err(), "empty value");
        let many: Vec<_> = (0..=MAX_CONDITIONS)
            .map(|i| c(&format!("c{i}"), "x"))
            .collect();
        assert!(check(&many).is_err());
    }

    #[test]
    fn checked_conditions_are_trimmed_and_ordered() {
        let checked = check(&[c(" sub ", " x "), c("aud", "y")]).unwrap();
        assert_eq!(checked, vec![c("aud", "y"), c("sub", "x")]);
    }

    #[test]
    fn only_top_level_scalars_match() {
        assert!(matches(&pod(), &[c("repository_id", "123456")]));
        assert!(matches(&pod(), &[c("run_number", "42")]));
        assert!(matches(&pod(), &[c("fork", "false")]));
        assert!(!matches(&pod(), &[c("kubernetes.io", "kestra")]));
        assert!(!matches(&pod(), &[c("groups", "a")]));
        assert!(!matches(&pod(), &[c("missing", "x")]));
        assert!(!matches(&pod(), &[]), "no conditions, no match");
        // Every condition must hold.
        assert!(!matches(
            &pod(),
            &[
                c("sub", "system:serviceaccount:kestra:worker"),
                c("fork", "true")
            ]
        ));
    }

    #[test]
    fn exactly_one_policy_matches_or_the_exchange_is_refused() {
        let worker = vec![c("sub", "system:serviceaccount:kestra:worker")];
        let kestra = vec![c("sub", "system:serviceaccount:kestra:*")];
        let other = vec![c("sub", "system:serviceaccount:other:x")];
        let one = [other.clone(), worker.clone()];
        assert_eq!(pick(&pod(), &one, Clone::clone), Pick::One(&worker));
        let none = [other.clone()];
        assert_eq!(pick(&pod(), &none, Clone::clone), Pick::None);
        let two = [worker.clone(), other, kestra];
        assert_eq!(pick(&pod(), &two, Clone::clone), Pick::Ambiguous(2));
    }

    #[test]
    fn a_stored_list_that_does_not_parse_matches_nothing() {
        assert!(stored(&json!("nope")).is_empty());
        assert!(stored(&json!([{ "claim": "sub" }])).is_empty());
        let good = stored(&json!([{ "claim": "sub", "value": "x" }]));
        assert_eq!(good, vec![c("sub", "x")]);
    }

    #[test]
    fn the_github_policy_columns_fit_as_conditions() {
        // What the follow-up moves GitHub policies into: each column becomes
        // one condition this model can hold.
        let github = [
            c("repository_id", "123456"),
            c("repository_owner_id", "7890"),
            c(
                "job_workflow_ref",
                "acme/app/.github/workflows/deploy.yml@*",
            ),
            c("ref", "refs/heads/*"),
            c("environment", "production"),
            c("runner_environment", "github-hosted"),
        ];
        let checked = check(&github).expect("fits");
        let token = claims(json!({
            "repository_id": "123456",
            "repository_owner_id": "7890",
            "job_workflow_ref": "acme/app/.github/workflows/deploy.yml@refs/heads/main",
            "ref": "refs/heads/main",
            "environment": "production",
            "runner_environment": "github-hosted",
        }));
        assert!(matches(&token, &checked));
    }
}
