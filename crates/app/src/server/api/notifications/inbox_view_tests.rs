use super::*;

fn item(href: Option<&str>, link: Option<&str>) -> InboxItem {
    let n = notifications::Model {
        id: Uuid::nil(),
        org_id: Uuid::nil(),
        user_id: Uuid::nil(),
        kind: "k".into(),
        title: String::new(),
        body: None,
        link: None,
        subject_kind: None,
        subject_id: None,
        created_at: chrono::Utc::now().fixed_offset(),
        read_at: None,
        message_id: None,
        app_id: None,
        thread_key: None,
    };
    let mut item = base(
        n,
        super::super::catalog::live::from_manifest(None).describe("k"),
    );
    item.title = Some("t".into());
    item.href = href.map(str::to_string);
    item.link = link.map(str::to_string);
    item
}

#[test]
fn hrefs_go_absolute_on_a_custom_app_host_and_stay_relative_elsewhere() {
    let admin = || Some("https://app.oxygen-hq.com".to_string());
    for host in [
        "acme--store.customer-apps.oxygen-hq.com",
        "staging--acme--store.customer-apps-dev.oxygen-hq.com:443",
    ] {
        assert_eq!(
            href_base(Some(host), admin()),
            "https://app.oxygen-hq.com",
            "{host}"
        );
    }
    assert_eq!(href_base(Some("app.oxygen-hq.com"), admin()), "");
    assert_eq!(
        href_base(Some("acme.oxygen-hq.com"), admin()),
        "",
        "an org subdomain serves the SPA"
    );
    assert_eq!(href_base(None, admin()), "");
    assert_eq!(
        href_base(Some("acme--store.customer-apps.oxygen-hq.com"), None),
        "",
        "no admin host configured: relative, not invented"
    );

    let mut items = vec![
        item(Some("/customer-apps/acme/store/lists/1"), Some("/lists/1")),
        item(Some("/acme/workspaces/w"), None),
        item(None, None),
    ];
    absolutize(&mut items, "https://app.oxygen-hq.com");
    assert_eq!(
        items[0].href.as_deref(),
        Some("https://app.oxygen-hq.com/customer-apps/acme/store/lists/1")
    );
    assert_eq!(
        items[0].link.as_deref(),
        Some("/lists/1"),
        "the app's own link is untouched"
    );
    assert_eq!(
        items[1].href.as_deref(),
        Some("https://app.oxygen-hq.com/acme/workspaces/w")
    );
    assert_eq!(items[2].href, None);
}

#[test]
fn url_is_the_admin_hosts_absolute_link_whatever_host_reads_it() {
    let links = Links {
        href_base: String::new(),
        admin_base: Some("https://app.oxygen-hq.com".into()),
    };
    let mut items = vec![item(Some("/acme/workspaces/w"), None), item(None, None)];
    links.apply(&mut items);
    assert_eq!(
        items[0].url.as_deref(),
        Some("https://app.oxygen-hq.com/acme/workspaces/w")
    );
    assert_eq!(
        items[0].href.as_deref(),
        Some("/acme/workspaces/w"),
        "href stays relative here"
    );
    assert_eq!(items[1].url, None);
    let mut items = vec![item(Some("/acme/workspaces/w"), None)];
    Links::default().apply(&mut items);
    assert_eq!(items[0].url, None, "no admin host: no invented url");
}

#[test]
fn every_field_is_in_the_json_null_when_absent() {
    // The SDK reads a missing key as a server that predates the field.
    let v = serde_json::to_value(item(None, None)).unwrap();
    for key in ["link", "thread", "url", "app", "body", "href", "read_at"] {
        assert_eq!(v.get(key), Some(&serde_json::Value::Null), "{key}");
    }
    assert_eq!(
        v["category"],
        serde_json::json!({ "path": "general", "labels": ["General"] })
    );
    assert_eq!(v["kind_label"], "K");
    assert_eq!(v["level"], "active");
    assert_eq!(v["actions"], serde_json::json!([]));
}

fn row(kind: &str, title: &str, body: Option<&str>) -> notifications::Model {
    notifications::Model {
        id: Uuid::nil(),
        org_id: Uuid::nil(),
        user_id: Uuid::nil(),
        kind: kind.into(),
        title: title.into(),
        body: body.map(str::to_string),
        link: None,
        subject_kind: Some("experiment_run".into()),
        subject_id: Some("r-1".into()),
        created_at: chrono::Utc::now().fixed_offset(),
        read_at: None,
        message_id: None,
        app_id: None,
        thread_key: None,
    }
}

#[test]
fn a_row_with_no_app_and_no_stored_message_reads_its_own_words() {
    // The shape `server::experiment::notify` writes.
    let n = row(
        "experiment",
        "Experiment promo",
        Some("guardrail breached: x"),
    );
    assert_eq!(
        own_words(&n),
        Some((
            "Experiment promo".into(),
            Some("guardrail breached: x".into())
        ))
    );
}

#[test]
fn an_apps_row_or_one_with_a_stored_message_or_an_empty_title_does_not() {
    let mut app = row("experiment", "t", None);
    app.app_id = Some(Uuid::new_v4());
    assert_eq!(
        own_words(&app),
        None,
        "an app's words live in its org store"
    );
    let mut stored = row("experiment", "t", None);
    stored.message_id = Some(Uuid::new_v4());
    assert_eq!(own_words(&stored), None, "the store's words win");
    assert_eq!(
        own_words(&row("experiment", "", None)),
        None,
        "a legacy empty title"
    );
}
