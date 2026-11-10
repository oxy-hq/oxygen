//! The words of an `oxy.cli_approval` notice, and the shape it is loaded in.

use chrono::TimeZone;
use serde_json::json;

use super::*;

fn at(h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 21, h, m, 0).unwrap()
}

fn request(kind: &'static str, apps: &[&str], hours: Option<i64>, status: &str) -> Record {
    Record::CliDeviceRequest {
        user_code: "BCDFGHJK".into(),
        hostname: "build-box".into(),
        kind,
        apps: apps.iter().map(|a| a.to_string()).collect(),
        hours,
        status: status.into(),
        expires_at: at(14, 5),
    }
}

fn row(ask: Value) -> cli_device_requests::Model {
    cli_device_requests::Model {
        device_code_hash: vec![1],
        user_code: "BCDFGHJK".into(),
        code_challenge: "c".into(),
        hostname: "build-box".into(),
        ask,
        status: "pending".into(),
        user_id: None,
        mint: None,
        denial: None,
        created_at: at(13, 55).fixed_offset(),
        expires_at: at(14, 5).fixed_offset(),
        decided_at: None,
        last_polled_at: None,
        consumed_at: None,
        approver_user_id: None,
    }
}

#[test]
fn each_kind_is_named_in_the_title_and_a_waiting_request_says_how_to_answer() {
    let cases = [
        (
            request("login", &[], None, "pending"),
            "oxyc on build-box asks to sign in as you",
        ),
        (
            request("agent", &[], Some(8), "pending"),
            "oxyc on build-box asks for an agent token (8 h)",
        ),
        (
            request(
                "sandbox_agent",
                &["acme/store", "acme/ops"],
                Some(2),
                "pending",
            ),
            "oxyc on build-box asks for a sandbox agent token for acme/store, acme/ops (2 h)",
        ),
    ];
    for (record, title) in cases {
        let r = words(&record, at(14, 0)).unwrap();
        assert_eq!(r.title, title);
        assert_eq!(
            r.body.as_deref(),
            Some(
                "Open the approval page and type the code from the terminal that ran oxyc. If you did not run this command, ignore it: nothing happens unless the code is typed. Expires at 14:05 UTC."
            )
        );
        assert_eq!(r.href.as_deref(), Some("/cli-auth/device"));
    }
}

#[test]
fn a_request_with_no_hours_or_apps_still_reads() {
    let r = words(&request("agent", &[], None, "pending"), at(14, 0)).unwrap();
    assert_eq!(r.title, "oxyc on build-box asks for an agent token");
    let r = words(
        &request("sandbox_agent", &[], Some(4), "pending"),
        at(14, 0),
    )
    .unwrap();
    assert_eq!(
        r.title,
        "oxyc on build-box asks for a sandbox agent token (4 h)"
    );
}

#[test]
fn a_decided_or_lapsed_request_keeps_its_title_and_says_how_it_ended() {
    let waiting = words(&request("agent", &[], Some(8), "pending"), at(14, 0)).unwrap();
    for (status, now, body) in [
        ("approved", at(14, 1), "Approved."),
        ("denied", at(14, 1), "Declined."),
        ("pending", at(14, 5), "Expired."),
        ("pending", at(15, 0), "Expired."),
    ] {
        let r = words(&request("agent", &[], Some(8), status), now).unwrap();
        assert_eq!(r.title, waiting.title, "{status}");
        assert_eq!(r.body.as_deref(), Some(body), "{status} at {now}");
    }
}

#[test]
fn the_link_is_the_page_without_the_code_and_no_word_carries_it() {
    for status in ["pending", "approved", "denied"] {
        let r = words(&request("agent", &[], Some(8), status), at(14, 0)).unwrap();
        assert_eq!(r.href.as_deref(), Some(PAGE));
        let said = format!("{} {}", r.title, r.body.unwrap_or_default());
        assert!(!said.contains("BCDF"), "{said}");
        assert!(!r.href.unwrap().contains("code"));
    }
}

#[test]
fn a_long_title_is_cut_to_two_hundred_characters() {
    let apps: Vec<String> = (0..40).map(|i| format!("acme/application-{i}")).collect();
    let r = words(
        &request(
            "sandbox_agent",
            &apps.iter().map(String::as_str).collect::<Vec<_>>(),
            Some(8),
            "pending",
        ),
        at(14, 0),
    )
    .unwrap();
    assert_eq!(r.title.chars().count(), 200);
    assert!(r.title.ends_with('…'));
    assert!(
        r.title
            .starts_with("oxyc on build-box asks for a sandbox agent token for acme/")
    );
}

#[test]
fn a_record_of_another_shape_has_no_words() {
    let other = Record::Billing {
        grace_ends_at: at(9, 0),
    };
    assert_eq!(words(&other, at(14, 0)), None);
}

#[test]
fn the_ask_is_read_as_the_cli_sent_it() {
    let (code, sandbox) = record_of(json!({
        "kind": "sandbox_agent", "apps": "acme/store, acme/ops,,", "hours": "8", "name": "x"
    }));
    assert_eq!(code, "BCDFGHJK");
    assert_eq!(
        sandbox,
        request(
            "sandbox_agent",
            &["acme/store", "acme/ops"],
            Some(8),
            "pending"
        )
    );

    let (_, login) = record_of(json!({}));
    assert_eq!(login, request("login", &[], None, "pending"));
    let (_, agent) = record_of(json!({ "kind": "agent", "hours": "many" }));
    assert_eq!(agent, request("agent", &[], None, "pending"));
    assert!(record(row(json!({ "kind": "from_the_future" }))).is_none());
}

fn record_of(ask: Value) -> (String, Record) {
    record(row(ask)).unwrap()
}

/// The hostname is the anonymous caller's text, sent down three channels: a
/// title carries only a host label of it.
#[test]
fn a_hostname_is_rendered_as_a_host_label() {
    let at = |h: u32, m: u32| chrono::Utc.with_ymd_and_hms(2026, 10, 21, h, m, 0).unwrap();
    let record = |hostname: &str| Record::CliDeviceRequest {
        user_code: "BCDFGHJK".into(),
        hostname: hostname.into(),
        kind: "login",
        apps: vec![],
        hours: None,
        status: "pending".into(),
        expires_at: at(14, 5),
    };
    let title = |hostname: &str| words(&record(hostname), at(14, 0)).unwrap().title;
    assert_eq!(
        title("build-box"),
        "oxyc on build-box asks to sign in as you"
    );
    assert_eq!(
        title("Click here <b>NOW</b>"),
        "oxyc on Click-here-b-NOW-b asks to sign in as you"
    );
    assert_eq!(
        title("\u{202e}"),
        "oxyc on a machine asks to sign in as you"
    );
    assert!(title(&"a".repeat(90)).contains(&"a".repeat(64)));
    assert!(!title(&"a".repeat(90)).contains(&"a".repeat(65)));
    // A host that could carry the code is not named at all.
    assert_eq!(
        title("your laptop - enter code BCDF-GHJK to keep your session"),
        "oxyc on a machine asks to sign in as you"
    );
}

#[test]
fn an_app_reference_is_rendered_as_a_label_or_dropped() {
    let at = |h: u32, m: u32| chrono::Utc.with_ymd_and_hms(2026, 10, 21, h, m, 0).unwrap();
    let record = |apps: &[&str]| Record::CliDeviceRequest {
        user_code: "BCDFGHJK".into(),
        hostname: "build-box".into(),
        kind: "sandbox_agent",
        apps: apps.iter().map(|a| a.to_string()).collect(),
        hours: Some(8),
        status: "pending".into(),
        expires_at: at(14, 5),
    };
    let title = |apps: &[&str]| words(&record(apps), at(14, 0)).unwrap().title;
    assert_eq!(
        title(&["oxygen/roadmap"]),
        "oxyc on build-box asks for a sandbox agent token for oxygen/roadmap (8 h)"
    );
    assert_eq!(
        title(&["oxygen/roadmap", "type BCDF-GHJK <now>"]),
        "oxyc on build-box asks for a sandbox agent token for oxygen/roadmap (8 h)"
    );
    assert_eq!(
        title(&["type BCDF-GHJK"]),
        "oxyc on build-box asks for a sandbox agent token (8 h)"
    );
    // A code over a slash is caught on the reference; one split across two
    // references is caught on the assembled title, which then names nothing
    // of the asker's.
    assert_eq!(
        title(&["BCDF/GHJK"]),
        "oxyc on build-box asks for a sandbox agent token (8 h)"
    );
    assert_eq!(
        title(&["bcdf", "ghjk"]),
        "oxyc on a machine asks for a sandbox agent token (8 h)"
    );
}
