//! The one decision: does a notification of this kind go out on this channel
//! (and, for push, to this device) now, and if not, why.
//!
//! Delivery records its answer in `notification_deliveries.reason`, the
//! settings document shows it as each kind's `effective`, and the tests pin
//! it here. Nothing else decides.
//!
//! # The order
//!
//! 1. **`not_sent`** — the kind is never delivered on this channel (an Oxygen
//!    kind's fixed channels).
//! 2. Unless the kind is **exempt** (critical, or a test notice), the
//!    person's settings, nearest explicit node first (kind, its category,
//!    each category above, the source):
//!    1. **`muted`** — the nearest node that sets this channel sets it off.
//!       A nearer `on` wins over a farther `off`.
//!    2. **`below_level`** — the nearest node that sets a floor sets one
//!       above the kind's level.
//!    3. **`paused`** — until `paused_until`, push, email and Slack.
//!    4. **`quiet_hours`** — inside a window, push and Slack for kinds below
//!       its `allow`. Email does not interrupt, so it is not held.
//!
//!    Lasting choices come before passing states, so a kind that is off
//!    says so even while everything is paused.
//! 3. For push to one device, whatever the kind: **`device_off`**,
//!    **`os_denied`**, **`os_disabled`** (its `<group>.<kind>` id or its
//!    whole group), and **`below_level`** for the device's own floor.
//!    Critical cannot reach a device that is switched off or blocked by its
//!    OS; the person's other devices and channels still get it.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::model::{DeviceSettings, GlobalSettings, Node, NodeSettings, OsPermission};
use super::quiet::parse_tz;
use crate::catalog::model::{Level, ancestors};
use crate::catalog::source::KindInfo;
use crate::delivery::Channel;

/// Why a notification did not go out on a channel. Stored as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Paused,
    QuietHours,
    Muted,
    BelowLevel,
    DeviceOff,
    OsDisabled,
    OsDenied,
    NotSent,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Paused => "paused",
            Reason::QuietHours => "quiet_hours",
            Reason::Muted => "muted",
            Reason::BelowLevel => "below_level",
            Reason::DeviceOff => "device_off",
            Reason::OsDisabled => "os_disabled",
            Reason::OsDenied => "os_denied",
            Reason::NotSent => "not_sent",
        }
    }
}

/// What the decision needs to know about a kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindFacts {
    pub level: Level,
    /// The channels it can ever be delivered on.
    pub channels: Vec<&'static str>,
    /// Ignores the person's settings: critical, or a test notice (which
    /// exists to show whether push arrives).
    pub exempt: bool,
}

impl KindFacts {
    /// `info` as the catalogue describes it today; `critical_send` when the
    /// sender marked this notification critical.
    pub fn of(info: &KindInfo, critical_send: bool) -> Self {
        let level = match critical_send {
            true => Level::Critical,
            false => info.level,
        };
        Self {
            level,
            channels: info.channels.clone(),
            exempt: critical_send || !info.mutable,
        }
    }
}

/// One node on the way from a kind up to its source, with what is set there.
#[derive(Debug, Clone, Copy)]
pub struct Link<'a> {
    /// The node's label, for `from`.
    pub label: &'a str,
    pub settings: &'a NodeSettings,
}

/// A person's settings as they bear on one kind.
#[derive(Debug, Clone)]
pub struct Settings<'a> {
    pub global: &'a GlobalSettings,
    /// Nearest first: the kind, its category, each one above, the source.
    pub chain: Vec<Link<'a>>,
}

/// What an unset node holds.
static UNSET: NodeSettings = NodeSettings {
    push: None,
    email: None,
    slack: None,
    min_level: None,
};

impl<'a> Settings<'a> {
    /// The chain for `info` from what a person set in its source (`nodes`),
    /// labelled for `from`: the kind, its category and each one above, then
    /// the source, `source_label`.
    pub fn for_kind(
        global: &'a GlobalSettings,
        nodes: Option<&'a HashMap<Node, NodeSettings>>,
        info: &'a KindInfo,
        source_label: &'a str,
    ) -> Self {
        let at = |node: Node| nodes.and_then(|n| n.get(&node)).unwrap_or(&UNSET);
        let mut chain = vec![Link {
            label: &info.label,
            settings: at(Node::Kind(info.kind.clone())),
        }];
        let paths = ancestors(&info.category);
        for (path, label) in paths.iter().zip(&info.category_labels).rev() {
            chain.push(Link {
                label,
                settings: at(Node::Scope(path.clone())),
            });
        }
        chain.push(Link {
            label: source_label,
            settings: at(Node::source()),
        });
        Self { global, chain }
    }
}

/// One push target.
#[derive(Debug, Clone, Copy)]
pub struct Device<'a> {
    pub settings: &'a DeviceSettings,
    /// `<group>.<kind>`, as `PushSignal::channel_id` builds it.
    pub channel_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub deliver: bool,
    pub reason: Option<Reason>,
    /// The label of the node that decided it: the one that turned the
    /// channel off, set the floor, or explicitly turned it on. `None` when
    /// no node did.
    pub from: Option<String>,
}

impl Decision {
    fn off(reason: Reason, from: Option<&str>) -> Self {
        Self {
            deliver: false,
            reason: Some(reason),
            from: from.map(str::to_string),
        }
    }
}

/// Whether `kind` goes out on `channel` (to `device`, for push) at `now`.
pub fn decide(
    settings: &Settings<'_>,
    kind: &KindFacts,
    channel: Channel,
    device: Option<&Device<'_>>,
    now: DateTime<Utc>,
) -> Decision {
    if !kind.channels.contains(&channel.as_str()) {
        return Decision::off(Reason::NotSent, None);
    }
    let mut from = None;
    if !kind.exempt {
        let set = settings
            .chain
            .iter()
            .find(|l| l.settings.channel(channel).is_some());
        if let Some(link) = set {
            if link.settings.channel(channel) == Some(false) {
                return Decision::off(Reason::Muted, Some(link.label));
            }
            from = Some(link.label.to_string());
        }
        let floor = settings
            .chain
            .iter()
            .find_map(|l| l.settings.min_level.map(|level| (l.label, level)));
        if let Some((label, floor)) = floor
            && kind.level < floor
        {
            return Decision::off(Reason::BelowLevel, Some(label));
        }
        if settings.global.paused_at(now) {
            return Decision::off(Reason::Paused, None);
        }
        if quiet_holds(settings.global, kind.level, channel, now) {
            return Decision::off(Reason::QuietHours, None);
        }
    }
    if let Some(device) = device
        && let Some(reason) = device_reason(device.settings, kind.level, device.channel_id)
    {
        return Decision::off(reason, None);
    }
    Decision {
        deliver: true,
        reason: None,
        from,
    }
}

/// Whether quiet hours hold `channel` back for a kind of `level` at `now`.
/// Email is never held: it does not interrupt.
pub fn quiet_holds(
    global: &GlobalSettings,
    level: Level,
    channel: Channel,
    now: DateTime<Utc>,
) -> bool {
    if channel == Channel::Email {
        return false;
    }
    let (Some(quiet), Some(tz)) = (
        global.quiet_hours.as_ref(),
        global.timezone.as_deref().and_then(parse_tz),
    ) else {
        return false;
    };
    !quiet.lets_through(level) && quiet.active_at(now, tz)
}

/// Why `device` does not get a push of `level` on `channel_id`, if it does
/// not. Applies to every kind, critical included.
pub fn device_reason(device: &DeviceSettings, level: Level, channel_id: &str) -> Option<Reason> {
    if !device.enabled {
        return Some(Reason::DeviceOff);
    }
    if device.os_permission == Some(OsPermission::Denied) {
        return Some(Reason::OsDenied);
    }
    // `<group>.<kind>`; a group (an app id, or `oxygen`) has no dot.
    let group = channel_id.split('.').next().unwrap_or(channel_id);
    if device
        .os_disabled
        .iter()
        .any(|id| id == channel_id || id == group)
    {
        return Some(Reason::OsDisabled);
    }
    if device.min_level.is_some_and(|floor| level < floor) {
        return Some(Reason::BelowLevel);
    }
    None
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
