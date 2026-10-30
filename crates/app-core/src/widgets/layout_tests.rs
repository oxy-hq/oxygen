use serde_json::{Value, json};

use super::layout::{CatalogEntry, Drawn, Placed, check, effective, read, resolve};
use super::model::{Size, WidgetDef};

fn widget(id: &str, size: Size, sizes: &[Size]) -> WidgetDef {
    WidgetDef {
        id: id.into(),
        title: id.into(),
        function: format!("{id}-widget"),
        size,
        sizes: sizes.to_vec(),
        params: None,
        actions: vec![],
    }
}

/// Default order: app name, then manifest order.
fn catalog() -> Vec<CatalogEntry> {
    use Size::*;
    vec![
        CatalogEntry::new("bookkeeping", widget("close", Wide, &[Regular, Wide, Full])),
        CatalogEntry::new("store-ops", widget("today", Regular, &[Regular, Wide])),
        CatalogEntry::new("warehouse", widget("receiving", Regular, &[Regular])),
    ]
}

/// Drawn widgets, others shown after.
fn shown(placed: Vec<Placed>) -> Drawn {
    Drawn::Widgets {
        title: None,
        placed,
        hide_others: false,
    }
}

/// A copy drawn at `size`, no id, title or settings.
fn at(index: usize, size: Size) -> Placed {
    Placed {
        index,
        size,
        instance: None,
        title: None,
        params: serde_json::Map::new(),
    }
}

fn problems(doc: Value) -> Vec<(String, String)> {
    check(&doc, &catalog())
        .expect_err("refused")
        .into_iter()
        .map(|p| (p.path, p.message))
        .collect()
}

#[test]
fn no_layout_draws_every_widget_at_its_default_then_apps_then_insights() {
    let drawn = resolve(None, &catalog());
    assert_eq!(
        drawn,
        [
            shown(vec![
                at(0, Size::Wide),
                at(1, Size::Regular),
                at(2, Size::Regular),
            ]),
            Drawn::Apps,
            Drawn::Insights,
        ]
    );
}

#[test]
fn a_layout_orders_sizes_hides_and_adds_the_widgets_it_does_not_list() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "apps" },
        { "kind": "widgets", "items": [
            { "widget": "store-ops/today", "size": "wide" },
            { "widget": "bookkeeping/close", "hidden": true }
        ] }
    ] });
    let layout = check(&doc, &catalog()).expect("valid");
    let drawn = resolve(Some(&layout), &catalog());
    assert_eq!(
        drawn,
        [
            Drawn::Apps,
            shown(vec![
                at(1, Size::Wide),
                // not listed: shown after, at its default
                at(2, Size::Regular),
            ]),
        ],
        "insights left out is not drawn"
    );
}

#[test]
fn others_hide_shows_only_what_is_listed() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "others": "hide", "items": [{ "widget": "warehouse/receiving" }] }
    ] });
    let layout = check(&doc, &catalog()).unwrap();
    assert_eq!(
        resolve(Some(&layout), &catalog()),
        [Drawn::Widgets {
            title: None,
            hide_others: true,
            placed: vec![at(2, Size::Regular)]
        }]
    );
}

#[test]
fn every_problem_is_listed_at_once_with_its_path() {
    let got = problems(json!({ "version": 2, "colour": "red", "sections": [
        { "kind": "widgets", "items": [
            { "widget": "bookkeeping/close", "size": "tall" },
            { "widget": "warehouse/receiving", "size": "full" },
            { "widget": "warehouse/recieving" },
            { "widget": "store-ops/today" },
            { "widget": "store-ops/today" }
        ] },
        { "kind": "apps", "x": 1 },
        { "kind": "apps" },
        { "kind": "dashboard" }
    ] }));
    let paths: Vec<&str> = got.iter().map(|(p, _)| p.as_str()).collect();
    for want in [
        "colour",
        "version",
        "sections[0].items[0].size",
        "sections[0].items[1].size",
        "sections[0].items[2].widget",
        "sections[0].items[4].widget",
        "sections[1].x",
        "sections[2].kind",
        "sections[3].kind",
    ] {
        assert!(paths.contains(&want), "missing {want} in {got:?}");
    }
}

#[test]
fn a_misspelt_widget_gets_a_suggestion() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "items": [{ "widget": "warehouse/recieving" }] }
    ] });
    let fix = &check(&doc, &catalog()).unwrap_err()[0].fix;
    assert!(fix.contains("Did you mean warehouse/receiving?"), "{fix}");
}

#[test]
fn a_stored_layout_tolerates_what_an_app_dropped_since() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "items": [
            { "widget": "gone/widget" },
            { "widget": "bookkeeping/close", "size": "full" }
        ] }
    ] });
    let layout = read(&doc).expect("still parses");
    // bookkeeping/close later narrows to regular only
    let mut narrowed = catalog();
    narrowed[0].def.sizes = vec![Size::Regular];
    narrowed[0].def.size = Size::Regular;
    assert_eq!(
        resolve(Some(&layout), &narrowed),
        [shown(vec![
            at(0, Size::Regular),
            at(1, Size::Regular),
            at(2, Size::Regular),
        ])]
    );
    assert!(read(&json!({ "sections": "x" })).is_none());
}

#[test]
fn the_effective_layout_round_trips() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "items": [
            { "widget": "store-ops/today", "size": "wide" },
            { "widget": "bookkeeping/close", "hidden": true }
        ] },
        { "kind": "insights" }
    ] });
    let layout = check(&doc, &catalog()).unwrap();
    let drawn = resolve(Some(&layout), &catalog());
    let eff = effective(&drawn, &catalog());
    assert_eq!(
        eff["sections"][0]["items"],
        json!([
            { "widget": "store-ops/today", "size": "wide" },
            { "widget": "warehouse/receiving", "size": "regular" },
            { "widget": "bookkeeping/close", "hidden": true }
        ])
    );
    // Writing it back draws the same home.
    let again = check(&eff, &catalog()).expect("the effective layout is valid");
    assert_eq!(resolve(Some(&again), &catalog()), drawn);
    // And so does the default's.
    let default = effective(&resolve(None, &catalog()), &catalog());
    let back = check(&default, &catalog()).unwrap();
    assert_eq!(resolve(Some(&back), &catalog()), resolve(None, &catalog()));
}

#[test]
fn others_hide_survives_the_round_trip() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "others": "hide", "items": [{ "widget": "store-ops/today" }] }
    ] });
    let layout = check(&doc, &catalog()).unwrap();
    let eff = effective(&resolve(Some(&layout), &catalog()), &catalog());
    assert_eq!(eff["sections"][0]["others"], "hide");
    // A widget an app publishes later stays off the home after a write-back.
    let mut grown = catalog();
    grown.push(CatalogEntry::new(
        "ops",
        widget("new", Size::Regular, &[Size::Regular]),
    ));
    let back = check(&eff, &grown).unwrap();
    let Drawn::Widgets { placed, .. } = &resolve(Some(&back), &grown)[0] else {
        panic!("widgets");
    };
    assert_eq!(
        placed.len(),
        1,
        "only store-ops/today; the new widget stays hidden"
    );
}

#[test]
fn an_empty_layout_is_refused_with_the_reset_named() {
    let got = check(&json!({ "version": 1, "sections": [] }), &catalog()).unwrap_err();
    assert_eq!(got[0].path, "sections");
    assert!(got[0].fix.contains("oxyc home reset"), "{}", got[0].fix);
}

/// The catalog plus a Store Ops widget that needs a store.
fn with_settings() -> Vec<CatalogEntry> {
    let mut c = catalog();
    let mut def = widget("store", Size::Regular, &[Size::Regular, Size::Wide]);
    def.params = crate::widgets::params::parse(Some(&json!({
        "type": "object",
        "properties": {
            "store": { "type": "string" },
            "late_after": { "type": "integer", "default": 60 }
        },
        "required": ["store"]
    })));
    c.push(CatalogEntry::new("store-ops", def));
    c
}

#[test]
fn copies_of_one_widget_get_their_own_settings_in_titled_groups() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "items": [{ "widget": "bookkeeping/close" }] },
        { "kind": "widgets", "title": "Stores", "items": [
            { "widget": "store-ops/store", "id": "mission", "title": "Mission St", "params": { "store": "mission" } },
            { "widget": "store-ops/store", "id": "valencia", "params": { "store": "valencia", "late_after": 30 } }
        ] }
    ] });
    let catalog = with_settings();
    let layout = check(&doc, &catalog).expect("valid");
    let drawn = resolve(Some(&layout), &catalog);
    let Drawn::Widgets { title, placed, .. } = &drawn[1] else {
        panic!("second group");
    };
    assert_eq!(title.as_deref(), Some("Stores"));
    assert_eq!(placed.len(), 2);
    assert_eq!(placed[0].key(&catalog), "store-ops/store#mission");
    assert_eq!(placed[0].title.as_deref(), Some("Mission St"));
    assert_eq!(
        Value::Object(placed[0].params.clone()),
        json!({ "store": "mission", "late_after": 60 }),
        "defaults filled in"
    );
    assert_eq!(placed[1].params["late_after"], 30);
    // The first group still gets the widgets the layout does not name — but not
    // one that needs a store to be drawn.
    let Drawn::Widgets { placed: first, .. } = &drawn[0] else {
        panic!("first group");
    };
    let keys: Vec<String> = first.iter().map(|p| p.key(&catalog)).collect();
    assert_eq!(
        keys,
        [
            "bookkeeping/close",
            "store-ops/today",
            "warehouse/receiving"
        ]
    );
}

#[test]
fn a_widget_that_needs_settings_is_never_added_by_default() {
    let catalog = with_settings();
    let Drawn::Widgets { placed, .. } = &resolve(None, &catalog)[0] else {
        panic!("widgets");
    };
    assert!(
        placed
            .iter()
            .all(|p| catalog[p.index].key != "store-ops/store")
    );
}

#[test]
fn settings_and_copies_are_checked_with_paths() {
    let got = check(
        &json!({ "version": 1, "sections": [
            { "kind": "widgets", "items": [
                { "widget": "store-ops/store" },
                { "widget": "store-ops/store", "id": "a", "params": { "store": 7, "colour": "x" } },
                { "widget": "store-ops/store", "id": "a", "params": { "store": "b" } },
                { "widget": "warehouse/receiving", "params": { "x": 1 } },
                { "widget": "bookkeeping/close", "id": "Bad Id", "title": "" }
            ] },
            { "kind": "widgets", "others": "hide" }
        ] }),
        &with_settings(),
    )
    .unwrap_err();
    let paths: Vec<&str> = got.iter().map(|p| p.path.as_str()).collect();
    for want in [
        "sections[0].items[0].params.store",
        "sections[0].items[1].params.store",
        "sections[0].items[1].params.colour",
        "sections[0].items[2].widget",
        "sections[0].items[3].params",
        "sections[0].items[4].id",
        "sections[0].items[4].title",
        "sections[1].others",
    ] {
        assert!(paths.contains(&want), "missing {want} in {got:?}");
    }
    let twice = got
        .iter()
        .find(|p| p.path == "sections[0].items[2].widget")
        .unwrap();
    assert!(twice.fix.contains("own `id`"), "{}", twice.fix);
}

#[test]
fn groups_copies_and_settings_round_trip() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "others": "hide", "items": [{ "widget": "bookkeeping/close", "size": "full" }] },
        { "kind": "apps" },
        { "kind": "widgets", "title": "Stores", "items": [
            { "widget": "store-ops/store", "id": "mission", "title": "Mission St", "params": { "store": "mission" } }
        ] }
    ] });
    let catalog = with_settings();
    let drawn = resolve(Some(&check(&doc, &catalog).unwrap()), &catalog);
    let eff = effective(&drawn, &catalog);
    assert_eq!(eff["sections"][2]["title"], "Stores");
    assert_eq!(eff["sections"][2]["items"][0]["params"]["late_after"], 60);
    assert!(
        eff["sections"][2].get("others").is_none(),
        "others only on the first group"
    );
    let again = check(&eff, &catalog).expect("the effective layout is valid");
    assert_eq!(resolve(Some(&again), &catalog), drawn);
}
