//! `notify:` — announcing an insight where people already are.
//!
//! The Insights Inbox only helps someone who opens it. A `.monitor.yml` that
//! carries a `notify:` block has each scan announce what it newly found: to a
//! Slack channel in the org's own connected Slack, to the Oxygen inboxes of
//! the people it names, or both:
//!
//! ```yaml
//! notify:
//!   slack_channel: C0123ABCDEF   # optional when `people` is given
//!   people:                      # optional when `slack_channel` is given
//!     - email: dana@acme.com     # an org member, by address
//!     - role: admin              # owner | admin | member, and everyone above it
//!     - team: Finance            # an org team, by name
//!   min_severity: high           # low | medium | high; default high
//! ```
//!
//! `people` resolves to org members only, when the announcement is delivered;
//! an entry that names nobody is skipped and logged. An older binary refuses a
//! file that uses it (`deny_unknown_fields`), so ship the binary first.
//!
//! Four pieces, split by what they need:
//! - this module — the config block, parsed with the rest of the file;
//! - [`ledger`] — which events are due, and the once-per-event claim on them,
//!   per channel (Postgres);
//! - [`message`] — the Slack message for a set of claimed events (pure);
//! - [`announce`] — the three in order, against a [`Destination`];
//! - [`enqueue`] — the queued delivery task a scan asks for ([`enqueue_if_due`]).
//!
//! Posting needs the org's Slack installation and its people, which this crate
//! does not have: the surface (`oxy-api-metric-monitoring`'s `anomaly_notify`)
//! supplies each [`Destination`] and runs [`announce()`] as the queued task.

use serde::{Deserialize, Serialize};

use crate::detect::Severity;

pub mod announce;
pub mod enqueue;
pub mod ledger;
pub mod message;

pub use announce::{AnnounceError, Destination, Heading, announce};
pub use enqueue::{ANOMALY_NOTIFY_KIND, AnomalyNotifyPayload, enqueue_if_due};
pub use ledger::Due;
pub use message::{Bucket, SlackMessage};

/// Most `people` entries one block may hold. A role or a team already names
/// many people; a list longer than this is a directory, not a notify list.
pub const MAX_PEOPLE_ENTRIES: usize = 50;

/// The file-level `notify:` block. Absent = nothing is announced.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    /// Slack channel **id** (`C0123ABCDEF`), not its name: a name is renamed
    /// out from under the file, and `chat.postMessage` resolves one only for
    /// public channels. The org's Oxygen Slack app must be in the channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slack_channel: Option<String>,
    /// People told in their Oxygen inbox (and by push), resolved to org
    /// members when the announcement is delivered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<PersonTarget>,
    /// The least severe event worth a message. Defaults to `high`: severity is
    /// how far a value cleared its seasonal band, so `low` announces buckets
    /// that barely left it.
    #[serde(default = "default_min_severity")]
    pub min_severity: Severity,
}

/// One `people` entry: exactly one of `email`, `role` or `team`.
///
/// A struct of options rather than an untagged enum so a mistake is named
/// (`checked` says which key is wrong) instead of "did not match any variant".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PersonTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<OrgRoleTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
}

/// An org role, meaning everyone holding it **or a higher one**: `admin` is
/// the org's admins and owners, `member` is every member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrgRoleTarget {
    Owner,
    Admin,
    Member,
}

/// A checked `people` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Person {
    Email(String),
    Role(OrgRoleTarget),
    Team(String),
}

impl PersonTarget {
    /// The one target this entry names, or why it does not name one.
    pub fn checked(&self) -> Result<Person, String> {
        let named = [
            self.email.is_some(),
            self.role.is_some(),
            self.team.is_some(),
        ];
        if named.iter().filter(|n| **n).count() != 1 {
            return Err(
                "each `people` entry names exactly one of `email`, `role` or `team`".into(),
            );
        }
        if let Some(email) = &self.email {
            let email = email.trim();
            if !email.contains('@') || email.chars().any(char::is_whitespace) {
                return Err(format!("{email:?} is not an email address"));
            }
            return Ok(Person::Email(email.to_ascii_lowercase()));
        }
        if let Some(role) = self.role {
            return Ok(Person::Role(role));
        }
        let team = self.team.as_deref().unwrap_or_default().trim();
        if team.is_empty() {
            return Err("`team` names an org team and cannot be empty".into());
        }
        Ok(Person::Team(team.to_string()))
    }
}

fn default_min_severity() -> Severity {
    Severity::High
}

impl NotifyConfig {
    /// Whether `slack_channel`, when given, has the shape of a channel id.
    ///
    /// Checked when the file loads so `#revenue-alerts` is refused where its
    /// author can read why, instead of as `channel_not_found` from a background
    /// task a day later. Shape only — whether the channel exists and the app
    /// is in it is Slack's to answer at post time.
    pub fn has_channel_id(&self) -> bool {
        let Some(id) = self.slack_channel.as_deref() else {
            return true;
        };
        id.len() >= 3
            && id.starts_with(['C', 'G'])
            && id
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    }

    /// The checked `people` entries, or the first problem with them — or with
    /// a block that names nowhere to announce at all.
    pub fn people(&self) -> Result<Vec<Person>, String> {
        if self.slack_channel.is_none() && self.people.is_empty() {
            return Err(
                "the block names nowhere to announce: give `slack_channel`, `people`, or both"
                    .into(),
            );
        }
        if self.people.len() > MAX_PEOPLE_ENTRIES {
            return Err(format!(
                "`people` holds more than {MAX_PEOPLE_ENTRIES} entries"
            ));
        }
        self.people.iter().map(PersonTarget::checked).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LoadError, MonitorConfig, load_from_file};

    const ENTRY: &str = "monitors:\n  - measure: a.b\n    time_dimension: a.t\n";

    fn load(yaml: &str) -> Result<MonitorConfig, LoadError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".monitor.yml");
        std::fs::write(&path, yaml).unwrap();
        load_from_file(&path)
    }

    #[test]
    fn a_file_without_the_block_announces_nothing() {
        let cfg = load(ENTRY).unwrap();
        assert_eq!(cfg.notify, None);
    }

    #[test]
    fn severity_defaults_to_high() {
        let cfg = load(&format!("notify:\n  slack_channel: C0123ABCDEF\n{ENTRY}")).unwrap();
        assert_eq!(
            cfg.notify,
            Some(NotifyConfig {
                slack_channel: Some("C0123ABCDEF".into()),
                people: Vec::new(),
                min_severity: Severity::High,
            })
        );
    }

    #[test]
    fn severity_can_be_lowered() {
        let cfg = load(&format!(
            "notify:\n  slack_channel: G01ABC\n  min_severity: medium\n{ENTRY}"
        ))
        .unwrap();
        assert_eq!(cfg.notify.unwrap().min_severity, Severity::Medium);
    }

    #[test]
    fn a_channel_name_is_refused_when_the_file_loads() {
        for name in [
            "#revenue-alerts",
            "revenue-alerts",
            "c0123abcdef",
            "",
            "U0123ABCDEF",
        ] {
            let err = load(&format!("notify:\n  slack_channel: \"{name}\"\n{ENTRY}"))
                .expect_err("only a channel id may be written");
            assert!(
                matches!(err, LoadError::InvalidNotifyChannel { .. }),
                "{name:?}: {err:?}"
            );
        }
    }

    #[test]
    fn the_block_needs_somewhere_to_announce() {
        assert!(matches!(
            load(&format!("notify:\n  min_severity: low\n{ENTRY}")),
            Err(LoadError::InvalidNotify { .. })
        ));
    }

    #[test]
    fn people_alone_is_enough_and_each_entry_names_one_target() {
        let cfg = load(&format!(
            "notify:\n  people:\n    - email: Dana@Acme.com\n    - role: admin\n    - team: Finance\n{ENTRY}"
        ))
        .unwrap();
        let notify = cfg.notify.unwrap();
        assert_eq!(notify.slack_channel, None);
        assert_eq!(
            notify.people().unwrap(),
            vec![
                Person::Email("dana@acme.com".into()),
                Person::Role(OrgRoleTarget::Admin),
                Person::Team("Finance".into()),
            ]
        );
        for bad in [
            "    - email: dana@acme.com\n      team: Finance\n",
            "    - email: not-an-address\n",
            "    - team: \"  \"\n",
            "    - {}\n",
        ] {
            let err = load(&format!("notify:\n  people:\n{bad}{ENTRY}")).expect_err(bad);
            assert!(
                matches!(err, LoadError::InvalidNotify { .. }),
                "{bad}: {err:?}"
            );
        }
        assert!(
            matches!(
                load(&format!("notify:\n  people:\n    - role: manager\n{ENTRY}")),
                Err(LoadError::Parse { .. })
            ),
            "an unknown role is a parse error"
        );
    }

    #[test]
    fn the_block_rejects_fields_it_does_not_know() {
        assert!(matches!(
            load(&format!(
                "notify:\n  slack_channel: C0123ABCDEF\n  email: a@b.c\n{ENTRY}"
            )),
            Err(LoadError::Parse { .. })
        ));
    }

    /// The compile boundary stores the file as JSON and writes it back out as
    /// YAML for the scan, so the block has to survive a serialize round trip —
    /// and a file without one must not grow a `notify: null`.
    #[test]
    fn the_block_survives_the_compile_round_trip() {
        let cfg = load(&format!(
            "notify:\n  slack_channel: C0123ABCDEF\n  people:\n    - role: owner\n  min_severity: low\n{ENTRY}"
        ))
        .unwrap();
        let again: MonitorConfig =
            serde_yaml::from_str(&serde_yaml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(again.notify, cfg.notify);

        let bare = serde_yaml::to_string(&load(ENTRY).unwrap()).unwrap();
        assert!(!bare.contains("notify"), "{bare}");

        // A Slack-only block serializes exactly as it did before `people`, so
        // a pod that predates it still reads the queued payload.
        let slack_only = load(&format!("notify:\n  slack_channel: C0123ABCDEF\n{ENTRY}"))
            .unwrap()
            .notify
            .unwrap();
        assert_eq!(
            serde_json::to_value(&slack_only).unwrap(),
            serde_json::json!({ "slack_channel": "C0123ABCDEF", "min_severity": "high" })
        );
    }
}
