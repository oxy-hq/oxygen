//! `notify:` — announcing an insight where people already are.
//!
//! The Insights Inbox only helps someone who opens it. A `.monitor.yml` that
//! carries a `notify:` block has each scan post what it newly found to a Slack
//! channel in the org's own connected Slack:
//!
//! ```yaml
//! notify:
//!   slack_channel: C0123ABCDEF
//!   min_severity: high   # low | medium | high; default high
//! ```
//!
//! Four pieces, split by what they need:
//! - this module — the config block, parsed with the rest of the file;
//! - [`ledger`] — which events are due, and the once-per-event claim on them
//!   (Postgres);
//! - [`message`] — the Slack message for a set of claimed events (pure);
//! - [`announce`] — the three in order, against a [`Destination`].
//!
//! Posting needs the org's Slack installation and a queue, neither of which
//! this crate has: the host (`oxy-app`) supplies the [`Destination`] and runs
//! [`announce()`] as a queued task after a scan.

use serde::{Deserialize, Serialize};

use crate::detect::Severity;

pub mod announce;
pub mod ledger;
pub mod message;

pub use announce::{AnnounceError, Destination, Heading, announce};
pub use ledger::Due;
pub use message::SlackMessage;

/// The file-level `notify:` block. Absent = nothing is announced.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    /// Slack channel **id** (`C0123ABCDEF`), not its name: a name is renamed
    /// out from under the file, and `chat.postMessage` resolves one only for
    /// public channels. The org's Oxygen Slack app must be in the channel.
    pub slack_channel: String,
    /// The least severe event worth a message. Defaults to `high`: severity is
    /// how far a value cleared its seasonal band, so `low` announces buckets
    /// that barely left it.
    #[serde(default = "default_min_severity")]
    pub min_severity: Severity,
}

fn default_min_severity() -> Severity {
    Severity::High
}

impl NotifyConfig {
    /// Whether `slack_channel` has the shape of a channel id.
    ///
    /// Checked when the file loads so `#revenue-alerts` is refused where its
    /// author can read why, instead of as `channel_not_found` from a background
    /// task a day later. Shape only — whether the channel exists and the app
    /// is in it is Slack's to answer at post time.
    pub fn has_channel_id(&self) -> bool {
        let id = self.slack_channel.as_str();
        id.len() >= 3
            && id.starts_with(['C', 'G'])
            && id
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
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
                slack_channel: "C0123ABCDEF".into(),
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
    fn the_block_needs_a_channel() {
        assert!(matches!(
            load(&format!("notify:\n  min_severity: low\n{ENTRY}")),
            Err(LoadError::Parse { .. })
        ));
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
            "notify:\n  slack_channel: C0123ABCDEF\n  min_severity: low\n{ENTRY}"
        ))
        .unwrap();
        let again: MonitorConfig =
            serde_yaml::from_str(&serde_yaml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(again.notify, cfg.notify);

        let bare = serde_yaml::to_string(&load(ENTRY).unwrap()).unwrap();
        assert!(!bare.contains("notify"), "{bare}");
    }
}
