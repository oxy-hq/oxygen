use std::collections::BTreeMap;

use chrono::TimeZone;
use oxy_app_core::notifications::catalog::model::Level;

use super::*;

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0)
        .single()
        .expect("valid timestamp")
}

#[test]
fn a_request_names_one_kind_and_nothing_else() {
    assert_eq!(
        parse(br#"{"kind":"check_failed"}"#),
        Ok("check_failed".to_string())
    );
    assert_eq!(
        parse(br#"{"kind":"  list.overdue "}"#),
        Ok("list.overdue".to_string())
    );
    for bad in [
        &b""[..],
        b"{}",
        br#"{"kind":""}"#,
        br#"{"kind":"   "}"#,
        br#"{"kind":7}"#,
        br#"{"kind":"a","to":"someone-else"}"#,
        b"not json",
    ] {
        assert_eq!(
            parse(bad),
            Err("invalid_body"),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn clicks_in_one_window_share_a_key_and_the_next_window_does_not() {
    let user = Uuid::nil();
    assert_eq!(window(at(1_000_000_000)), window(at(1_000_000_009)));
    assert_ne!(window(at(1_000_000_009)), window(at(1_000_000_010)));
    assert_eq!(
        window_key(user, window(at(1_000_000_003))),
        window_key(user, window(at(1_000_000_007)))
    );
    assert_ne!(window_key(user, 7), window_key(Uuid::from_u128(1), 7));
}

#[test]
fn the_notice_says_it_is_a_test_and_when_it_was_sent() {
    assert_eq!(title("Check failed"), "Test: Check failed");
    let sent = Utc
        .with_ymd_and_hms(2026, 10, 20, 9, 41, 48)
        .single()
        .expect("valid date");
    assert_eq!(
        body(sent),
        "Sent from the admin console at 09:41:48 UTC on October 20."
    );
}

fn summary(kind: &str, label: &str, level: Level, mutable: bool) -> KindSummary {
    KindSummary {
        kind: kind.to_string(),
        label: label.to_string(),
        description: None,
        level,
        category_labels: vec!["General".to_string()],
        mutable,
    }
}

/// What the catalogue says of each kind (levels, mutes, a kind only a function
/// declares) is the notifications surface's, tested beside its `LiveKinds`
/// implementation; here, that the console shows each as it is described.
#[test]
fn the_console_lists_every_kind_with_its_level_and_whether_a_mute_applies() {
    let kinds = kinds_of(&[
        summary("check_failed", "Check failed", Level::TimeSensitive, true),
        summary("fire_alarm", "Fire alarm", Level::Critical, false),
    ]);
    let by_kind: BTreeMap<_, _> = kinds.iter().map(|k| (k.kind.as_str(), k)).collect();
    assert_eq!(by_kind.len(), 2, "{kinds:?}");
    assert_eq!(by_kind["check_failed"].label, "Check failed");
    assert_eq!(by_kind["check_failed"].level, "time_sensitive");
    assert!(by_kind["check_failed"].mutable);
    assert_eq!(by_kind["check_failed"].category, ["General"]);
    assert_eq!(by_kind["fire_alarm"].level, "critical");
    assert!(!by_kind["fire_alarm"].mutable);
}

#[test]
fn an_app_with_no_live_build_lists_no_kinds() {
    assert!(kinds_of(&[]).is_empty());
}
