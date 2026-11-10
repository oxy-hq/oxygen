use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use super::*;
use crate::settings::quiet::{QuietHours, days_mask, parse_time};

fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

/// A Monday noon, UTC.
fn noon() -> DateTime<Utc> {
    at("2026-10-19T12:00:00Z")
}

fn kind(level: Level) -> KindFacts {
    KindFacts {
        level,
        channels: vec!["push", "email", "slack"],
        exempt: level == Level::Critical,
    }
}

fn off(channel: Channel) -> NodeSettings {
    let mut s = NodeSettings::default();
    s.set_channel(channel, Some(false));
    s
}

fn on(channel: Channel) -> NodeSettings {
    let mut s = NodeSettings::default();
    s.set_channel(channel, Some(true));
    s
}

fn floor(level: Level) -> NodeSettings {
    NodeSettings {
        min_level: Some(level),
        ..NodeSettings::default()
    }
}

/// kind ← category ← parent category ← source.
#[derive(Default)]
struct Tree {
    kind: NodeSettings,
    category: NodeSettings,
    parent: NodeSettings,
    source: NodeSettings,
}

impl Tree {
    fn settings<'a>(&'a self, global: &'a GlobalSettings) -> Settings<'a> {
        Settings {
            global,
            chain: vec![
                Link {
                    label: "Temperature alarm",
                    settings: &self.kind,
                },
                Link {
                    label: "Food safety",
                    settings: &self.category,
                },
                Link {
                    label: "Store operations",
                    settings: &self.parent,
                },
                Link {
                    label: "Store ops",
                    settings: &self.source,
                },
            ],
        }
    }
}

fn decide_at(
    tree: &Tree,
    global: &GlobalSettings,
    level: Level,
    channel: Channel,
    now: DateTime<Utc>,
) -> Decision {
    decide(&tree.settings(global), &kind(level), channel, None, now)
}

fn reason(d: &Decision) -> Option<&'static str> {
    d.reason.map(Reason::as_str)
}

fn quiet(allow: Level) -> GlobalSettings {
    GlobalSettings {
        paused_until: None,
        // 22:00–07:00, every night.
        quiet_hours: Some(QuietHours {
            start: parse_time("22:00").unwrap(),
            end: parse_time("07:00").unwrap(),
            days: days_mask(&[0, 1, 2, 3, 4, 5, 6]).unwrap(),
            allow,
        }),
        timezone: Some("Europe/Stockholm".into()),
    }
}

#[test]
fn nothing_set_delivers_on_every_channel_decided_by_no_node() {
    let global = GlobalSettings::default();
    for channel in Channel::ALL {
        let d = decide_at(&Tree::default(), &global, Level::Active, channel, noon());
        assert_eq!(
            d,
            Decision {
                deliver: true,
                reason: None,
                from: None
            },
            "{channel:?}"
        );
    }
}

#[test]
fn a_channel_the_kind_never_uses_is_not_sent_whatever_is_set() {
    let k = KindFacts {
        level: Level::Critical,
        channels: vec!["push"],
        exempt: true,
    };
    let tree = Tree {
        kind: on(Channel::Email),
        ..Tree::default()
    };
    let global = GlobalSettings::default();
    let d = decide(&tree.settings(&global), &k, Channel::Email, None, noon());
    assert_eq!(reason(&d), Some("not_sent"));
    assert!(!d.deliver);
}

#[test]
fn the_nearest_explicit_setting_wins_so_a_child_turns_back_on_under_a_parent_that_is_off() {
    let global = GlobalSettings::default();
    let tree = Tree {
        source: off(Channel::Push),
        category: on(Channel::Push),
        ..Tree::default()
    };
    let d = decide_at(&tree, &global, Level::Active, Channel::Push, noon());
    assert!(d.deliver);
    assert_eq!(
        d.from.as_deref(),
        Some("Food safety"),
        "the node that turned it on"
    );

    // Off at the source alone: muted, from the source.
    let tree = Tree {
        source: off(Channel::Push),
        ..Tree::default()
    };
    let d = decide_at(&tree, &global, Level::Active, Channel::Push, noon());
    assert_eq!(
        (reason(&d), d.from.as_deref()),
        (Some("muted"), Some("Store ops"))
    );

    // A nearer off beats a farther on.
    let tree = Tree {
        kind: off(Channel::Slack),
        parent: on(Channel::Slack),
        ..Tree::default()
    };
    let d = decide_at(&tree, &global, Level::Active, Channel::Slack, noon());
    assert_eq!(
        (reason(&d), d.from.as_deref()),
        (Some("muted"), Some("Temperature alarm"))
    );
    // Only that channel.
    assert!(decide_at(&tree, &global, Level::Active, Channel::Push, noon()).deliver);
}

#[test]
fn a_floor_at_a_category_holds_back_kinds_below_it_on_every_channel() {
    let global = GlobalSettings::default();
    let tree = Tree {
        parent: floor(Level::TimeSensitive),
        ..Tree::default()
    };
    for channel in Channel::ALL {
        let d = decide_at(&tree, &global, Level::Active, channel, noon());
        assert_eq!(
            (reason(&d), d.from.as_deref()),
            (Some("below_level"), Some("Store operations")),
            "{channel:?}"
        );
        assert!(decide_at(&tree, &global, Level::TimeSensitive, channel, noon()).deliver);
    }
    // A nearer floor wins over a farther one, either way.
    let tree = Tree {
        kind: floor(Level::Passive),
        source: floor(Level::Critical),
        ..Tree::default()
    };
    assert!(decide_at(&tree, &global, Level::Active, Channel::Push, noon()).deliver);
}

#[test]
fn a_pause_holds_everything_but_critical_until_it_expires() {
    let until = noon() + Duration::hours(1);
    let global = GlobalSettings {
        paused_until: Some(until),
        ..GlobalSettings::default()
    };
    let tree = Tree::default();
    for channel in Channel::ALL {
        let d = decide_at(&tree, &global, Level::TimeSensitive, channel, noon());
        assert_eq!(reason(&d), Some("paused"), "{channel:?}");
        assert!(decide_at(&tree, &global, Level::Critical, channel, noon()).deliver);
    }
    assert_eq!(
        reason(&decide_at(
            &tree,
            &global,
            Level::Active,
            Channel::Push,
            until - Duration::seconds(1)
        )),
        Some("paused")
    );
    assert!(
        decide_at(&tree, &global, Level::Active, Channel::Push, until).deliver,
        "a pause is over at its end"
    );
}

#[test]
fn quiet_hours_hold_push_and_slack_below_allow_but_never_email() {
    // 23:30 Stockholm (CEST, UTC+2) on a Monday.
    let late = at("2026-10-19T21:30:00Z");
    let global = quiet(Level::TimeSensitive);
    let tree = Tree::default();
    for channel in [Channel::Push, Channel::Slack] {
        let d = decide_at(&tree, &global, Level::Active, channel, late);
        assert_eq!(reason(&d), Some("quiet_hours"), "{channel:?}");
        assert!(
            decide_at(&tree, &global, Level::TimeSensitive, channel, late).deliver,
            "allow: time_sensitive lets it through"
        );
    }
    assert!(decide_at(&tree, &global, Level::Active, Channel::Email, late).deliver);
    // Past 07:00 the next morning, over.
    let morning = at("2026-10-20T05:00:00Z");
    assert!(decide_at(&tree, &global, Level::Active, Channel::Push, morning).deliver);
    // `allow: critical` holds time-sensitive too.
    let strict = quiet(Level::Critical);
    assert_eq!(
        reason(&decide_at(
            &tree,
            &strict,
            Level::TimeSensitive,
            Channel::Push,
            late
        )),
        Some("quiet_hours")
    );
    // Without a timezone nothing is quiet: storage refuses that, and the
    // decision does not guess one.
    let no_tz = GlobalSettings {
        timezone: None,
        ..quiet(Level::Critical)
    };
    assert!(decide_at(&tree, &no_tz, Level::Active, Channel::Push, late).deliver);
}

#[test]
fn lasting_choices_are_reported_before_passing_states() {
    let global = GlobalSettings {
        paused_until: Some(noon() + Duration::days(1)),
        ..GlobalSettings::default()
    };
    let tree = Tree {
        category: off(Channel::Push),
        ..Tree::default()
    };
    assert_eq!(
        reason(&decide_at(
            &tree,
            &global,
            Level::Active,
            Channel::Push,
            noon()
        )),
        Some("muted")
    );
    let tree = Tree {
        source: floor(Level::Critical),
        ..Tree::default()
    };
    assert_eq!(
        reason(&decide_at(
            &tree,
            &global,
            Level::Active,
            Channel::Push,
            noon()
        )),
        Some("below_level")
    );
}

#[test]
fn critical_overrides_pause_quiet_hours_mutes_and_floors() {
    let mut global = quiet(Level::Critical);
    global.paused_until = Some(noon() + Duration::days(2));
    let late = at("2026-10-19T21:30:00Z");
    let tree = Tree {
        kind: off(Channel::Push),
        source: floor(Level::Critical),
        ..Tree::default()
    };
    let d = decide_at(&tree, &global, Level::Critical, Channel::Push, late);
    assert_eq!(
        d,
        Decision {
            deliver: true,
            reason: None,
            from: None
        }
    );
}

#[test]
fn a_test_notice_is_exempt_like_critical_without_being_critical() {
    let k = KindFacts {
        level: Level::Active,
        channels: vec!["push"],
        exempt: true,
    };
    let global = GlobalSettings {
        paused_until: Some(noon() + Duration::hours(1)),
        ..GlobalSettings::default()
    };
    let tree = Tree {
        source: off(Channel::Push),
        ..Tree::default()
    };
    assert!(decide(&tree.settings(&global), &k, Channel::Push, None, noon()).deliver);
}

fn device(f: impl FnOnce(&mut DeviceSettings)) -> DeviceSettings {
    let mut d = DeviceSettings::default();
    f(&mut d);
    d
}

const CHANNEL_ID: &str = "0b7f2a6c-0000-4000-8000-000000000001.temp_alarm";

fn on_device(level: Level, d: &DeviceSettings) -> Decision {
    let global = GlobalSettings::default();
    let tree = Tree::default();
    let target = Device {
        settings: d,
        channel_id: CHANNEL_ID,
    };
    decide(
        &tree.settings(&global),
        &kind(level),
        Channel::Push,
        Some(&target),
        noon(),
    )
}

#[test]
fn a_device_can_be_off_blocked_by_its_os_or_floored_and_critical_cannot_override_the_first_two() {
    let cases = [
        (device(|d| d.enabled = false), "device_off"),
        (
            device(|d| d.os_permission = Some(OsPermission::Denied)),
            "os_denied",
        ),
        (
            device(|d| d.os_disabled = vec![CHANNEL_ID.into()]),
            "os_disabled",
        ),
        (
            device(|d| d.os_disabled = vec!["0b7f2a6c-0000-4000-8000-000000000001".into()]),
            "os_disabled",
        ),
    ];
    for (d, expected) in &cases {
        assert_eq!(reason(&on_device(Level::Active, d)), Some(*expected));
        assert_eq!(
            reason(&on_device(Level::Critical, d)),
            Some(*expected),
            "critical does not override {expected}"
        );
    }
    // Another kind's channel is untouched by an os_disabled entry.
    let other = device(|d| d.os_disabled = vec!["oxygen.oxy.run_failed".into()]);
    assert!(on_device(Level::Active, &other).deliver);

    // A watch that only gets critical.
    let watch = device(|d| d.min_level = Some(Level::Critical));
    assert_eq!(
        reason(&on_device(Level::TimeSensitive, &watch)),
        Some("below_level")
    );
    assert!(on_device(Level::Critical, &watch).deliver);

    for permission in [
        OsPermission::Granted,
        OsPermission::Provisional,
        OsPermission::Ephemeral,
        OsPermission::Unknown,
    ] {
        let d = device(|d| d.os_permission = Some(permission));
        assert!(on_device(Level::Active, &d).deliver, "{permission:?}");
    }
}

#[test]
fn the_persons_settings_are_decided_before_the_device() {
    let global = GlobalSettings {
        paused_until: Some(noon() + Duration::hours(1)),
        ..GlobalSettings::default()
    };
    let off_device = device(|d| d.enabled = false);
    let target = Device {
        settings: &off_device,
        channel_id: CHANNEL_ID,
    };
    let d = decide(
        &Tree::default().settings(&global),
        &kind(Level::Active),
        Channel::Push,
        Some(&target),
        noon(),
    );
    assert_eq!(reason(&d), Some("paused"));
}

#[test]
fn every_reason_has_its_stored_name() {
    for (r, name) in [
        (Reason::Paused, "paused"),
        (Reason::QuietHours, "quiet_hours"),
        (Reason::Muted, "muted"),
        (Reason::BelowLevel, "below_level"),
        (Reason::DeviceOff, "device_off"),
        (Reason::OsDisabled, "os_disabled"),
        (Reason::OsDenied, "os_denied"),
        (Reason::NotSent, "not_sent"),
    ] {
        assert_eq!(r.as_str(), name);
        assert_eq!(serde_json::to_value(r).unwrap(), name);
    }
}

#[test]
fn the_chain_runs_from_the_kind_up_through_each_category_to_the_source() {
    use crate::catalog::live::from_manifest;
    let catalogue = from_manifest(Some(&serde_json::json!({
        "functions": { "f": { "notify": { "kinds": { "temp_alarm": {} } } } },
        "notifications": {
            "categories": { "ops": { "label": "Store operations" },
                            "ops/food": { "label": "Food safety" } },
            "kinds": { "temp_alarm": { "category": "ops/food", "label": "Temperature alarm" } }
        }
    })));
    let info = catalogue.describe("temp_alarm");
    let nodes: HashMap<Node, NodeSettings> = [
        (Node::Scope("ops".into()), off(Channel::Push)),
        (Node::source(), floor(Level::Critical)),
    ]
    .into();
    let global = GlobalSettings::default();
    let s = Settings::for_kind(&global, Some(&nodes), &info, "Store ops");
    let labels: Vec<&str> = s.chain.iter().map(|l| l.label).collect();
    assert_eq!(
        labels,
        [
            "Temperature alarm",
            "Food safety",
            "Store operations",
            "Store ops"
        ]
    );
    assert_eq!(s.chain[2].settings.push, Some(false));
    assert_eq!(s.chain[3].settings.min_level, Some(Level::Critical));
    assert!(s.chain[0].settings.is_empty() && s.chain[1].settings.is_empty());
    let nothing = Settings::for_kind(&global, None, &info, "Store ops");
    assert!(nothing.chain.iter().all(|l| l.settings.is_empty()));
}
