//! What `GET /api/home/widgets` does with `for` and a person's hidden copies,
//! short of the database: who is left out before any function runs, what the
//! restore list names, and the shape of the answer.

use oxy_app_core::widgets::layout::{
    CatalogEntry, Drawn, Viewer, check, resolve, visible, without,
};
use oxy_app_core::widgets::model::{Size, WidgetDef};
use serde_json::json;
use uuid::Uuid;

use super::load::WidgetApp;
use super::{ListResponse, SectionOut, hidden_cards};

fn def(id: &str) -> WidgetDef {
    WidgetDef {
        id: id.into(),
        title: format!("{id} title"),
        function: format!("{id}-widget"),
        size: Size::Regular,
        sizes: vec![Size::Regular],
        params: None,
        actions: vec![],
    }
}

fn app(slug: &str) -> WidgetApp {
    WidgetApp {
        id: Uuid::nil(),
        slug: slug.into(),
        name: slug.into(),
        org_slug: "poke-house".into(),
        url: format!("https://{slug}.example.com/"),
    }
}

/// bookkeeping/close, store-ops/today.
fn setup() -> (Vec<CatalogEntry>, Vec<WidgetApp>) {
    let apps = vec![app("bookkeeping"), app("store-ops")];
    let entries = vec![
        CatalogEntry::new("bookkeeping", def("close")),
        CatalogEntry::new("store-ops", def("today")),
    ];
    (entries, apps)
}

fn layout() -> serde_json::Value {
    json!({ "version": 1, "sections": [
        { "kind": "widgets", "title": "Back office", "others": "hide",
          "for": { "people": ["scott@poke.house"] },
          "items": [{ "widget": "bookkeeping/close", "title": "Month-end" }] },
        { "kind": "widgets", "items": [{ "widget": "store-ops/today" }] },
        { "kind": "apps" }
    ] })
}

fn keys(drawn: &[Drawn], entries: &[CatalogEntry]) -> Vec<String> {
    drawn
        .iter()
        .flat_map(|d| match d {
            Drawn::Widgets { placed, .. } => placed.iter().map(|p| p.key(entries)).collect(),
            _ => vec![],
        })
        .collect()
}

#[test]
fn a_copy_the_caller_is_not_in_for_is_never_drawn_so_never_run() {
    let (entries, _) = setup();
    let layout = check(&layout(), &entries).unwrap();
    let drawn = resolve(Some(&layout), &entries);
    let amy = visible(drawn.clone(), &Viewer::new(Some("amy@poke.house"), []));
    assert_eq!(keys(&amy, &entries), ["store-ops/today"]);
    let scott = visible(drawn, &Viewer::new(Some("Scott@poke.house"), []));
    assert_eq!(
        keys(&scott, &entries),
        ["bookkeeping/close", "store-ops/today"]
    );
}

#[test]
fn hidden_copies_are_listed_for_restore_only_when_the_caller_could_see_them() {
    let (entries, apps) = setup();
    let catalog: Vec<(CatalogEntry, &WidgetApp)> =
        entries.iter().cloned().zip(apps.iter()).collect();
    let layout = check(&layout(), &entries).unwrap();
    let hidden = vec![
        "bookkeeping/close".to_string(),
        "store-ops/today".to_string(),
        "gone/widget".to_string(),
    ];
    // Amy is not in the Back office's `for`: bookkeeping/close is not hers to
    // restore, and a key an app dropped names nothing.
    let amy = visible(
        resolve(Some(&layout), &entries),
        &Viewer::new(Some("amy@poke.house"), []),
    );
    let listed = hidden_cards(&amy, &catalog, &entries, &hidden);
    assert_eq!(
        listed.iter().map(|h| h.key.as_str()).collect::<Vec<_>>(),
        ["store-ops/today"]
    );
    assert_eq!(listed[0].title, "today title");
    assert_eq!(listed[0].app.slug, "store-ops");
    // Hiding takes them out of the drawn home, and out of its sections.
    let left = without(amy, |p| hidden.contains(&p.key(&entries)));
    assert!(keys(&left, &entries).is_empty());
    assert!(left.iter().all(|d| !matches!(d, Drawn::Widgets { .. })));
    // Scott can restore both, the first with the layout's title for it.
    let scott = visible(
        resolve(Some(&layout), &entries),
        &Viewer::new(Some("scott@poke.house"), []),
    );
    let listed = hidden_cards(&scott, &catalog, &entries, &hidden);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].title, "Month-end");
}

#[test]
fn the_answer_carries_hidden_and_today() {
    let body = serde_json::to_value(ListResponse {
        widgets: vec![],
        sections: vec![SectionOut::Apps],
        hidden: vec![],
        today: "hidden",
    })
    .unwrap();
    assert_eq!(
        body,
        json!({ "widgets": [], "sections": [{ "kind": "apps" }], "hidden": [], "today": "hidden" })
    );
    let empty = serde_json::to_value(ListResponse::empty()).unwrap();
    assert_eq!(empty["today"], "shown");
    assert_eq!(empty["hidden"], json!([]));
}
