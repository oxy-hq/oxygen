use super::super::model::fixtures::*;
use super::*;

const CONSOLE: &str = "https://app.oxygen-hq.com";

fn week() -> Snapshot {
    snapshot(vec![
        org(
            1,
            "Poke House",
            vec![app(10, "Store Ops", 6, 4), app(11, "Checklists", 0, 0)],
        ),
        org(2, "Rivermark", vec![app(20, "Crew Board", 3, 9)]),
        org(3, "Dormant Co", vec![app(30, "Archive", 0, 0)]),
    ])
}

fn labels(mail: &ReportEmail) -> Vec<&str> {
    mail.stat_rows
        .iter()
        .flatten()
        .map(|s| s.label.as_str())
        .collect()
}

#[test]
fn the_subject_says_what_happened_and_where() {
    let mail = view(&week(), Some(CONSOLE));
    assert_eq!(
        mail.subject,
        "Custom app usage, Sep 28 – Oct 4: 9 people in 2 apps, 1 needs a look \
         (app.oxygen-hq.com)"
    );
    let quiet = snapshot(vec![org(1, "Poke House", vec![app(10, "Store Ops", 0, 0)])]);
    assert_eq!(
        view(&quiet, None).subject,
        "Custom app usage, Sep 28 – Oct 4: nobody opened an app"
    );
}

#[test]
fn a_week_of_traffic_alone_shows_three_figures() {
    let mail = view(&week(), None);
    assert_eq!(labels(&mail), ["People", "Opens", "Apps in use"]);
    let people = &mail.stat_rows[0][0];
    assert_eq!((people.value.as_str(), people.note.as_str()), ("9", "−4"));
    assert_eq!(mail.stat_rows[0][2].value, "2 of 4");
    assert!(!mail.show_calls && !mail.show_releases && !mail.show_storage);
}

#[test]
fn functions_releases_and_storage_appear_when_there_is_something_to_say() {
    let mut report = week();
    let store_ops = &mut report.orgs[0].apps[0];
    store_ops.current.function_calls = 1240;
    store_ops.current.function_failures = 31;
    store_ops.current.releases = 2;
    store_ops.storage_bytes = Some(3 * 1024 * 1024 * 1024);
    store_ops.storage_bytes_before = Some(2 * 1024 * 1024 * 1024);

    let mail = view(&report, None);
    assert_eq!(
        labels(&mail),
        [
            "People",
            "Opens",
            "Apps in use",
            "Function calls",
            "Releases",
            "Storage"
        ]
    );
    assert_eq!(mail.stat_rows.len(), 2, "three to a row");
    let calls = &mail.stat_rows[1][0];
    assert_eq!(
        (calls.value.as_str(), calls.note.as_str()),
        ("1,240", "31 failed")
    );
    let storage = &mail.stat_rows[1][2];
    assert_eq!(
        (storage.value.as_str(), storage.note.as_str()),
        ("3.0 GB", "+1.0 GB")
    );

    assert!(mail.show_calls && mail.show_releases && mail.show_storage);
    let poke = &mail.orgs[0];
    assert_eq!(
        (
            poke.calls.as_str(),
            poke.releases.as_str(),
            poke.storage.as_str()
        ),
        ("1,240", "2", "3.0 GB")
    );
    // Rivermark was never measured: a dash, not zero bytes.
    assert_eq!(mail.orgs[1].storage, "–");
}

#[test]
fn highlights_are_one_line_each_in_the_section_they_belong_to() {
    let mail = view(&week(), Some(CONSOLE));
    assert_eq!(mail.attention.len(), 1);
    let line = &mail.attention[0];
    assert_eq!(
        (
            line.app.as_str(),
            line.org.as_str(),
            line.label.as_str(),
            line.detail.as_str()
        ),
        ("Crew Board", "Rivermark", "Fewer people", "9 → 3 people")
    );
    assert!(mail.good.is_empty());
    assert_eq!(mail.idle, "Archive (Dormant Co), Checklists (Poke House).");
}

#[test]
fn only_orgs_somebody_used_are_tabled() {
    let mail = view(&week(), Some(CONSOLE));
    let names: Vec<&str> = mail.orgs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["Poke House", "Rivermark"]);
    assert_eq!(mail.orgs[0].change, "+2");
    assert_eq!(mail.orgs[1].change, "−6");
    assert_eq!(mail.orgs[0].views, "24");
}

#[test]
fn long_lists_are_cut_and_counted() {
    let many: Vec<_> = (0..12)
        .map(|i| org(i, &format!("Org {i:02}"), vec![app(100 + i, "Tool", 0, 5)]))
        .collect();
    let mail = view(&snapshot(many), None);
    assert_eq!(mail.attention.len(), ATTENTION_IN_MAIL);
    assert_eq!(mail.attention_more, 4);
    assert_eq!(mail.orgs.len(), ORGS_IN_MAIL);
    assert_eq!(mail.orgs_more, 2);

    let idle: Vec<_> = (0..8)
        .map(|i| org(i, &format!("Org {i}"), vec![app(100 + i, "Tool", 0, 0)]))
        .collect();
    let mail = view(&snapshot(idle), None);
    assert!(mail.idle.ends_with("and 2 more."), "{}", mail.idle);
}

#[test]
fn links_point_at_the_console_when_its_address_is_known() {
    let mail = view(&week(), Some(CONSOLE));
    assert_eq!(
        mail.report_url.as_deref(),
        Some("https://app.oxygen-hq.com/admin/usage-report")
    );
    assert_eq!(
        mail.settings_url.as_deref(),
        Some("https://app.oxygen-hq.com/admin/settings")
    );
    let unknown = view(&week(), None);
    assert_eq!(
        (unknown.report_url, unknown.settings_url, unknown.host),
        (None, None, None)
    );
}

#[test]
fn numbers_and_sizes_are_written_for_reading() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1_240), "1,240");
    assert_eq!(grouped(12_345_678), "12,345,678");
    assert_eq!(bytes_label(512), "512 B");
    assert_eq!(bytes_label(12 * 1024), "12 KB");
    assert_eq!(bytes_label(3_565_158), "3.4 MB");
    assert_eq!(bytes_label(5 * 1024 * 1024 * 1024), "5.0 GB");
    assert_eq!(bytes_label(640 * 1024 * 1024), "640 MB");
    assert_eq!(bytes_label(12 * 1024 * 1024 * 1024), "12 GB");
    assert_eq!(bytes_change(1024, 1024), "");
    assert_eq!(bytes_change(1024, 3 * 1024), "−2 KB");
    assert_eq!(change(7, 7), "");
}

#[test]
fn the_whole_mail_renders() {
    let message = compose(&week(), Some(CONSOLE)).unwrap();
    assert!(message.html_body.contains("Crew Board"));
    assert!(
        message
            .text_body
            .contains("- Poke House: 6 people (+2), 24 opens")
    );
    assert!(
        message
            .text_body
            .contains("- Crew Board (Rivermark): Fewer people, 9 → 3 people")
    );
}
