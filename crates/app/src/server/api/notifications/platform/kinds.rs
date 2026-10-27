//! The platform's notice kinds, and what each one is about.
//!
//! A closed list: every `oxy.*` kind is named here with its subject, whether
//! it can be muted, and which channels it asks for. An app cannot send one
//! (`input::is_reserved_kind`), so a kind outside this list on a signal is a
//! row from a newer binary, rendered as a plain "Oxygen notice" rather than
//! guessed at.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::api::notifications::input::RESERVED_KIND_PREFIX;

pub const BILLING_PAST_DUE: &str = "oxy.billing_past_due";
pub const WORKSPACE_READY: &str = "oxy.workspace_ready";
pub const RUN_FAILED: &str = "oxy.run_failed";
pub const ANOMALY: &str = "oxy.anomaly";
/// `oxyc … --no-browser --approver <email>` asks this person to approve.
pub const CLI_APPROVAL: &str = "oxy.cli_approval";
/// A notice a person sent themselves (`POST /api/notifications/test`).
pub const TEST: &str = "oxy.test";
/// An Ask answer someone asked to hear about is ready
/// (`POST …/analytics/runs/{id}/notify`; `ask_finished`).
pub const ASK_FINISHED: &str = "oxy.ask_finished";
/// That Ask run failed or timed out instead (`ask_finished`).
pub const ASK_FAILED: &str = "oxy.ask_failed";
/// An Ask run someone asked to hear about stopped to ask them a clarifying
/// question (`ask_waiting`).
pub const ASK_WAITING: &str = "oxy.ask_waiting";

/// Every kind the platform sends.
pub const KINDS: [&str; 9] = [
    BILLING_PAST_DUE,
    WORKSPACE_READY,
    RUN_FAILED,
    ANOMALY,
    CLI_APPROVAL,
    TEST,
    ASK_FINISHED,
    ASK_FAILED,
    ASK_WAITING,
];

/// Whether `kind` is one the platform sends.
pub fn is_platform_kind(kind: &str) -> bool {
    kind.starts_with(RESERVED_KIND_PREFIX) && KINDS.contains(&kind)
}

/// A critical kind ignores mutes. Only billing: an owner who muted it would
/// learn of a failed payment when the org is suspended.
pub fn is_critical(kind: &str) -> bool {
    kind == BILLING_PAST_DUE
}

/// Whether a person's mutes apply to `kind`. Not to a critical kind, and not
/// to a test notice: it exists to show whether push reaches this device, so a
/// mute that silenced it would make the button report success and deliver
/// nothing. Unlike billing it is not critical — it asks for nothing urgent.
/// It has no settings switch either ([`super::preferences::kinds`]).
pub fn is_mutable(kind: &str) -> bool {
    !is_critical(kind) && kind != TEST
}

/// The channels a platform kind is delivered on, so the only ones its settings
/// switches can change. Push always.
///
/// Email only for a failed run: someone has to act, possibly away from
/// Oxygen. Billing already sends the owner its own past-due email
/// (`billing/notify.rs`), so asking for another would tell them twice.
///
/// A Slack DM, where the org connected Slack, for the three a person has to
/// act on: a failed run, a past-due bill, and an anomaly someone was named
/// for (a monitor's own `slack_channel` post is the broadcast; this is the
/// named person). A CLI approval asks for all three channels: the person is
/// away from the terminal that is waiting on them, so push, email and Slack
/// are each a way to reach their phone. A ready workspace is news its creator is looking at. A test
/// notice tests push and the inbox, nothing else. A finished or failed Ask
/// answer, and an Ask run waiting on the person, were asked for from the phone
/// that left them, so push only.
pub fn channels(kind: &str) -> &'static [&'static str] {
    match kind {
        RUN_FAILED | CLI_APPROVAL => &["push", "email", "slack"],
        BILLING_PAST_DUE | ANOMALY => &["push", "slack"],
        _ => &["push"],
    }
}

/// The record a notice is about, by kind and id — never its text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Subject {
    /// `billing`, `workspace`, `automation_run`, `airway_run`, `anomaly` or
    /// `cli_device_request`.
    pub kind: &'static str,
    pub id: String,
}

/// A past-due episode of the signal's org, by the end of its grace period.
pub const SUBJECT_BILLING: &str = "billing";
pub const SUBJECT_WORKSPACE: &str = "workspace";
pub const SUBJECT_AUTOMATION_RUN: &str = "automation_run";
pub const SUBJECT_AIRWAY_RUN: &str = "airway_run";
pub const SUBJECT_ANOMALY: &str = "anomaly";
/// A device-code request, by its user code without the dash: unique while the
/// request exists, and never the device code.
pub const SUBJECT_CLI_DEVICE_REQUEST: &str = "cli_device_request";
/// A test notice: its id is a [`TestSubject`].
pub const SUBJECT_TEST: &str = "test";
/// An analytics (Ask) run, by its id.
pub const SUBJECT_ANALYTICS_RUN: &str = "analytics_run";
/// One suspension of an analytics run: its id is a [`SuspensionSubject`].
pub const SUBJECT_ANALYTICS_SUSPENSION: &str = "analytics_suspension";

/// The subject kind as stored, mapped back to its `'static` name. `None` for
/// a name this binary does not know.
pub fn subject_kind(stored: &str) -> Option<&'static str> {
    [
        SUBJECT_BILLING,
        SUBJECT_WORKSPACE,
        SUBJECT_AUTOMATION_RUN,
        SUBJECT_AIRWAY_RUN,
        SUBJECT_ANOMALY,
        SUBJECT_CLI_DEVICE_REQUEST,
        SUBJECT_TEST,
        SUBJECT_ANALYTICS_RUN,
        SUBJECT_ANALYTICS_SUSPENSION,
    ]
    .into_iter()
    .find(|k| *k == stored)
}

/// What a test notice is about: the test itself. Stored as the subject id
/// (JSON), so the notice renders with no record to look up, as billing's
/// episode does. A moment and an id, never text: like every platform notice,
/// a test stores nothing it says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSubject {
    /// When it was sent. Unique per person (the route allows one per 10
    /// seconds), which makes the subject unique and the dedupe a no-op.
    pub at: DateTime<Utc>,
    /// The API token that sent it (`api_tokens.id`), named when rendered;
    /// `None` for a login session (the web app, the mobile app).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<Uuid>,
}

impl TestSubject {
    pub fn id(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// `None` for an id this binary cannot read.
    pub fn parse(id: &str) -> Option<Self> {
        serde_json::from_str(id).ok()
    }
}

/// One stop of a run to ask the person something: the run, and when that
/// suspension began (`agentic_run_suspensions.created_at`, which each new
/// suspension of the run resets). A run that asks twice is two subjects, so
/// the at-most-once check tells the person of each; the same suspension
/// observed twice is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuspensionSubject {
    pub run: String,
    pub at: DateTime<Utc>,
}

impl SuspensionSubject {
    pub fn id(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// `None` for an id this binary cannot read.
    pub fn parse(id: &str) -> Option<Self> {
        serde_json::from_str(id).ok()
    }
}

/// One thing the platform tells people. Each variant carries the ids of the
/// Oxy records it is rendered from, and nothing a person wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformNotice {
    /// The org's subscription payment failed. The grace period's end names
    /// the episode: it is set once when the org enters past due, so a second
    /// episode months later is a new notice while a replayed webhook is not.
    BillingPastDue {
        org_id: Uuid,
        grace_ends_at: DateTime<Utc>,
    },
    /// A workspace finished setting up and can be opened.
    WorkspaceReady { org_id: Uuid, workspace_id: Uuid },
    /// An automation run failed for good: no retry is coming.
    AutomationRunFailed {
        org_id: Uuid,
        workspace_id: Uuid,
        run_id: String,
    },
    /// An Airway pipeline run failed for good.
    AirwayRunFailed {
        org_id: Uuid,
        workspace_id: Uuid,
        run_id: String,
    },
    /// A monitor found an anomaly event, for the people its
    /// `notify.people` names. The event, not a bucket: the Slack announcement
    /// and its ledger are per event too.
    Anomaly {
        org_id: Uuid,
        workspace_id: Uuid,
        event_id: Uuid,
    },
    /// A CLI asks this person to approve its device-code request. The user
    /// code names the request; the notice never says it.
    CliApproval { org_id: Uuid, user_code: String },
    /// A person sent themselves a test, filed under one of their orgs.
    Test { org_id: Uuid, subject: TestSubject },
    /// An analytics run whose thread owner asked to hear when it ended
    /// finished with an answer.
    AskFinished {
        org_id: Uuid,
        workspace_id: Uuid,
        run_id: String,
    },
    /// The same, when it failed or timed out instead.
    AskFailed {
        org_id: Uuid,
        workspace_id: Uuid,
        run_id: String,
    },
    /// An analytics run whose thread owner asked to hear about it stopped to
    /// ask them something. One per suspension.
    AskWaiting {
        org_id: Uuid,
        workspace_id: Uuid,
        suspension: SuspensionSubject,
    },
}

impl PlatformNotice {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BillingPastDue { .. } => BILLING_PAST_DUE,
            Self::WorkspaceReady { .. } => WORKSPACE_READY,
            Self::AutomationRunFailed { .. } | Self::AirwayRunFailed { .. } => RUN_FAILED,
            Self::Anomaly { .. } => ANOMALY,
            Self::CliApproval { .. } => CLI_APPROVAL,
            Self::Test { .. } => TEST,
            Self::AskFinished { .. } => ASK_FINISHED,
            Self::AskFailed { .. } => ASK_FAILED,
            Self::AskWaiting { .. } => ASK_WAITING,
        }
    }

    pub fn org_id(&self) -> Uuid {
        match self {
            Self::BillingPastDue { org_id, .. }
            | Self::WorkspaceReady { org_id, .. }
            | Self::AutomationRunFailed { org_id, .. }
            | Self::AirwayRunFailed { org_id, .. }
            | Self::Anomaly { org_id, .. }
            | Self::CliApproval { org_id, .. }
            | Self::Test { org_id, .. }
            | Self::AskFinished { org_id, .. }
            | Self::AskFailed { org_id, .. }
            | Self::AskWaiting { org_id, .. } => *org_id,
        }
    }

    /// The workspace the delivery run is recorded under. `None` for an
    /// org-level notice, which records it under the org's oldest workspace.
    pub fn workspace_id(&self) -> Option<Uuid> {
        match self {
            Self::BillingPastDue { .. } | Self::CliApproval { .. } | Self::Test { .. } => None,
            Self::WorkspaceReady { workspace_id, .. }
            | Self::AutomationRunFailed { workspace_id, .. }
            | Self::AirwayRunFailed { workspace_id, .. }
            | Self::Anomaly { workspace_id, .. }
            | Self::AskFinished { workspace_id, .. }
            | Self::AskFailed { workspace_id, .. }
            | Self::AskWaiting { workspace_id, .. } => Some(*workspace_id),
        }
    }

    /// What it is about. With the kind, the dedupe key per recipient.
    pub fn subject(&self) -> Subject {
        match self {
            // The episode, not the org: the org has many past-due spells.
            Self::BillingPastDue { grace_ends_at, .. } => Subject {
                kind: SUBJECT_BILLING,
                id: grace_ends_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            },
            Self::WorkspaceReady { workspace_id, .. } => Subject {
                kind: SUBJECT_WORKSPACE,
                id: workspace_id.to_string(),
            },
            Self::AutomationRunFailed { run_id, .. } => Subject {
                kind: SUBJECT_AUTOMATION_RUN,
                id: run_id.clone(),
            },
            Self::AirwayRunFailed { run_id, .. } => Subject {
                kind: SUBJECT_AIRWAY_RUN,
                id: run_id.clone(),
            },
            Self::Anomaly { event_id, .. } => Subject {
                kind: SUBJECT_ANOMALY,
                id: event_id.to_string(),
            },
            Self::CliApproval { user_code, .. } => Subject {
                kind: SUBJECT_CLI_DEVICE_REQUEST,
                id: user_code.clone(),
            },
            Self::Test { subject, .. } => Subject {
                kind: SUBJECT_TEST,
                id: subject.id(),
            },
            Self::AskFinished { run_id, .. } | Self::AskFailed { run_id, .. } => Subject {
                kind: SUBJECT_ANALYTICS_RUN,
                id: run_id.clone(),
            },
            Self::AskWaiting { suspension, .. } => Subject {
                kind: SUBJECT_ANALYTICS_SUSPENSION,
                id: suspension.id(),
            },
        }
    }

    pub fn critical(&self) -> bool {
        is_critical(self.kind())
    }

    /// Whether it is also emailed; see [`channels`].
    pub fn emailed(&self) -> bool {
        channels(self.kind()).contains(&"email")
    }

    /// Whether it is also DMed in the org's Slack; see [`channels`].
    pub fn slacked(&self) -> bool {
        channels(self.kind()).contains(&"slack")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_in_the_reserved_namespace_and_only_billing_is_critical() {
        for kind in KINDS {
            assert!(kind.starts_with(RESERVED_KIND_PREFIX), "{kind}");
            assert!(is_platform_kind(kind));
            assert_eq!(is_critical(kind), kind == BILLING_PAST_DUE, "{kind}");
        }
        assert!(!is_platform_kind("oxy.something_new"));
        assert!(!is_platform_kind("billing_past_due"));
    }

    #[test]
    fn only_a_failed_run_and_a_cli_approval_are_emailed_and_only_news_and_the_test_skip_slack() {
        for kind in KINDS {
            let ch = channels(kind);
            assert_eq!(ch[0], "push", "{kind}");
            let emailed = kind == RUN_FAILED || kind == CLI_APPROVAL;
            assert_eq!(ch.contains(&"email"), emailed, "{kind}");
            // A ready workspace is news in the product; a test notice tests push
            // and the inbox, nothing else; a finished answer was asked for on
            // the phone.
            let push_only = kind == WORKSPACE_READY
                || kind == TEST
                || kind == ASK_FINISHED
                || kind == ASK_FAILED
                || kind == ASK_WAITING;
            assert_eq!(ch.contains(&"slack"), !push_only, "{kind}");
        }
        let run = PlatformNotice::AirwayRunFailed {
            org_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            run_id: "r-1".into(),
        };
        assert!(run.emailed());
        assert!(run.slacked());
    }

    #[test]
    fn a_billing_notice_is_keyed_by_its_episode_not_its_org() {
        let org = Uuid::new_v4();
        let at = |s| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let a = PlatformNotice::BillingPastDue {
            org_id: org,
            grace_ends_at: at("2026-10-21T09:00:00Z"),
        };
        let b = PlatformNotice::BillingPastDue {
            org_id: org,
            grace_ends_at: at("2027-02-03T09:00:00Z"),
        };
        assert_ne!(a.subject(), b.subject());
        assert_eq!(a.subject().id, "2026-10-21T09:00:00Z");
        assert!(a.critical());
        assert!(
            !a.emailed(),
            "billing keeps its own email; the notice must not repeat it"
        );
        assert_eq!(a.workspace_id(), None);
    }

    #[test]
    fn test_notice_is_push_only_and_no_mute_applies_though_it_is_not_critical() {
        assert_eq!(channels(TEST), &["push"]);
        assert!(!is_critical(TEST));
        assert!(!is_mutable(TEST) && !is_mutable(BILLING_PAST_DUE));
        assert!(is_mutable(WORKSPACE_READY) && is_mutable(RUN_FAILED) && is_mutable(ANOMALY));
    }

    #[test]
    fn a_cli_approval_is_keyed_by_its_user_code_and_reaches_every_channel_but_can_be_muted() {
        let org = Uuid::new_v4();
        let notice = PlatformNotice::CliApproval {
            org_id: org,
            user_code: "BCDFGHJK".into(),
        };
        assert_eq!(notice.kind(), CLI_APPROVAL);
        assert_eq!(notice.org_id(), org);
        let subject = notice.subject();
        assert_eq!(subject.kind, SUBJECT_CLI_DEVICE_REQUEST);
        assert_eq!(subject_kind(subject.kind), Some(SUBJECT_CLI_DEVICE_REQUEST));
        assert_eq!(subject.id, "BCDFGHJK", "the code without its dash");
        assert_eq!(channels(CLI_APPROVAL), &["push", "email", "slack"]);
        assert!(is_platform_kind(CLI_APPROVAL));
        assert!(is_mutable(CLI_APPROVAL) && !is_critical(CLI_APPROVAL));
        assert!(notice.emailed() && notice.slacked() && !notice.critical());
        assert_eq!(notice.workspace_id(), None);
        let other = PlatformNotice::CliApproval {
            org_id: org,
            user_code: "BCDFGHJL".into(),
        };
        assert_ne!(notice.subject(), other.subject());
    }

    #[test]
    fn a_finished_answer_is_push_only_and_keyed_by_its_run() {
        let (org, ws) = (Uuid::new_v4(), Uuid::new_v4());
        let notice = PlatformNotice::AskFinished {
            org_id: org,
            workspace_id: ws,
            run_id: "r-7".into(),
        };
        assert_eq!(notice.kind(), "oxy.ask_finished");
        assert!(is_platform_kind(ASK_FINISHED));
        assert_eq!(channels(ASK_FINISHED), &["push"]);
        assert!(!notice.emailed() && !notice.slacked() && !notice.critical());
        assert!(is_mutable(ASK_FINISHED));
        assert_eq!((notice.org_id(), notice.workspace_id()), (org, Some(ws)));
        let subject = notice.subject();
        assert_eq!(
            (subject.kind, subject.id.as_str()),
            (SUBJECT_ANALYTICS_RUN, "r-7")
        );
        assert_eq!(subject_kind("analytics_run"), Some(SUBJECT_ANALYTICS_RUN));
    }

    #[test]
    fn a_failed_answer_is_its_own_push_only_kind_about_the_same_run() {
        let failed = PlatformNotice::AskFailed {
            org_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            run_id: "r-7".into(),
        };
        assert_eq!(failed.kind(), "oxy.ask_failed");
        assert!(is_platform_kind(ASK_FAILED) && is_mutable(ASK_FAILED));
        assert_eq!(channels(ASK_FAILED), &["push"]);
        assert!(!failed.emailed() && !failed.slacked() && !failed.critical());
        let subject = failed.subject();
        assert_eq!(
            (subject.kind, subject.id.as_str()),
            (SUBJECT_ANALYTICS_RUN, "r-7")
        );
    }

    #[test]
    fn a_waiting_run_is_push_only_and_keyed_by_each_suspension() {
        let at = |s| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let notice = |when| PlatformNotice::AskWaiting {
            org_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            suspension: SuspensionSubject {
                run: "r-7".into(),
                at: at(when),
            },
        };
        let first = notice("2026-10-23T09:00:00.123456Z");
        assert_eq!(first.kind(), "oxy.ask_waiting");
        assert!(is_platform_kind(ASK_WAITING) && is_mutable(ASK_WAITING));
        assert_eq!(channels(ASK_WAITING), &["push"]);
        assert!(!first.emailed() && !first.slacked() && !first.critical());
        let subject = first.subject();
        assert_eq!(subject.kind, SUBJECT_ANALYTICS_SUSPENSION);
        assert_eq!(
            subject_kind("analytics_suspension"),
            Some(SUBJECT_ANALYTICS_SUSPENSION)
        );
        assert_eq!(
            SuspensionSubject::parse(&subject.id).map(|s| s.run),
            Some("r-7".to_string())
        );
        assert_eq!(
            subject,
            notice("2026-10-23T09:00:00.123456Z").subject(),
            "the same suspension seen twice is one subject"
        );
        assert_ne!(
            subject,
            notice("2026-10-23T09:05:00Z").subject(),
            "a second suspension of the run is another"
        );
        assert_eq!(SuspensionSubject::parse("r-7"), None);
    }

    #[test]
    fn test_notice_subject_round_trips_and_is_unique_per_moment() {
        let at = DateTime::parse_from_rfc3339("2026-10-16T14:03:21.123Z")
            .unwrap()
            .with_timezone(&Utc);
        let via = Uuid::new_v4();
        let subject = TestSubject { at, via: Some(via) };
        let notice = PlatformNotice::Test {
            org_id: Uuid::new_v4(),
            subject: subject.clone(),
        };
        let stored = notice.subject();
        assert_eq!(stored.kind, SUBJECT_TEST);
        assert_eq!(subject_kind(stored.kind), Some(SUBJECT_TEST));
        assert_eq!(TestSubject::parse(&stored.id), Some(subject));
        assert_eq!(
            stored.id,
            format!(r#"{{"at":"2026-10-16T14:03:21.123Z","via":"{via}"}}"#),
            "a moment and an id, no text"
        );
        let bare = TestSubject { at, via: None };
        assert_eq!(bare.id(), r#"{"at":"2026-10-16T14:03:21.123Z"}"#);
        assert_eq!(TestSubject::parse("2026-10-16"), None);
        assert_eq!(notice.workspace_id(), None);
        assert!(!notice.emailed() && !notice.critical());
    }
}
