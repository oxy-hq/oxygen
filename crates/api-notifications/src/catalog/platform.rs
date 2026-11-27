//! Oxygen's own catalogue: the platform notice kinds, in code.
//!
//! The kinds themselves, their channels and which one is critical are
//! `platform::kinds`; this places them in the tree and labels them. A kind
//! from a newer binary that is not here lands in `general`, as an app's
//! undescribed kind does.
//!
//! It also declares each kind's [`Audience`] — who it is for. That one
//! statement decides what a person's catalogue and settings draw and which
//! stream the kind is in (`crate::stream`), so a crew member is not shown a
//! Billing switch for a notice only an owner ever receives.

use std::collections::BTreeMap;

use super::model::{Action, Catalogue, CategoryDef, KindDef, Level};
use super::source::{Audience, Source, SourceCatalogue};
use crate::platform::kinds::{
    ANOMALY, ASK_FAILED, ASK_FINISHED, ASK_WAITING, BILLING_PAST_DUE, CLI_APPROVAL,
    PRODUCTION_CHECK_FAILED, RUN_FAILED, TEST, WORKSPACE_READY,
};
use crate::platform::self_test::TITLE as TEST_TITLE;

/// (kind, label, category, category label, level, audience).
///
/// The audience of each is the standing its **recipients already hold**, never a
/// wish: `billing_past_due` is `Owners` because `api-billing` sends it to owners
/// and nobody else, and the unit test below asserts that pairing rather than
/// trusting this comment.
const PLATFORM: [(&str, &str, &str, &str, Level, Audience); 9] = [
    (
        BILLING_PAST_DUE,
        "Payment past due",
        "billing",
        "Billing",
        Level::Critical,
        // `billing/notify.rs` sends it to `OrgRole::Owner` and nobody else.
        Audience::Owners,
    ),
    (
        WORKSPACE_READY,
        "Workspace ready",
        "workspaces",
        "Workspaces",
        Level::Active,
        // "Your apps are ready": the people sitting on the being-set-up screen,
        // which is why it broadcasts to every member.
        Audience::Everyone,
    ),
    (
        RUN_FAILED,
        "Run failed",
        "runs",
        "Automations and pipelines",
        Level::TimeSensitive,
        // Nobody who cannot act on a failed pipeline wants a switch for one.
        // Its RECIPIENTS are not narrowed to this audience: a workspace admin
        // who is a plain org member is told today and keeps being told
        // (`internal-docs/2026-11-24-notification-segmentation-design.md` 3.1).
        Audience::Operators,
    ),
    (
        ANOMALY,
        "Anomaly found",
        "monitoring",
        "Monitoring",
        Level::Active,
        // Whoever a monitor's `notify.people` names — any role.
        Audience::Addressed,
    ),
    (
        CLI_APPROVAL,
        "oxyc approval request",
        "access",
        "Access and sign-in",
        Level::TimeSensitive,
        // The one person who typed their own address into `oxyc`.
        Audience::Addressed,
    ),
    // An answer the person asked to hear about when they left Ask. The label is
    // what a push says beside "Oxygen": a platform push carries no other words.
    (
        ASK_FINISHED,
        "Answer ready",
        "ask",
        "Ask",
        Level::Active,
        // Only the person who asked, and only because they asked to hear.
        Audience::Addressed,
    ),
    // Or it failed or timed out instead.
    (
        ASK_FAILED,
        "Couldn't answer",
        "ask",
        "Ask",
        Level::Active,
        Audience::Addressed,
    ),
    // The run they left stopped to ask them a clarifying question.
    (
        ASK_WAITING,
        "Needs your answer",
        "ask",
        "Ask",
        Level::Active,
        Audience::Addressed,
    ),
    // A build that just went live shows new errors on production: the first
    // kind of an app's own health. `Addressed`, because it goes to named
    // people (whoever moved production, and the org's owners and admins) and
    // the mover need not be an officer: a standing would hide it from them.
    (
        PRODUCTION_CHECK_FAILED,
        "Production check failed",
        APPS,
        "App health",
        Level::TimeSensitive,
        Audience::Addressed,
    ),
];

/// The category a custom app's own health will be told in; see [`oxygen`].
pub const APPS: &str = "apps";

/// Every platform notice opens what it is about: its rendered link.
fn open() -> Action {
    Action {
        id: "open".to_string(),
        label: "Open".to_string(),
        function: None,
        destructive: false,
    }
}

/// Oxygen's tree.
pub fn oxygen() -> SourceCatalogue {
    let mut categories = BTreeMap::new();
    let mut kinds = BTreeMap::new();
    let mut audiences = BTreeMap::new();
    for (kind, label, category, category_label, level, audience) in PLATFORM {
        categories.insert(
            category.to_string(),
            CategoryDef {
                label: Some(category_label.to_string()),
                description: None,
            },
        );
        kinds.insert(
            kind.to_string(),
            KindDef {
                category: Some(category.to_string()),
                label: Some(label.to_string()),
                description: None,
                level: Some(level),
                actions: vec![open()],
            },
        );
        audiences.insert(kind.to_string(), audience);
    }
    // A custom app's own health. Its first kind is the production check's
    // notice (in `PLATFORM`); a dead-lettered function and a failed deployment
    // are not told to anyone yet. The label is declared here as well so the
    // category reads the same whichever kind names it.
    categories.insert(
        APPS.to_string(),
        CategoryDef {
            label: Some("App health".to_string()),
            description: None,
        },
    );
    // A test notice a person sent themselves: in `general`, and given no
    // settings switch (`KindInfo::has_switch`) — no mute applies to it.
    kinds.insert(
        TEST.to_string(),
        KindDef {
            category: None,
            label: Some(TEST_TITLE.to_string()),
            description: None,
            level: Some(Level::Active),
            actions: vec![open()],
        },
    );
    SourceCatalogue {
        source: Source::Oxygen,
        catalogue: Catalogue { categories, kinds },
        declared: BTreeMap::new(),
        audiences,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::kinds::{KINDS, is_critical};

    /// Every platform kind and who it is for, with **where its recipients are
    /// chosen** named beside it.
    ///
    /// This table is the test. An audience that does not match the people a
    /// source actually sends to is the failure this whole segmentation would
    /// create rather than fix: a switch for a notice you can never receive, or
    /// worse, a category hidden from someone who is receiving one. The first
    /// draft of the design classified billing as "owners and admins" when
    /// `api-billing` sends it to owners alone, and nothing would have caught
    /// it. Changing a row means going and reading the named file first.
    const RECIPIENTS: [(&str, Audience, &str); 10] = [
        (
            BILLING_PAST_DUE,
            Audience::Owners,
            "api-billing's billing/notify.rs: org_members_from(.., OrgRole::Owner)",
        ),
        (
            WORKSPACE_READY,
            Audience::Everyone,
            "app-core's platform/workspace_ready.rs: every member of the org",
        ),
        (
            RUN_FAILED,
            Audience::Operators,
            "platform/run_failed.rs: whoever started the run, else the \
             WORKSPACE's own admins (people::workspace_admins) — which is NOT \
             the same set as this audience, and deliberately not narrowed to \
             it (design 3.1)",
        ),
        (
            ANOMALY,
            Audience::Addressed,
            "oxy-api-metric-monitoring's anomaly_notify::people: the people a \
             monitor's notify.people names, of any role",
        ),
        (
            CLI_APPROVAL,
            Audience::Addressed,
            "platform/cli_approval.rs: the one address oxyc was given",
        ),
        (
            ASK_FINISHED,
            Audience::Addressed,
            "platform/ask_finished.rs: the thread's owner, on request",
        ),
        (
            ASK_FAILED,
            Audience::Addressed,
            "platform/ask_finished.rs, the same person",
        ),
        (
            ASK_WAITING,
            Audience::Addressed,
            "platform/ask_waiting.rs: the person whose run stopped to ask",
        ),
        (
            TEST,
            Audience::Everyone,
            "platform/self_test.rs: the person who pressed the button, and it \
             must work for anyone",
        ),
        (
            PRODUCTION_CHECK_FAILED,
            Audience::Addressed,
            "api-custom-apps' custom_apps_build_checks/production/followup.rs \
             recipients(): whoever moved production, by id, and the org's \
             owners and admins; the mover need not be an officer",
        ),
    ];

    #[test]
    fn every_kind_declares_who_it_is_for_and_it_matches_who_it_is_sent_to() {
        let c = oxygen();
        for (kind, audience, recipients) in RECIPIENTS {
            assert_eq!(
                c.audience_of(kind),
                audience,
                "{kind} is sent to {recipients}"
            );
            assert_eq!(
                c.describe(kind).audience,
                audience,
                "{kind} through describe"
            );
        }
        // Exhaustive the other way too, so a new kind cannot be added without
        // someone deciding whose it is.
        for kind in KINDS {
            assert!(
                RECIPIENTS.iter().any(|(k, _, _)| *k == kind),
                "{kind} has no audience decision in RECIPIENTS"
            );
        }
        // A kind this binary has never heard of is shown, not hidden.
        assert_eq!(c.audience_of("oxy.something_new"), Audience::Everyone);
    }

    #[test]
    fn the_app_health_category_is_labelled_and_holds_the_production_check_alone() {
        let c = oxygen();
        assert_eq!(c.label_of(APPS), "App health");
        let in_it: Vec<&str> = c
            .catalogue
            .kinds
            .iter()
            .filter(|(_, k)| k.category.as_deref() == Some(APPS))
            .map(|(kind, _)| kind.as_str())
            .collect();
        assert_eq!(
            in_it,
            [PRODUCTION_CHECK_FAILED],
            "a new kind of app health picks an audience in RECIPIENTS first"
        );
    }

    #[test]
    fn every_platform_kind_is_placed_and_only_billing_is_critical() {
        let c = oxygen();
        for kind in KINDS {
            let info = c.describe(kind);
            if kind == TEST {
                continue;
            }
            assert_ne!(info.category, "general", "{kind} is placed");
            assert_eq!(info.critical(), is_critical(kind), "{kind}");
            assert_eq!(info.actions.len(), 1, "{kind}");
            assert!(info.actions[0].function.is_none(), "{kind} opens");
        }
        let test = c.describe(TEST);
        assert_eq!(
            (test.category.as_str(), test.label.as_str()),
            ("general", "Test notification")
        );
        assert!(
            !test.mutable && !test.has_switch(),
            "no mute applies to a test"
        );
        assert!(c.describe(WORKSPACE_READY).mutable);
        assert!(!c.describe(BILLING_PAST_DUE).mutable && c.describe(BILLING_PAST_DUE).has_switch());
        let run = c.describe(RUN_FAILED);
        assert_eq!(run.category_labels, vec!["Automations and pipelines"]);
        assert_eq!(run.level, Level::TimeSensitive);
        let approval = c.describe(CLI_APPROVAL);
        assert_eq!(approval.label, "oxyc approval request");
        assert_eq!(approval.category_labels, vec!["Access and sign-in"]);
        assert_eq!(approval.level, Level::TimeSensitive);
        assert!(approval.mutable && approval.has_switch());
        assert_eq!(
            approval.channels,
            crate::platform::kinds::channels(CLI_APPROVAL)
        );
        assert_eq!(run.channels, crate::platform::kinds::channels(RUN_FAILED));
        for (kind, label) in [
            (ASK_FINISHED, "Answer ready"),
            (ASK_FAILED, "Couldn't answer"),
            (ASK_WAITING, "Needs your answer"),
        ] {
            let ask = c.describe(kind);
            assert_eq!(
                (ask.label.as_str(), ask.category.as_str(), ask.level),
                (label, "ask", Level::Active),
                "{kind}"
            );
            assert_eq!(ask.category_labels, vec!["Ask"], "{kind}");
            assert_eq!(ask.channels, vec!["push"], "{kind}");
            assert_eq!(ask.actions.len(), 1, "{kind}");
            assert_eq!(
                (ask.actions[0].id.as_str(), ask.actions[0].label.as_str()),
                ("open", "Open"),
                "{kind}"
            );
        }
        let failed = c.describe(PRODUCTION_CHECK_FAILED);
        assert_eq!(
            (
                failed.label.as_str(),
                failed.category.as_str(),
                failed.level
            ),
            ("Production check failed", "apps", Level::TimeSensitive)
        );
        assert_eq!(failed.category_labels, vec!["App health"]);
        assert_eq!(failed.channels, vec!["push", "email", "slack"]);
        assert!(failed.mutable && failed.has_switch());
        let newer = c.describe("oxy.something_new");
        assert_eq!(
            (newer.category.as_str(), newer.label.as_str()),
            ("general", "Something new")
        );
    }
}
