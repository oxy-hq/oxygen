use serde_json::{Value, json};

use super::model::{ActionDef, Size, WidgetDef};
use super::view::{Level, Progress, RowState, Trend, check, link};

const APP: &str = "https://acme--books.apps.example.com/";

fn def() -> WidgetDef {
    WidgetDef {
        id: "close".into(),
        title: "Close".into(),
        function: "close-widget".into(),
        size: Size::Wide,
        sizes: vec![Size::Wide],
        params: None,
        actions: vec![
            ActionDef {
                id: "post".into(),
                function: "post-ready".into(),
                confirm: true,
                destructive: false,
            },
            ActionDef {
                id: "undo".into(),
                function: "undo".into(),
                confirm: false,
                destructive: true,
            },
        ],
    }
}

#[test]
fn a_full_view_comes_through_with_links_made_absolute() {
    let raw = json!({
        "headline": "19 stores ready to post",
        "detail": "September · 61 of 84 posted",
        "level": "attention",
        "progress": { "done": 61, "total": 84 },
        "rows": [{ "title": "Valencia", "detail": "Not mapped", "state": "blocked", "link": "/settings?tab=map" }],
        "buttons": [{ "action": "post", "label": "Post 19 ready", "input": { "period": "2026-09" }, "key": "2026-09" }],
        "link": "?month=2026-09",
        "badge": 19
    });
    let view = check(&raw, &def(), APP).expect("a view");
    assert_eq!(view.level, Level::Attention);
    assert_eq!(
        view.progress,
        Some(Progress {
            done: 61,
            total: 84
        })
    );
    assert_eq!(view.rows[0].state, Some(RowState::Blocked));
    assert_eq!(
        view.rows[0].href.as_deref(),
        Some("https://acme--books.apps.example.com/settings?tab=map")
    );
    assert_eq!(
        view.href.as_deref(),
        Some("https://acme--books.apps.example.com/?month=2026-09")
    );
    let button = &view.buttons[0];
    assert_eq!(button.key.as_deref(), Some("2026-09"));
    // The manifest asks for a confirmation, so the view gets one even unasked.
    assert_eq!(button.confirm.as_deref(), Some("Post 19 ready?"));
    assert_eq!(view.badge, Some(19));
}

#[test]
fn no_headline_is_no_view() {
    assert!(check(&json!({ "detail": "x" }), &def(), APP).is_none());
    assert!(check(&json!({ "headline": "  " }), &def(), APP).is_none());
    assert!(check(&Value::Null, &def(), APP).is_none());
}

#[test]
fn only_app_relative_links_survive() {
    for bad in [
        "javascript:alert(1)",
        "https://evil.example.com",
        "//evil.example.com/x",
        "/\\evil.example.com",
        "/a\nb",
        "settings",
        "/../other-app/",
        "/a/../../admin",
        "/%2e%2e/other-app",
        "/%2E./x",
        "/./x",
    ] {
        assert_eq!(link(Some(&json!(bad)), APP), None, "{bad}");
    }
    // Dots inside a name, and dots in the query, are not path segments.
    assert_eq!(
        link(Some(&json!("/v1.2/report?next=../x")), APP),
        Some("https://acme--books.apps.example.com/v1.2/report?next=../x".into())
    );
    assert_eq!(
        link(Some(&json!("/x")), "/customer-apps/acme/books"),
        Some("/customer-apps/acme/books/x".into())
    );
}

#[test]
fn buttons_for_undeclared_actions_and_oversized_input_are_dropped() {
    let raw = json!({
        "headline": "h",
        "buttons": [
            { "action": "nope", "label": "Nope" },
            { "action": "post", "label": "Big", "input": "x".repeat(5000) },
            { "action": "undo", "label": "Undo", "confirm": "Undo September?" },
            { "action": "post", "label": "Third" }
        ]
    });
    let view = check(&raw, &def(), APP).unwrap();
    assert_eq!(view.buttons.len(), 2);
    assert_eq!(view.buttons[0].action, "undo");
    assert!(view.buttons[0].destructive);
    assert_eq!(view.buttons[0].confirm.as_deref(), Some("Undo September?"));
    assert_eq!(view.buttons[1].label, "Third");
}

#[test]
fn text_and_lists_are_cut_and_odd_values_ignored() {
    let rows: Vec<Value> = (0..11)
        .map(|i| json!({ "title": format!("r{i}") }))
        .collect();
    let raw = json!({
        "headline": "h".repeat(200),
        "level": "panic",
        "progress": { "done": 90, "total": 84 },
        "rows": rows,
        "badge": 123456
    });
    let view = check(&raw, &def(), APP).unwrap();
    assert_eq!(view.headline.chars().count(), 80);
    assert_eq!(view.level, Level::Info);
    assert_eq!(
        view.progress,
        Some(Progress {
            done: 84,
            total: 84
        })
    );
    assert_eq!(view.rows.len(), 8);
    assert_eq!(view.badge, Some(9_999));
    let zero = check(
        &json!({ "headline": "h", "progress": { "done": 1, "total": 0 } }),
        &def(),
        APP,
    );
    assert_eq!(zero.unwrap().progress, None);
}

#[test]
fn a_metric_comes_through_checked() {
    let spark: Vec<Value> = (0..40).map(|i| json!(i)).collect();
    let raw = json!({
        "headline": "Revenue",
        "metric": { "value": "$48,210", "label": "Posted this month", "delta": "+12%", "trend": "up", "spark": spark }
    });
    let m = check(&raw, &def(), APP).unwrap().metric.expect("a metric");
    assert_eq!(m.value, "$48,210");
    assert_eq!(m.trend, Some(Trend::Up));
    assert_eq!(m.spark.len(), 30);
    let no_value = json!({ "headline": "h", "metric": { "label": "x" } });
    assert!(check(&no_value, &def(), APP).unwrap().metric.is_none());
    let odd = json!({ "headline": "h", "metric": { "value": "1", "trend": "sideways", "spark": ["a", 2] } });
    let m = check(&odd, &def(), APP).unwrap().metric.unwrap();
    assert_eq!((m.trend, m.spark), (None, vec![2.0]));
}

#[test]
fn refresh_after_is_clamped() {
    let at = |n: u64| {
        check(&json!({ "headline": "h", "refresh_after": n }), &def(), APP)
            .unwrap()
            .refresh_after
    };
    assert_eq!(
        (at(5), at(300), at(999_999)),
        (Some(60), Some(300), Some(86_400))
    );
    assert_eq!(
        check(&json!({ "headline": "h" }), &def(), APP)
            .unwrap()
            .refresh_after,
        None
    );
}
