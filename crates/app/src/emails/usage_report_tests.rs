use super::*;

fn stat(label: &str, value: &str, note: &str) -> EmailStat {
    EmailStat {
        label: label.into(),
        value: value.into(),
        note: note.into(),
    }
}

fn mail() -> ReportEmail {
    ReportEmail {
        subject: "Custom app usage, Sep 28 – Oct 4: 9 people in 2 apps, 1 needs a look".into(),
        period: "Sep 28 – Oct 4, 2026".into(),
        headline: "9 people opened 2 custom apps across 2 organizations.".into(),
        stat_rows: vec![vec![
            stat("People", "9", "−4"),
            stat("Opens", "36", ""),
            stat("Function calls", "240", "31 failed"),
        ]],
        attention: vec![EmailHighlight {
            app: "Crew <b>Board</b>".into(),
            org: "Rivermark".into(),
            label: "Fewer people".into(),
            detail: "9 → 3 people".into(),
        }],
        attention_more: 2,
        good: vec![],
        good_more: 0,
        orgs: vec![EmailOrg {
            name: "Poke House".into(),
            people: 6,
            change: "+2".into(),
            views: "24".into(),
            calls: "240".into(),
            releases: "".into(),
            storage: "1.5 GB".into(),
        }],
        orgs_more: 0,
        show_calls: true,
        show_releases: false,
        show_storage: true,
        idle: "Checklists (Poke House).".into(),
        host: Some("app.oxygen-hq.com".into()),
        report_url: Some("https://app.oxygen-hq.com/admin/usage-report".into()),
        settings_url: Some("https://app.oxygen-hq.com/admin/settings".into()),
    }
}

#[test]
fn the_mail_leads_with_the_headline_and_ends_with_the_links() {
    let message = render(&mail()).unwrap();
    let text = &message.text_body;
    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("no {needle:?}"));
    assert!(at("9 people opened") < at("People 9 (−4) | Opens 36"));
    assert!(at("Opens 36") < at("Needs a look"));
    assert!(at("Needs a look") < at("By organization"));
    assert!(at("By organization") < at("Full report: https://"));
    assert!(at("Full report:") < at("Stop these emails: https://"));
    assert!(text.contains("- Crew <b>Board</b> (Rivermark): Fewer people, 9 → 3 people"));
    assert!(text.contains("- And 2 more in the full report."));
    assert!(!text.contains("Going well"), "an empty section is left out");
    assert_eq!(message.subject, mail().subject);
}

#[test]
fn the_table_carries_only_the_columns_it_can_fill() {
    let message = render(&mail()).unwrap();
    assert!(
        message
            .text_body
            .contains("- Poke House: 6 people (+2), 24 opens, 240 calls, 1.5 GB")
    );
    let mut shipped = mail();
    shipped.show_releases = true;
    shipped.orgs[0].releases = "1".into();
    assert!(
        render(&shipped)
            .unwrap()
            .text_body
            .contains("240 calls, 1 release, 1.5 GB")
    );
    let html = message.html_body;
    assert!(html.contains(">Calls<") && html.contains(">Storage<"));
    assert!(!html.contains(">Releases<"), "nothing was released");
    assert!(html.contains("1.5 GB"));
}

#[test]
fn the_html_escapes_names_an_org_chose() {
    let html = render(&mail()).unwrap().html_body;
    assert!(html.contains("Crew &lt;b&gt;Board&lt;/b&gt;"));
    assert!(!html.contains("Crew <b>Board</b>"));
    assert!(html.contains("href=\"https://app.oxygen-hq.com/admin/usage-report\""));
    assert!(html.contains("on app.oxygen-hq.com"));
    assert!(!html.contains("Going well"));
    assert!(html.contains("31 failed"));
}

#[test]
fn a_week_with_nothing_to_look_at_says_so_in_one_line() {
    let mut calm = mail();
    calm.attention.clear();
    calm.attention_more = 0;
    let message = render(&calm).unwrap();
    assert!(
        message
            .text_body
            .contains("Nothing needs a look this week.")
    );
    assert!(
        message
            .html_body
            .contains("Nothing needs a look this week.")
    );
    assert!(!message.html_body.contains("check in with them"));
}

#[test]
fn with_no_known_address_the_mail_says_where_to_click() {
    let mut m = mail();
    m.report_url = None;
    m.settings_url = None;
    m.host = None;
    let message = render(&m).unwrap();
    assert!(
        message
            .text_body
            .contains("Full report: Admin, then Usage report.")
    );
    assert!(!message.html_body.contains("Open the full report"));
    assert!(message.html_body.contains("open Admin, then Settings"));
}

#[test]
fn a_preview_deployment_previews_and_an_unconfigured_one_is_off() {
    assert_eq!(mode_for(true, false), DeliveryMode::Email);
    assert_eq!(mode_for(true, true), DeliveryMode::Preview);
    assert_eq!(mode_for(false, true), DeliveryMode::Preview);
    assert_eq!(mode_for(false, false), DeliveryMode::Off);
    assert_eq!(
        serde_json::to_value(DeliveryMode::Off).unwrap(),
        serde_json::json!("off")
    );
}
