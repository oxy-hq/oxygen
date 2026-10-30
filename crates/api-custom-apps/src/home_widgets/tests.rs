use axum::http::StatusCode;
use oxy_app_core::function_answer::Outcome;
use oxy_app_core::widgets::model::{ActionDef, Size, WidgetDef};
use serde_json::{Value, json};

use super::run::{Press, Pressed, pressed};

const APP: &str = "https://acme--books.apps.example.com/";

fn widget() -> WidgetDef {
    WidgetDef {
        id: "close".into(),
        title: "Close".into(),
        function: "close-widget".into(),
        size: Size::Regular,
        sizes: vec![Size::Regular],
        params: None,
        actions: vec![ActionDef {
            id: "post-ready".into(),
            function: "post-ready-jes".into(),
            confirm: true,
            destructive: false,
        }],
    }
}

#[test]
fn a_press_sends_the_widget_the_action_and_the_input() {
    let w = widget();
    let press = Press {
        widget: &w,
        action: &w.actions[0],
        input: json!({ "period": "2026-09" }),
        key: Some("2026-09".into()),
    };
    let body: Value = serde_json::from_slice(&press.body()).unwrap();
    assert_eq!(
        body,
        json!({ "widget": "close", "action": "post-ready", "input": { "period": "2026-09" } })
    );
    let key = press.idempotency_key();
    assert!(key.starts_with("widget:close:post-ready:k-"), "{key}");
    let again = Press {
        input: json!({ "period": "2026-09", "note": "different input, same key" }),
        ..press
    };
    assert_eq!(
        again.idempotency_key(),
        key,
        "the key, not the input, names the press"
    );
}

#[test]
fn without_a_key_the_same_input_is_the_same_press() {
    let w = widget();
    let press = |input: Value| Press {
        widget: &w,
        action: &w.actions[0],
        input,
        key: None,
    };
    let a = press(json!({ "period": "2026-09" })).idempotency_key();
    assert_eq!(a, press(json!({ "period": "2026-09" })).idempotency_key());
    assert_ne!(a, press(json!({ "period": "2026-10" })).idempotency_key());
    assert!(a.starts_with("widget:close:post-ready:in-"));
    assert!(a.len() < 200, "under the function route's key cap");
}

#[test]
fn any_key_becomes_a_header_the_function_route_reads() {
    let w = widget();
    for key in [
        "Café-2026-09",
        "2026–09",
        "tab\there",
        "a b",
        &"x".repeat(120),
    ] {
        let press = Press {
            widget: &w,
            action: &w.actions[0],
            input: Value::Null,
            key: Some(key.to_string()),
        };
        let header = press.idempotency_key();
        let value = axum::http::HeaderValue::from_str(&header).expect("a valid header");
        // What `custom_apps_functions` does with it: absent unless visible ASCII.
        assert_eq!(value.to_str().ok(), Some(header.as_str()), "{key}");
        assert!(header.len() < 200);
    }
}

#[test]
fn a_done_answer_carries_its_message_and_a_checked_view() {
    let got = pressed(
        Outcome::Done(
            json!({ "message": " Posted 19 ", "view": { "headline": "All posted", "level": "ok" } }),
        ),
        &widget(),
        APP,
    );
    let Pressed::Done { message, view } = got else {
        panic!("expected done");
    };
    assert_eq!(message.as_deref(), Some("Posted 19"));
    assert_eq!(view.unwrap().headline, "All posted");
    assert_eq!(
        pressed(Outcome::Done(Value::Null), &widget(), APP),
        Pressed::Done {
            message: None,
            view: None
        }
    );
}

#[test]
fn a_refused_credential_is_403_never_401() {
    let Pressed::Failed { status, error, .. } = pressed(
        Outcome::Refused(StatusCode::UNAUTHORIZED, Value::Null),
        &widget(),
        APP,
    ) else {
        panic!("expected failed");
    };
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error, "FunctionRefused");
    let Pressed::Failed {
        status, message, ..
    } = pressed(Outcome::Failed("boom".into()), &widget(), APP)
    else {
        panic!("expected failed");
    };
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(message, "boom");
}

mod describe {
    use oxy_app_core::widgets::layout::{CatalogEntry, check};
    use oxy_app_core::widgets::model::{Size, WidgetDef};
    use serde_json::json;
    use uuid::Uuid;

    use crate::home_widgets::describe::{OrgRef, SectionView, describe};
    use crate::home_widgets::load::WidgetApp;

    fn app(slug: &str) -> WidgetApp {
        WidgetApp {
            id: Uuid::new_v4(),
            slug: slug.into(),
            name: slug.into(),
            org_slug: "acme".into(),
            url: format!("/customer-apps/acme/{slug}/"),
        }
    }

    fn def(id: &str, sizes: &[Size]) -> WidgetDef {
        WidgetDef {
            id: id.into(),
            title: id.into(),
            function: format!("{id}-fn"),
            size: sizes[0],
            sizes: sizes.to_vec(),
            params: None,
            actions: vec![],
        }
    }

    fn org() -> OrgRef {
        OrgRef {
            id: Uuid::nil(),
            slug: "acme".into(),
            name: "Acme".into(),
        }
    }

    #[test]
    fn a_customized_home_says_what_shows_what_does_not_and_how_to_write_it_back() {
        let (books, ops) = (app("books"), app("ops"));
        let catalog = vec![
            (
                CatalogEntry::new("books", def("close", &[Size::Regular, Size::Full])),
                &books,
            ),
            (
                CatalogEntry::new("ops", def("today", &[Size::Regular])),
                &ops,
            ),
        ];
        let doc = json!({ "version": 1, "sections": [
            { "kind": "widgets", "items": [
                { "widget": "books/close", "size": "full" },
                { "widget": "ops/today", "hidden": true }
            ] },
            { "kind": "apps" }
        ] });
        let entries: Vec<CatalogEntry> = catalog.iter().map(|(e, _)| e.clone()).collect();
        let layout = check(&doc, &entries).unwrap();
        let d = describe(org(), Some((doc.clone(), 4)), Some(&layout), &catalog);
        assert_eq!((d.revision, d.customized), (4, true));
        let SectionView::Widgets { items, .. } = &d.sections[0] else {
            panic!("widgets first");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(
            (items[0].widget.as_str(), items[0].size),
            ("books/close", Size::Full)
        );
        assert!(matches!(d.sections[1], SectionView::Apps));
        assert_eq!(d.sections.len(), 2, "insights left out");
        assert_eq!(
            d.catalog
                .iter()
                .map(|c| (c.widget.as_str(), c.shown))
                .collect::<Vec<_>>(),
            [("books/close", true), ("ops/today", false)]
        );
        // The effective layout is valid and complete.
        assert!(check(&d.effective_layout, &entries).is_ok());
        let wire = serde_json::to_value(&d).unwrap();
        assert_eq!(wire["sections"][0]["kind"], "widgets");
        assert_eq!(wire["sections"][0]["items"][0]["size"], "full");
    }

    #[test]
    fn copies_with_settings_and_what_a_widget_needs_are_described() {
        let ops = app("ops");
        let mut store = def("store", &[Size::Regular]);
        store.params = oxy_app_core::widgets::params::parse(Some(&json!({
            "type": "object",
            "properties": { "store": { "type": "string" } },
            "required": ["store"]
        })));
        let catalog = vec![(CatalogEntry::new("ops", store), &ops)];
        let doc = json!({ "version": 1, "sections": [
            { "kind": "widgets", "title": "Stores", "items": [
                { "widget": "ops/store", "id": "mission", "title": "Mission St", "params": { "store": "mission" } }
            ] }
        ] });
        let entries: Vec<CatalogEntry> = catalog.iter().map(|(e, _)| e.clone()).collect();
        let layout = check(&doc, &entries).unwrap();
        let wire =
            serde_json::to_value(describe(org(), Some((doc, 1)), Some(&layout), &catalog)).unwrap();
        let group = &wire["sections"][0];
        assert_eq!(group["title"], "Stores");
        assert_eq!(group["items"][0]["key"], "ops/store#mission");
        assert_eq!(group["items"][0]["title"], "Mission St");
        assert_eq!(group["items"][0]["params"], json!({ "store": "mission" }));
        assert_eq!(wire["catalog"][0]["needs_params"], json!(["store"]));
        assert_eq!(wire["catalog"][0]["params"]["required"], json!(["store"]));
        // Unplaced, it would not show: nothing to say which store.
        let bare = serde_json::to_value(describe(org(), None, None, &catalog)).unwrap();
        assert_eq!(bare["catalog"][0]["shown"], false);
    }

    #[test]
    fn an_uncustomized_home_is_revision_zero_with_the_default() {
        let books = app("books");
        let catalog = vec![(
            CatalogEntry::new("books", def("close", &[Size::Wide])),
            &books,
        )];
        let d = describe(org(), None, None, &catalog);
        assert_eq!(
            (d.revision, d.customized, d.layout.is_none()),
            (0, false, true)
        );
        assert_eq!(d.sections.len(), 3);
    }
}

#[test]
fn a_null_revision_is_refused_never_an_unchecked_overwrite() {
    use crate::home_widgets::layout_routes::{PutBody, expected_revision};
    let body = |v: Value| serde_json::from_value::<PutBody>(v).unwrap().revision;
    let layout = json!({ "version": 1, "sections": [{ "kind": "apps" }] });
    // Absent: nothing to check, an overwrite the caller asked for.
    assert_eq!(
        expected_revision(body(json!({ "layout": layout }))),
        Some(None)
    );
    // A number: checked.
    assert_eq!(
        expected_revision(body(json!({ "layout": layout, "revision": 3 }))),
        Some(Some(3))
    );
    // `null` (a client's NaN): refused.
    assert_eq!(
        expected_revision(body(json!({ "layout": layout, "revision": null }))),
        None
    );
}
