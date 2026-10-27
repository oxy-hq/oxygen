//! Oxygen's own catalogue: the platform notice kinds, in code.
//!
//! The kinds themselves, their channels and which one is critical are
//! `platform::kinds`; this places them in the tree and labels them. A kind
//! from a newer binary that is not here lands in `general`, as an app's
//! undescribed kind does.

use std::collections::BTreeMap;

use super::model::{Action, Catalogue, CategoryDef, KindDef, Level};
use super::source::{Source, SourceCatalogue};
use crate::server::api::notifications::platform::kinds::{
    ANOMALY, ASK_FAILED, ASK_FINISHED, ASK_WAITING, BILLING_PAST_DUE, CLI_APPROVAL, RUN_FAILED,
    TEST, WORKSPACE_READY,
};
use crate::server::api::notifications::platform::self_test::TITLE as TEST_TITLE;

/// (kind, label, category, category label, level).
const PLATFORM: [(&str, &str, &str, &str, Level); 8] = [
    (
        BILLING_PAST_DUE,
        "Payment past due",
        "billing",
        "Billing",
        Level::Critical,
    ),
    (
        WORKSPACE_READY,
        "Workspace ready",
        "workspaces",
        "Workspaces",
        Level::Active,
    ),
    (
        RUN_FAILED,
        "Run failed",
        "runs",
        "Automations and pipelines",
        Level::TimeSensitive,
    ),
    (
        ANOMALY,
        "Anomaly found",
        "monitoring",
        "Monitoring",
        Level::Active,
    ),
    (
        CLI_APPROVAL,
        "oxyc approval request",
        "access",
        "Access and sign-in",
        Level::TimeSensitive,
    ),
    // An answer the person asked to hear about when they left Ask. The label is
    // what a push says beside "Oxygen": a platform push carries no other words.
    (ASK_FINISHED, "Answer ready", "ask", "Ask", Level::Active),
    // Or it failed or timed out instead.
    (ASK_FAILED, "Couldn't answer", "ask", "Ask", Level::Active),
    // The run they left stopped to ask them a clarifying question.
    (
        ASK_WAITING,
        "Needs your answer",
        "ask",
        "Ask",
        Level::Active,
    ),
];

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
    for (kind, label, category, category_label, level) in PLATFORM {
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
    }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::api::notifications::platform::kinds::{KINDS, is_critical};

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
            crate::server::api::notifications::platform::kinds::channels(CLI_APPROVAL)
        );
        assert_eq!(
            run.channels,
            crate::server::api::notifications::platform::kinds::channels(RUN_FAILED)
        );
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
        let newer = c.describe("oxy.something_new");
        assert_eq!(
            (newer.category.as_str(), newer.label.as_str()),
            ("general", "Something new")
        );
    }
}
