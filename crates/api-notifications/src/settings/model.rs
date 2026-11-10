//! What a person can set, as values: a node of a source's tree and what is
//! set there, the person-level settings above every source, and a device's.
//!
//! Plain data. Reading and writing it is [`super::store`]; turning it into
//! "does this go out" is [`super::decide`].

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use super::quiet::{QuietHours, QuietHoursView};
use crate::catalog::model::Level;
use crate::delivery::Channel;

/// A node of a source's tree that can carry settings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    /// `""` is the whole source; otherwise a category path, covering its
    /// subtree.
    Scope(String),
    Kind(String),
}

impl Node {
    /// The whole source.
    pub fn source() -> Self {
        Node::Scope(String::new())
    }

    /// `node_type` as stored.
    pub fn type_str(&self) -> &'static str {
        match self {
            Node::Scope(_) => "scope",
            Node::Kind(_) => "kind",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Node::Scope(s) | Node::Kind(s) => s,
        }
    }

    /// From `(node_type, node)` as stored; `None` for a type this binary does
    /// not know.
    pub fn from_stored(node_type: &str, node: String) -> Option<Self> {
        match node_type {
            "scope" => Some(Node::Scope(node)),
            "kind" => Some(Node::Kind(node)),
            _ => None,
        }
    }
}

/// What is set at one node itself. `None` everywhere is "inherit".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeSettings {
    pub push: Option<bool>,
    pub email: Option<bool>,
    pub slack: Option<bool>,
    /// Kinds below it go to the inbox only, on every channel.
    pub min_level: Option<Level>,
}

impl NodeSettings {
    pub fn channel(&self, channel: Channel) -> Option<bool> {
        match channel {
            Channel::Push => self.push,
            Channel::Email => self.email,
            Channel::Slack => self.slack,
        }
    }

    pub fn set_channel(&mut self, channel: Channel, value: Option<bool>) {
        match channel {
            Channel::Push => self.push = value,
            Channel::Email => self.email = value,
            Channel::Slack => self.slack = value,
        }
    }

    /// Nothing set: the row can go.
    pub fn is_empty(&self) -> bool {
        Channel::ALL.iter().all(|c| self.channel(*c).is_none()) && self.min_level.is_none()
    }

    /// The channels explicitly off here, in channel order: what the
    /// catalogue and the old preferences route call `muted`.
    pub fn off(&self) -> Vec<&'static str> {
        Channel::ALL
            .iter()
            .filter(|c| self.channel(**c) == Some(false))
            .map(|c| c.as_str())
            .collect()
    }

    pub fn view(&self) -> NodeSettingsView {
        NodeSettingsView {
            channels: Channel::ALL
                .iter()
                .filter_map(|c| self.channel(*c).map(|v| (c.as_str(), v)))
                .collect(),
            min_level: self.min_level,
        }
    }
}

/// `NodeSettings` on the wire: explicit values only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeSettingsView {
    /// Absent = inherit.
    pub channels: BTreeMap<&'static str, bool>,
    pub min_level: Option<Level>,
}

/// The person's own settings, above every source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalSettings {
    pub paused_until: Option<DateTime<Utc>>,
    pub quiet_hours: Option<QuietHours>,
    /// IANA. Required while quiet hours are set.
    pub timezone: Option<String>,
}

impl GlobalSettings {
    pub fn paused_at(&self, now: DateTime<Utc>) -> bool {
        self.paused_until.is_some_and(|until| until > now)
    }

    pub fn view(&self) -> GlobalView {
        GlobalView {
            paused_until: self
                .paused_until
                .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true)),
            quiet_hours: self.quiet_hours.as_ref().map(QuietHours::view),
            timezone: self.timezone.clone(),
        }
    }
}

/// `GlobalSettings` on the wire. Every field present, `null` when unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GlobalView {
    pub paused_until: Option<String>,
    pub quiet_hours: Option<QuietHoursView>,
    pub timezone: Option<String>,
}

/// What a device last reported of its OS notification permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OsPermission {
    Granted,
    Denied,
    /// iOS: delivered quietly to the notification centre.
    Provisional,
    /// iOS App Clips: granted for a short while.
    Ephemeral,
    Unknown,
}

impl OsPermission {
    pub const ALL: [OsPermission; 5] = [
        OsPermission::Granted,
        OsPermission::Denied,
        OsPermission::Provisional,
        OsPermission::Ephemeral,
        OsPermission::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            OsPermission::Granted => "granted",
            OsPermission::Denied => "denied",
            OsPermission::Provisional => "provisional",
            OsPermission::Ephemeral => "ephemeral",
            OsPermission::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == s)
    }
}

/// One device's settings and OS state, as [`super::decide`] reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSettings {
    pub enabled: bool,
    pub min_level: Option<Level>,
    pub os_permission: Option<OsPermission>,
    /// `<group>.<kind>` ids, or a whole `<group>`, the OS has switched off.
    pub os_disabled: Vec<String>,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            min_level: None,
            os_permission: None,
            os_disabled: Vec::new(),
        }
    }
}

impl DeviceSettings {
    /// From a `device_tokens` row. A value this binary cannot read is
    /// treated as unset, never as a refusal.
    pub fn of(row: &entity::device_tokens::Model) -> Self {
        Self {
            enabled: row.enabled,
            min_level: row.min_level.as_deref().and_then(Level::parse),
            os_permission: row.os_permission.as_deref().and_then(OsPermission::parse),
            os_disabled: row
                .os_disabled
                .as_ref()
                .and_then(|v| v.as_array())
                .map(|ids| {
                    ids.iter()
                        .filter_map(|id| id.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_round_trips_and_only_explicit_values_are_shown() {
        for node in [
            Node::source(),
            Node::Scope("a/b".into()),
            Node::Kind("k".into()),
        ] {
            assert_eq!(
                Node::from_stored(node.type_str(), node.name().to_string()),
                Some(node.clone())
            );
        }
        assert_eq!(Node::from_stored("app", "x".into()), None);
        let mut s = NodeSettings::default();
        assert!(s.is_empty());
        s.set_channel(Channel::Slack, Some(false));
        s.set_channel(Channel::Push, Some(true));
        assert_eq!(s.off(), vec!["slack"]);
        let v = serde_json::to_value(s.view()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "channels": { "push": true, "slack": false }, "min_level": null })
        );
        s.set_channel(Channel::Slack, None);
        s.set_channel(Channel::Push, None);
        assert!(s.is_empty());
    }

    #[test]
    fn global_settings_serialize_every_field_null_not_missing() {
        let v = serde_json::to_value(GlobalSettings::default().view()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "paused_until": null, "quiet_hours": null, "timezone": null })
        );
    }
}
