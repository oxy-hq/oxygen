//! Make a JSON Schema acceptable to Anthropic's strict mode.
//!
//! Strict tools (`strict: true`) and structured outputs (`output_config.format`)
//! accept only a subset of JSON Schema, and the API rejects the whole request
//! with a 400 when a schema steps outside it. The documented limits
//! (platform.claude.com, "Structured outputs" → "JSON Schema limitations"):
//!
//! - numerical constraints (`minimum`, `maximum`, `multipleOf`, …) are not supported;
//! - string constraints (`minLength`, `maxLength`) are not supported;
//! - array `minItems` is supported only with the values 0 and 1, and no other
//!   array constraint is supported.
//!
//! A schema written for validation (the legacy specify schema's
//! `join_path` is `minItems: 3, maxItems: 3`) therefore 400s every request that
//! carries it. Rather than make each schema author remember the subset, the
//! provider rewrites the schema it sends. This follows what the official SDKs
//! do: each unsupported constraint is removed and restated in the member's
//! `description`, so the model still sees the intent. Nothing validates the
//! constraint on the response afterwards; callers that need it must check it.

use serde_json::{Map, Value};

/// Keywords the strict subset rejects outright.
const DROPPED: &[&str] = &[
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "maxItems",
    "uniqueItems",
    "minContains",
    "maxContains",
    "minProperties",
    "maxProperties",
];

/// Keys whose value is a single sub-schema.
const SCHEMA_KEYS: &[&str] = &["items", "additionalProperties", "not", "contains"];
/// Keys whose value is an array of sub-schemas.
const SCHEMA_ARRAY_KEYS: &[&str] = &["anyOf", "allOf", "oneOf", "prefixItems"];
/// Keys whose value maps names to sub-schemas.
const SCHEMA_MAP_KEYS: &[&str] = &["properties", "$defs", "definitions"];

/// Return a copy of `schema` with every constraint outside Anthropic's
/// strict subset removed, and restated in the owning member's description.
pub(crate) fn sanitize_for_strict(schema: &Value) -> Value {
    let mut out = schema.clone();
    sanitize_in_place(&mut out);
    out
}

fn sanitize_in_place(schema: &mut Value) {
    let Value::Object(obj) = schema else { return };
    let removed = strip_constraints(obj);
    if !removed.is_empty() {
        restate_in_description(obj, &removed);
    }
    recurse_into_subschemas(obj);
}

/// Remove unsupported keywords from one schema node; return what was removed.
fn strip_constraints(obj: &mut Map<String, Value>) -> Map<String, Value> {
    let mut removed = Map::new();
    for key in DROPPED {
        if let Some(v) = obj.remove(*key) {
            removed.insert((*key).to_string(), v);
        }
    }
    // minItems is allowed only as 0 or 1. A larger floor becomes 1, which is
    // still true of every valid value, and the original is restated.
    if let Some(min) = obj.get("minItems").and_then(Value::as_u64) {
        if min > 1 {
            removed.insert("minItems".to_string(), obj["minItems"].clone());
            obj.insert("minItems".to_string(), Value::from(1));
        }
    }
    removed
}

fn restate_in_description(obj: &mut Map<String, Value>, removed: &Map<String, Value>) {
    // Keys sorted by hand: `Map`'s own order depends on whether serde_json's
    // `preserve_order` feature is unified in, and the note must not.
    let mut keys: Vec<&String> = removed.keys().collect();
    keys.sort();
    let fields: Vec<String> = keys
        .into_iter()
        .map(|k| format!("{}:{}", Value::String(k.clone()), removed[k]))
        .collect();
    let note = format!("{{{}}}", fields.join(","));
    let text = match obj.get("description").and_then(Value::as_str) {
        Some(d) if !d.is_empty() => format!("{d} {note}"),
        _ => note,
    };
    obj.insert("description".to_string(), Value::String(text));
}

fn recurse_into_subschemas(obj: &mut Map<String, Value>) {
    for key in SCHEMA_KEYS {
        if let Some(sub) = obj.get_mut(*key) {
            match sub {
                // `items` may also be a tuple form (array of schemas).
                Value::Array(list) => list.iter_mut().for_each(sanitize_in_place),
                other => sanitize_in_place(other),
            }
        }
    }
    for key in SCHEMA_ARRAY_KEYS {
        if let Some(Value::Array(list)) = obj.get_mut(*key) {
            list.iter_mut().for_each(sanitize_in_place);
        }
    }
    for key in SCHEMA_MAP_KEYS {
        if let Some(Value::Object(map)) = obj.get_mut(*key) {
            map.values_mut().for_each(sanitize_in_place);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn min_items_above_one_becomes_one_and_max_items_is_dropped() {
        // The legacy specify schema's join_path, which 400'd every request.
        let schema = json!({
            "type": "array",
            "items": {"type": "string"},
            "minItems": 3,
            "maxItems": 3
        });
        let out = sanitize_for_strict(&schema);
        assert_eq!(out["minItems"], 1);
        assert!(out.get("maxItems").is_none());
        assert_eq!(out["description"], r#"{"maxItems":3,"minItems":3}"#);
    }

    #[test]
    fn min_items_zero_or_one_is_kept_untouched() {
        for n in [0, 1] {
            let schema = json!({"type": "array", "items": {"type": "string"}, "minItems": n});
            assert_eq!(sanitize_for_strict(&schema), schema, "minItems {n}");
        }
    }

    #[test]
    fn numeric_and_string_constraints_are_restated_in_the_description() {
        let schema = json!({
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "description": "Row cap.", "minimum": 1, "maximum": 100},
                "name": {"type": "string", "minLength": 2, "pattern": "^[a-z]+$"}
            },
            "required": ["limit", "name"],
            "additionalProperties": false
        });
        let out = sanitize_for_strict(&schema);
        let limit = &out["properties"]["limit"];
        assert!(limit.get("minimum").is_none() && limit.get("maximum").is_none());
        assert_eq!(
            limit["description"],
            r#"Row cap. {"maximum":100,"minimum":1}"#
        );
        let name = &out["properties"]["name"];
        assert!(name.get("minLength").is_none());
        // `pattern` is in the supported subset and stays.
        assert_eq!(name["pattern"], "^[a-z]+$");
        assert_eq!(out["required"], json!(["limit", "name"]));
        assert_eq!(out["additionalProperties"], false);
    }

    #[test]
    fn nested_schemas_are_reached_through_items_any_of_and_defs() {
        let schema = json!({
            "type": "object",
            "properties": {
                "joins": {
                    "type": "array",
                    "items": {"type": "array", "items": {"type": "string"}, "minItems": 3, "maxItems": 3}
                },
                "either": {"anyOf": [{"type": "string", "maxLength": 9}, {"type": "null"}]},
                "ref": {"$ref": "#/$defs/n"}
            },
            "$defs": {"n": {"type": "number", "multipleOf": 5}}
        });
        let out = sanitize_for_strict(&schema);
        assert_eq!(out["properties"]["joins"]["items"]["minItems"], 1);
        assert!(
            out["properties"]["either"]["anyOf"][0]
                .get("maxLength")
                .is_none()
        );
        assert!(out["$defs"]["n"].get("multipleOf").is_none());
    }

    #[test]
    fn a_property_named_like_a_keyword_is_not_mistaken_for_one() {
        // `properties` maps NAMES to schemas; a field called "minimum" is data.
        let schema = json!({
            "type": "object",
            "properties": {"minimum": {"type": "number"}, "maxItems": {"type": "integer"}},
            "required": ["minimum", "maxItems"],
            "additionalProperties": false
        });
        assert_eq!(sanitize_for_strict(&schema), schema);
    }

    #[test]
    fn a_schema_already_in_the_subset_is_returned_unchanged() {
        let schema = json!({
            "type": "object",
            "properties": {"kind": {"type": "string", "enum": ["a", "b"]}},
            "required": ["kind"],
            "additionalProperties": false
        });
        assert_eq!(sanitize_for_strict(&schema), schema);
    }
}
