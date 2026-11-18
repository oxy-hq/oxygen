//! The manifest's side of app events: the app-level `events` catalogue (what
//! this build declares it can publish, each with its embedded JSON Schema) and
//! the function-level keys, `events.publish` (the capability to publish) and
//! `on` (the events a function listens to).
//!
//! These read what the build declares. They never decide identity: the host
//! stamps the publisher, org and environment.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::valid_event_name;

/// One declared event.
#[derive(Debug, Clone, PartialEq)]
pub struct EventDecl {
    pub description: Option<String>,
    /// The JSON Schema `oxyc publish` embedded in the manifest.
    pub schema: Value,
}

/// An app's declared events, by bare name (`statement.uploaded`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventCatalog {
    pub events: BTreeMap<String, EventDecl>,
}

const NAME_RULE: &str =
    "must be lowercase, start with a letter and be dot-separated letters and digits";

impl EventCatalog {
    /// Read the manifest's top-level `events` block (`None` or absent: no events).
    pub fn from_manifest(manifest: Option<&Value>) -> Result<Self, String> {
        let Some(block) = manifest.and_then(|m| m.get("events")) else {
            return Ok(Self::default());
        };
        let block = block
            .as_object()
            .ok_or("`events` must be an object of event name to declaration")?;
        let mut events = BTreeMap::new();
        for (name, decl) in block {
            if !valid_event_name(name) {
                return Err(format!("event `{name}`: the name {NAME_RULE}"));
            }
            let schema = match decl.get("schema") {
                Some(s) if s.is_object() => s.clone(),
                Some(Value::String(_)) => {
                    return Err(format!(
                        "event `{name}`: schema is a file path; publish with oxyc, which embeds it"
                    ));
                }
                _ => {
                    return Err(format!(
                        "event `{name}`: schema must be a JSON Schema object"
                    ));
                }
            };
            let description = decl
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned);
            events.insert(
                name.clone(),
                EventDecl {
                    description,
                    schema,
                },
            );
        }
        Ok(Self { events })
    }
}

/// The event names a function may publish: its `events.publish` list.
pub fn function_publishes(function_manifest: &Value) -> Result<BTreeSet<String>, String> {
    let Some(publish) = function_manifest
        .get("events")
        .and_then(|e| e.get("publish"))
    else {
        return Ok(BTreeSet::new());
    };
    let list = publish
        .as_array()
        .ok_or("`events.publish` must be an array of event names")?;
    let mut out = BTreeSet::new();
    for item in list {
        let name = item
            .as_str()
            .ok_or("`events.publish` entries must be strings")?;
        if !valid_event_name(name) {
            return Err(format!("`events.publish`: `{name}` {NAME_RULE}"));
        }
        out.insert(name.to_owned());
    }
    Ok(out)
}

/// The qualified `<app>/<event>` types a function listens to, as written in
/// its `on` key (a string or an array of strings), in order.
pub fn function_subscribes(function_manifest: &Value) -> Result<Vec<String>, String> {
    let Some(on) = function_manifest.get("on") else {
        return Ok(Vec::new());
    };
    let items: Vec<&Value> = match on {
        Value::String(_) => vec![on],
        Value::Array(a) => a.iter().collect(),
        _ => return Err("`on` must be a string or an array of strings".into()),
    };
    items
        .into_iter()
        .map(|item| {
            let s = item.as_str().ok_or("`on` entries must be strings")?;
            if valid_qualified(s) {
                Ok(s.to_owned())
            } else {
                Err(format!(
                    "`on`: `{s}` must be `<app>/<event>`, an app slug (lowercase letters, digits, hyphens) and an event name ({NAME_RULE})"
                ))
            }
        })
        .collect()
}

fn valid_qualified(s: &str) -> bool {
    match s.split_once('/') {
        Some((slug, name)) => {
            !slug.is_empty()
                && slug
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && valid_event_name(name)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_reads_declared_events() {
        let m = json!({ "events": { "statement.uploaded": {
            "description": "A statement landed.",
            "schema": { "type": "object" }
        }}});
        let c = EventCatalog::from_manifest(Some(&m)).unwrap();
        let d = &c.events["statement.uploaded"];
        assert_eq!(d.description.as_deref(), Some("A statement landed."));
        assert_eq!(d.schema, json!({ "type": "object" }));
        assert!(EventCatalog::from_manifest(None).unwrap().events.is_empty());
        assert!(
            EventCatalog::from_manifest(Some(&json!({})))
                .unwrap()
                .events
                .is_empty()
        );
    }

    #[test]
    fn catalog_refuses_a_bad_name() {
        let m = json!({ "events": { "Statement.Uploaded": { "schema": {} } } });
        let err = EventCatalog::from_manifest(Some(&m)).unwrap_err();
        assert!(err.contains("lowercase"), "{err}");
    }

    #[test]
    fn catalog_refuses_an_unembedded_schema() {
        let m = json!({ "events": { "x": { "schema": "events/x.schema.json" } } });
        assert_eq!(
            EventCatalog::from_manifest(Some(&m)).unwrap_err(),
            "event `x`: schema is a file path; publish with oxyc, which embeds it"
        );
    }

    #[test]
    fn on_accepts_a_string_or_an_array() {
        assert_eq!(
            function_subscribes(&json!({ "on": "locations/statement.uploaded" })).unwrap(),
            vec!["locations/statement.uploaded"]
        );
        assert_eq!(
            function_subscribes(&json!({ "on": ["a-b/x.y", "c/z"] })).unwrap(),
            vec!["a-b/x.y", "c/z"]
        );
        assert!(function_subscribes(&json!({})).unwrap().is_empty());
    }

    #[test]
    fn on_refuses_an_unqualified_type() {
        let err = function_subscribes(&json!({ "on": "statement.uploaded" })).unwrap_err();
        assert!(err.contains("<app>/<event>"), "{err}");
        assert!(function_subscribes(&json!({ "on": ["Loc/x"] })).is_err());
        assert!(function_subscribes(&json!({ "on": ["loc/a-b"] })).is_err());
        assert!(function_subscribes(&json!({ "on": ["loc/1a"] })).is_err());
    }

    #[test]
    fn publishes_lists_names() {
        let m = json!({ "events": { "publish": ["b.x", "a.y", "b.x"] } });
        let got: Vec<_> = function_publishes(&m).unwrap().into_iter().collect();
        assert_eq!(got, vec!["a.y", "b.x"]);
        assert!(function_publishes(&json!({})).unwrap().is_empty());
        assert!(function_publishes(&json!({ "events": { "publish": ["Bad"] } })).is_err());
    }
}
