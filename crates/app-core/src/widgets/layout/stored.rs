//! How a layout is stored so that a server older than `for` can still read it.
//!
//! A server one release back refuses any field it does not know, and a refused
//! stored layout draws the default home: the org's groups, order and sizes gone for
//! everyone, over one field. So `for` never goes in the stored document. `split`
//! takes it out before a write into a side value (`org_home_layouts.audiences`);
//! `join` puts it back on read. The older server reads the document alone, a layout
//! it fully understands, and only loses the narrowing.
//!
//! The side value carries the document it was split from. An older server that saves
//! a layout of its own leaves the side value as it was, now describing a different
//! document; `join` sees the two differ and ignores it, rather than pinning a stale
//! `for` onto whatever now sits at the same index. Compared as JSON values, not text:
//! Postgres reorders the keys of a `jsonb`.

use serde_json::{Map, Value, json};

const FOR: &str = "for";

/// `doc` without any `for`, and the `for`s taken out (`None` when there were none).
pub fn split(doc: &Value) -> (Value, Option<Value>) {
    let mut bare = doc.clone();
    let mut sections = Map::new();
    let mut items = Map::new();
    if let Some(list) = bare.get_mut("sections").and_then(Value::as_array_mut) {
        for (s, section) in list.iter_mut().enumerate() {
            let Some(obj) = section.as_object_mut() else {
                continue;
            };
            if let Some(audience) = obj.remove(FOR) {
                sections.insert(s.to_string(), audience);
            }
            let Some(entries) = obj.get_mut("items").and_then(Value::as_array_mut) else {
                continue;
            };
            for (i, item) in entries.iter_mut().enumerate() {
                if let Some(audience) = item.as_object_mut().and_then(|o| o.remove(FOR)) {
                    items.insert(format!("{s}/{i}"), audience);
                }
            }
        }
    }
    if sections.is_empty() && items.is_empty() {
        return (bare, None);
    }
    let side = json!({ "layout": bare.clone(), "sections": sections, "items": items });
    (bare, Some(side))
}

/// `doc` with the `for`s of `side` put back, when `side` was split from this very
/// document; otherwise `doc` unchanged.
pub fn join(doc: Value, side: Option<&Value>) -> Value {
    let Some(side) = side.filter(|s| s.get("layout") == Some(&doc)) else {
        return doc;
    };
    let mut doc = doc;
    let Some(list) = doc.get_mut("sections").and_then(Value::as_array_mut) else {
        return doc;
    };
    for (s, section) in list.iter_mut().enumerate() {
        let Some(obj) = section.as_object_mut() else {
            continue;
        };
        if let Some(audience) = side.pointer(&format!("/sections/{s}")) {
            obj.insert(FOR.into(), audience.clone());
        }
        let Some(entries) = obj.get_mut("items").and_then(Value::as_array_mut) else {
            continue;
        };
        for (i, item) in entries.iter_mut().enumerate() {
            let at = format!("{s}/{i}");
            let found = side.get("items").and_then(|m| m.get(&at));
            if let (Some(audience), Some(obj)) = (found, item.as_object_mut()) {
                obj.insert(FOR.into(), audience.clone());
            }
        }
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_for() -> Value {
        json!({ "version": 1, "sections": [
            { "kind": "apps" },
            { "kind": "widgets", "title": "Back office",
              "for": { "people": ["scott@poke.house"] },
              "items": [
                { "widget": "warehouse/receiving" },
                { "widget": "bookkeeping/close", "size": "wide",
                  "for": { "teams": ["office"] } }
              ] }
        ] })
    }

    fn mentions_for(v: &Value) -> bool {
        match v {
            Value::Object(m) => m.contains_key(FOR) || m.values().any(mentions_for),
            Value::Array(a) => a.iter().any(mentions_for),
            _ => false,
        }
    }

    #[test]
    fn the_stored_document_never_carries_for_and_joins_back_whole() {
        let (bare, side) = split(&with_for());
        assert!(
            !mentions_for(&bare),
            "an older server must be able to read it"
        );
        assert_eq!(join(bare, side.as_ref()), with_for());
    }

    #[test]
    fn a_layout_without_for_has_no_side_value() {
        let doc = json!({ "version": 1, "sections": [{ "kind": "widgets", "items": [] }] });
        assert_eq!(split(&doc), (doc.clone(), None));
        assert_eq!(join(doc.clone(), None), doc);
    }

    #[test]
    fn a_side_value_from_another_document_is_ignored() {
        // An older server saved its own layout: the side value still describes the old one.
        let (_, side) = split(&with_for());
        let theirs = json!({ "version": 1, "sections": [
            { "kind": "widgets", "items": [{ "widget": "bookkeeping/close" }] }
        ] });
        assert_eq!(join(theirs.clone(), side.as_ref()), theirs);
    }

    #[test]
    fn key_order_does_not_matter() {
        // jsonb hands the document back with its keys in its own order.
        let (_, side) = split(&with_for());
        let reordered: Value = serde_json::from_str(
            r#"{ "sections": [
                { "kind": "apps" },
                { "items": [
                    { "widget": "warehouse/receiving" },
                    { "size": "wide", "widget": "bookkeeping/close" }
                  ], "title": "Back office", "kind": "widgets" }
              ], "version": 1 }"#,
        )
        .unwrap();
        assert_eq!(join(reordered, side.as_ref()), with_for());
    }
}
