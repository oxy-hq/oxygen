//! The wire contract this endpoint is monitored against — the verdict values,
//! the response shape, and the fixed ladder of checks.
//!
//! Split from the handlers because a monitor is configured against *this* file's
//! decisions (the `oxy_app_health` key, `pass`/`fail`, the check names and their
//! order) and those outlive any particular check's implementation. See the
//! parent module for why the shape is what it is.

use serde::Serialize;
use uuid::Uuid;

/// Verdict values. Chosen so neither is a substring of the other — see the
/// module docs on why `healthy`/`unhealthy` is a trap for a `Contains` matcher.
pub(super) const PASS: &str = "pass";
pub(super) const FAIL: &str = "fail";
pub(super) const SKIPPED: &str = "skipped";

/// The entry file every bundle must serve. Its presence in the build store is
/// the difference between "the registry thinks this app is published" and "a
/// browser hitting it would get something".
pub(super) const ENTRYPOINT: &str = "index.html";

/// Every check this endpoint reports, **in evaluation order**.
///
/// [`skip_remaining`] fills the tail from this list, so a bail-out cannot
/// silently omit a check — adding one here is enough to have every earlier exit
/// report it as `skipped`. The alternative (each early return listing the
/// remaining checks by hand) is exactly where the next check would be forgotten.
/// The happy path is held to the same list by a `debug_assert!` in
/// [`super::respond`], because `skip_remaining` alone would let a newly added
/// name be silently absent from every *passing* body.
///
/// `source_config` comes second on purpose: an app whose source oxy doesn't
/// serve fails at dispatch, so no later rung means anything for it. The name
/// dates from when there were several source kinds; it stays because monitors
/// match on it.
pub(super) const LADDER: [&str; 5] = [
    "registered",
    "source_config",
    "published",
    "build_record",
    "bundle_entrypoint",
];

#[derive(Serialize)]
pub struct HealthResponse {
    /// `"pass"` or `"fail"`. Named so it cannot collide with anything in the
    /// SPA fall-through shell — assert on `"oxy_app_health":"pass"`.
    pub oxy_app_health: &'static str,
    pub app: AppRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildRef>,
    /// Ordered per [`LADDER`]; the first non-`pass` entry is the reason.
    pub checks: Vec<Check>,
    pub checked_at: String,
}

#[derive(Serialize)]
pub struct AppRef {
    pub id: Uuid,
    pub org_slug: String,
    pub slug: String,
}

#[derive(Serialize)]
pub struct BuildRef {
    /// The engineer-facing version string (`app_builds.build_id`) — a git sha or
    /// CI run id. Useful in a monitor's alert body: "still failing, still on
    /// build X" is a different incident from "started failing after build Y".
    pub build_id: String,
    pub published_at: Option<String>,
}

#[derive(Serialize)]
pub struct Check {
    pub name: &'static str,
    pub result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Check {
    pub(super) fn pass(name: &'static str) -> Self {
        Self {
            name,
            result: PASS,
            detail: None,
        }
    }
    pub(super) fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            result: FAIL,
            detail: Some(detail.into()),
        }
    }
    /// Not evaluated — because an earlier check already decided the verdict, or
    /// because this app's source kind puts the answer outside what oxy hosts.
    /// Deliberately neither `pass` nor `fail`; see [`super::respond`].
    pub(super) fn skipped(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            result: SKIPPED,
            detail: Some(detail.into()),
        }
    }
}

/// Fill the rest of [`LADDER`] with `skipped`, given `checks` holds one entry
/// per ladder name so far.
pub(super) fn skip_remaining(checks: &mut Vec<Check>, why: &str) {
    for name in LADDER.iter().skip(checks.len()) {
        checks.push(Check::skipped(name, why));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole endpoint exists because a monitor cannot tell the SPA shell
    /// from a healthy answer. These two properties are what make the body
    /// assertable, so they are pinned rather than left to review.
    #[test]
    fn verdict_values_are_not_substrings_of_each_other() {
        assert!(!PASS.contains(FAIL) && !FAIL.contains(PASS));
        // The trap this avoids: `Contains "healthy"` matches "unhealthy".
        assert!("unhealthy".contains("healthy"));
    }

    #[test]
    fn a_failing_check_flips_the_verdict_in_the_body() {
        let body = serde_json::to_string(&HealthResponse {
            oxy_app_health: FAIL,
            app: AppRef {
                id: Uuid::nil(),
                org_slug: "acme".into(),
                slug: "console".into(),
            },
            build: None,
            checks: vec![Check::fail("published", "no published build")],
            checked_at: "2026-08-21T00:00:00Z".into(),
        })
        .unwrap();
        assert!(body.contains(r#""oxy_app_health":"fail""#));
        assert!(!body.contains("<!doctype"));
    }

    /// `skipped` must not read as healthy *or* as broken: a rung we could not
    /// evaluate is neither a failure nor a check we ran.
    #[test]
    fn skipped_checks_do_not_fail_the_verdict() {
        let checks = vec![
            Check::pass("registered"),
            Check::skipped("bundle_entrypoint", "not evaluated"),
        ];
        assert!(!checks.iter().any(|c| c.result == FAIL));
    }

    /// The reason `LADDER` exists: an early bail-out must still report every
    /// later check, so a monitor's body always has the same shape and a check
    /// added later cannot be silently dropped from an existing exit path.
    #[test]
    fn a_bail_out_still_reports_every_remaining_check() {
        // Bail at the earliest rung that can fail — the source, which is now
        // second because it decides what every later rung means.
        let mut checks = vec![
            Check::pass("registered"),
            Check::fail("source_config", "unparseable"),
        ];
        skip_remaining(&mut checks, "source configuration is unreadable");
        let names: Vec<&str> = checks.iter().map(|c| c.name).collect();
        assert_eq!(names, LADDER.to_vec());
        assert!(
            checks[2..].iter().all(|c| c.result == SKIPPED),
            "everything after the failure is unevaluated, not failed"
        );
    }
}
