//! The preflight's message, for the migrate Job's log and — when it has news —
//! the custom-apps channel.

use std::collections::HashSet;

use super::{ENV, Findings, Judged};

/// Refusals listed before the message summarises the rest.
const LISTED: usize = 15;

pub fn report(findings: &Findings, blocking: bool) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let breaking: Vec<&Judged> = findings
        .judged
        .iter()
        .filter(|j| j.breaks_working)
        .collect();
    let quiet_new: Vec<&Judged> = findings
        .judged
        .iter()
        .filter(|j| j.new && !j.breaks_working)
        .collect();
    let carried: Vec<&Judged> = findings.judged.iter().filter(|j| !j.new).collect();

    let mut out = if findings.baseline && !findings.judged.is_empty() {
        baseline_header(version, findings.judged.len(), breaking.len())
    } else {
        header(
            version,
            blocking,
            &breaking,
            (quiet_new.len(), carried.len()),
            !findings.unchecked.is_empty(),
        )
    };
    let mut budget = LISTED;
    list(&mut out, &mut budget, None, &breaking);
    let quiet_heading = (!breaking.is_empty())
        .then(|| format!("New, but not working this week ({}):", quiet_new.len()));
    list(&mut out, &mut budget, quiet_heading, &quiet_new);
    let carried_heading = (!breaking.is_empty() || !quiet_new.is_empty()).then(|| {
        format!(
            "Carried over from earlier releases, not blocking ({}):",
            carried.len()
        )
    });
    list(&mut out, &mut budget, carried_heading, &carried);
    let listed = LISTED - budget;
    if findings.judged.len() > listed {
        out.push_str(&format!("\n…and {} more.", findings.judged.len() - listed));
    }
    if !findings.unchecked.is_empty() {
        out.push_str(&format!(
            "\nNot checked: {} function(s) whose workspace config could not be read — see the \
             preflight warnings in this log.",
            findings.unchecked.len()
        ));
    }
    if !breaking.is_empty() {
        out.push_str(
            "\nFix: republish each app with the manifest change its reason names. \
             internal-docs/custom-app-availability-guidelines.md",
        );
    }
    out
}

/// The first run on a deployment records instead of blocking. It cannot tell
/// which refusals this release introduced, so it says so, and lists the ones on
/// working functions first.
fn baseline_header(version: &str, refused: usize, on_working: usize) -> String {
    let check = if on_working > 0 {
        format!(
            " Refusals on functions that answered this week ({on_working}) are listed first: the \
             first run cannot tell whether this release caused them, so check them."
        )
    } else {
        String::new()
    };
    format!(
        ":information_source: First custom-app preflight on this deployment (release {version}): \
         recording the {refused} refusal(s) live apps already carry as the baseline, not \
         blocking. From the next release on, only a refusal a release adds can block.{check}"
    )
}

/// `(quiet_new, carried)`: refusals new but breaking nothing working, and
/// refusals carried over from earlier releases.
fn header(
    version: &str,
    blocking: bool,
    breaking: &[&Judged],
    (quiet_new, carried): (usize, usize),
    some_unchecked: bool,
) -> String {
    let functions: HashSet<_> = breaking
        .iter()
        .map(|j| (j.refusal.app_id, &j.refusal.function))
        .collect();
    let stopping = functions.len();
    if blocking {
        format!(
            ":no_entry: Release {version} would stop {stopping} working custom-app function(s) \
             — rollout blocked ({ENV}=block)."
        )
    } else if stopping > 0 {
        // Not blocking with breaks means the mode is `warn`.
        format!(
            ":warning: Release {version} will stop {stopping} working custom-app function(s) — \
             rolling out anyway ({ENV}=warn)."
        )
    } else if quiet_new > 0 {
        format!(
            ":information_source: Release {version} refuses {quiet_new} new custom-app call(s), \
             on functions that were not working this week."
        )
    } else if carried > 0 {
        format!(
            "custom-app preflight: release {version} refuses nothing new; {carried} refusal(s) \
             carried over from earlier releases."
        )
    } else if some_unchecked {
        // "Every function passes" would be the green that hides a check that
        // never ran; the unchecked count follows.
        format!(
            "custom-app preflight: every live function it could check passes release \
             {version}'s rules."
        )
    } else {
        format!("custom-app preflight: every live function passes release {version}'s rules.")
    }
}

fn list(out: &mut String, budget: &mut usize, heading: Option<String>, judged: &[&Judged]) {
    if judged.is_empty() || *budget == 0 {
        return;
    }
    if let Some(heading) = heading {
        out.push('\n');
        out.push_str(&heading);
    }
    for j in judged.iter().take(*budget) {
        let reason: String = j.refusal.reason.chars().take(220).collect();
        out.push_str(&format!(
            "\n• `{}` · `{}` — {reason}",
            j.refusal.app, j.refusal.function
        ));
        *budget -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Refusal, rule};
    use super::*;
    use uuid::Uuid;

    fn judged(function: &str, new: bool, breaks_working: bool) -> Judged {
        Judged {
            refusal: Refusal {
                app_id: Uuid::nil(),
                app: "poke-house/warehouse".into(),
                function: function.into(),
                rule: rule::CUSTOMER_WAREHOUSE_WRITE,
                database: "clickhouse".into(),
                reason: "database 'clickhouse' is a customer warehouse".into(),
            },
            new,
            breaks_working,
        }
    }

    fn findings(judged: Vec<Judged>) -> Findings {
        Findings {
            judged,
            ..Default::default()
        }
    }

    #[test]
    fn the_first_run_records_a_baseline_and_says_what_to_check() {
        let mut f = findings(vec![
            judged("admin-settings", true, true),
            judged("retired", true, false),
        ]);
        f.baseline = true;
        let text = report(&f, false);
        assert!(text.contains("First custom-app preflight"), "{text}");
        assert!(text.contains("recording the 2 refusal(s)"), "{text}");
        assert!(
            text.contains("answered this week (1) are listed first"),
            "{text}"
        );
        assert!(
            !text.contains("rolling out anyway") && !text.contains("blocked"),
            "{text}"
        );

        // Nothing refused: the ordinary all-clear, not a baseline notice.
        let mut clean = findings(vec![]);
        clean.baseline = true;
        assert!(report(&clean, false).contains("every live function passes"));
    }

    #[test]
    fn a_blocked_release_names_the_function_and_the_fix() {
        let text = report(&findings(vec![judged("admin-settings", true, true)]), true);
        assert!(text.contains("rollout blocked"), "{text}");
        assert!(
            text.contains("`poke-house/warehouse` · `admin-settings`"),
            "{text}"
        );
        assert!(text.contains("republish"), "{text}");
    }

    #[test]
    fn a_warned_release_says_it_rolls_out_anyway() {
        let text = report(&findings(vec![judged("admin-settings", true, true)]), false);
        assert!(text.contains("rolling out anyway"), "{text}");
        assert!(!text.contains("blocked"), "{text}");
    }

    #[test]
    fn news_that_breaks_nothing_working_is_information() {
        let text = report(&findings(vec![judged("retired", true, false)]), false);
        assert!(text.contains("not working this week"), "{text}");
        assert!(
            !text.contains("blocked") && !text.contains("republish"),
            "{text}"
        );
    }

    #[test]
    fn a_carried_refusal_is_listed_as_carried_and_nothing_new() {
        let text = report(
            &findings(vec![
                judged("admin-settings", true, true),
                judged("submit-receiving", false, false),
            ]),
            true,
        );
        assert!(
            text.contains("Carried over from earlier releases, not blocking (1)"),
            "{text}"
        );

        let only = report(
            &findings(vec![judged("submit-receiving", false, false)]),
            false,
        );
        assert!(only.contains("refuses nothing new"), "{only}");
        assert!(only.contains("`submit-receiving`"), "{only}");
    }

    #[test]
    fn unchecked_functions_are_said_not_passed() {
        let mut f = findings(vec![]);
        f.unchecked.insert((Uuid::nil(), "submit-receiving".into()));
        let text = report(&f, false);
        assert!(text.contains("it could check"), "{text}");
        assert!(text.contains("Not checked: 1 function(s)"), "{text}");
    }

    #[test]
    fn a_long_list_is_capped() {
        let many = (0..LISTED + 5)
            .map(|i| judged(&format!("f{i}"), false, false))
            .collect();
        let text = report(&findings(many), false);
        assert_eq!(text.matches("\n• ").count(), LISTED, "{text}");
        assert!(text.ends_with("…and 5 more."), "{text}");
    }
}
