use serde_json::{Value, json};

use super::*;

fn specs(spec: Value) -> Vec<(String, Value)> {
    vec![("create-order".to_string(), spec)]
}

/// What `oxyc publish` generates for `export interface Input { … }`, and what a
/// hand-written schema usually looks like: every supported keyword at least once.
fn full_schema() -> Value {
    json!({
        "$schema": DIALECT,
        "$comment": "generated",
        "title": "Input",
        "description": "Create an order.",
        "type": "object",
        "properties": {
            "store": { "type": "integer", "minimum": 1, "maximum": 9999, "description": "Store number." },
            "note": { "type": ["string", "null"], "maxLength": 200, "minLength": 0, "pattern": "^\\S" },
            "kind": { "enum": ["pickup", "delivery"], "default": "pickup" },
            "placed_at": { "type": "string", "format": "date-time", "examples": ["2026-11-17T10:00:00Z"] },
            "lines": {
                "type": "array",
                "minItems": 1,
                "maxItems": 50,
                "uniqueItems": false,
                "items": {
                    "type": "object",
                    "properties": {
                        "sku": { "type": "string" },
                        "qty": { "type": "number", "exclusiveMinimum": 0, "multipleOf": 0.5 }
                    },
                    "required": ["sku", "qty"],
                    "additionalProperties": false
                }
            },
            "tags": { "type": "object", "additionalProperties": { "type": "string" } },
            "version": { "const": 2, "deprecated": true },
            "who": { "anyOf": [{ "type": "string" }, { "type": "integer" }] },
            "when": { "oneOf": [{ "type": "string" }, { "type": "null" }], "exclusiveMaximum": 3 }
        },
        "required": ["store", "lines"],
        "additionalProperties": true
    })
}

#[test]
fn accepts_every_supported_keyword() {
    assert_eq!(check_schema(&full_schema()), Ok(()));
    assert!(check_functions(&specs(json!({ "inputSchema": full_schema() }))).is_ok());
}

#[test]
fn accepts_a_function_without_schemas_and_a_null_one() {
    assert!(check_functions(&specs(json!({ "route": true }))).is_ok());
    assert!(check_functions(&specs(json!({ "inputSchema": null }))).is_ok());
}

#[test]
fn an_empty_schema_accepts_anything_and_passes() {
    assert_eq!(check_schema(&json!({})), Ok(()));
}

#[test]
fn refuses_a_schema_that_is_not_an_object() {
    for bad in [json!("object"), json!(true), json!([]), json!(3)] {
        let err = check_schema(&bad).unwrap_err();
        assert_eq!(err.path, "");
        assert!(err.message.contains("JSON Schema object"), "{err:?}");
    }
}

#[test]
fn refuses_an_unknown_keyword_by_name_and_place() {
    let schema = json!({
        "type": "object",
        "properties": { "store": { "type": "integer", "minimun": 1 } }
    });
    let err = check_schema(&schema).unwrap_err();
    assert_eq!(err.path, "/properties/store/minimun");
    assert!(
        err.message
            .contains("`minimun` is not in the supported subset")
    );
}

#[test]
fn refuses_references_with_their_own_sentence() {
    let schema = json!({ "$defs": { "a": {} }, "$ref": "#/$defs/a" });
    let err = check_schema(&schema).unwrap_err();
    assert!(
        err.message.contains("write the referenced schema inline"),
        "{err:?}"
    );
}

#[test]
fn refuses_openapi_nullable_pointing_at_type() {
    let err = check_schema(&json!({ "type": "string", "nullable": true })).unwrap_err();
    assert!(err.message.contains("add \"null\" to `type`"), "{err:?}");
}

#[test]
fn refuses_unsupported_composition_and_tuples() {
    assert!(check_schema(&json!({ "allOf": [{}] })).is_err());
    assert!(check_schema(&json!({ "not": {} })).is_err());
    assert!(check_schema(&json!({ "if": {}, "then": {} })).is_err());
    assert!(check_schema(&json!({ "patternProperties": {} })).is_err());
    let err = check_schema(&json!({ "type": "array", "items": [{}, {}] })).unwrap_err();
    assert!(err.message.contains("tuples"), "{err:?}");
}

#[test]
fn checks_type_names() {
    let err = check_schema(&json!({ "type": "int" })).unwrap_err();
    assert!(err.message.contains("\"int\" is not a JSON Schema type"));
    assert!(check_schema(&json!({ "type": [] })).is_err());
    assert!(check_schema(&json!({ "type": ["string", "string"] })).is_err());
    assert!(check_schema(&json!({ "type": ["string", 1] })).is_err());
    assert!(check_schema(&json!({ "type": 1 })).is_err());
}

#[test]
fn checks_keyword_values() {
    let bad = [
        json!({ "enum": [] }),
        json!({ "anyOf": [] }),
        json!({ "oneOf": {} }),
        json!({ "properties": [] }),
        json!({ "required": "store" }),
        json!({ "required": ["a", "a"] }),
        json!({ "required": [1] }),
        json!({ "minLength": -1 }),
        json!({ "maxItems": 1.5 }),
        json!({ "minimum": "1" }),
        json!({ "multipleOf": 0 }),
        json!({ "pattern": 1 }),
        json!({ "description": 1 }),
        json!({ "examples": "x" }),
        json!({ "uniqueItems": "yes" }),
        json!({ "additionalProperties": "no" }),
        json!({ "items": 1 }),
    ];
    for schema in bad {
        assert!(check_schema(&schema).is_err(), "accepted {schema}");
    }
}

#[test]
fn schema_dialect_must_be_2020_12_and_root_only() {
    let err =
        check_schema(&json!({ "$schema": "http://json-schema.org/draft-07/schema#" })).unwrap_err();
    assert!(err.message.contains("2020-12"));
    let nested = json!({ "properties": { "a": { "$schema": DIALECT } } });
    assert!(
        check_schema(&nested)
            .unwrap_err()
            .message
            .contains("root only")
    );
}

#[test]
fn caps_nesting_depth() {
    let mut schema = json!({ "type": "string" });
    for _ in 0..=MAX_SCHEMA_DEPTH {
        schema = json!({ "type": "array", "items": schema });
    }
    let err = check_schema(&schema).unwrap_err();
    assert!(err.message.contains("nested deeper"), "{err:?}");

    let mut ok = json!({ "type": "string" });
    for _ in 0..MAX_SCHEMA_DEPTH {
        ok = json!({ "type": "array", "items": ok });
    }
    assert_eq!(check_schema(&ok), Ok(()));
}

#[test]
fn refuses_a_schema_over_the_size_cap_as_its_own_check() {
    let big = json!({ "type": "string", "description": "x".repeat(MAX_SCHEMA_BYTES) });
    let err = check_functions(&specs(json!({ "outputSchema": big }))).unwrap_err();
    assert_eq!(err.check, "function_schema_too_large");
    assert!(err.message.contains("\"create-order\""), "{}", err.message);
    assert!(err.message.contains("outputSchema"), "{}", err.message);

    // At the cap is fine: the cap is inclusive.
    let base = json!({ "type": "string", "description": "" });
    let room = MAX_SCHEMA_BYTES - serde_json::to_vec(&base).unwrap().len();
    let exact = json!({ "type": "string", "description": "x".repeat(room) });
    assert_eq!(serde_json::to_vec(&exact).unwrap().len(), MAX_SCHEMA_BYTES);
    assert!(check_functions(&specs(json!({ "outputSchema": exact }))).is_ok());
}

#[test]
fn names_the_function_key_and_path_in_the_refusal() {
    let spec = json!({
        "inputSchema": { "type": "object" },
        "outputSchema": { "type": "object", "properties": { "id": { "type": "uuid" } } }
    });
    let err = check_functions(&specs(spec)).unwrap_err();
    assert_eq!(err.check, "function_schema_invalid");
    assert!(
        err.message
            .starts_with("functions.create-order.outputSchema at /properties/id/type:"),
        "{}",
        err.message
    );
    // Display is what the 422 body carries.
    assert!(err.to_string().contains("[function_schema_invalid]"));
}

#[test]
fn declared_reads_objects_only() {
    let spec = json!({ "inputSchema": { "type": "object" }, "outputSchema": "nope" });
    assert_eq!(
        declared(Some(&spec), "inputSchema"),
        Some(json!({ "type": "object" }))
    );
    assert_eq!(declared(Some(&spec), "outputSchema"), None);
    assert_eq!(declared(None, "inputSchema"), None);
}

/// The SDK says the same rule first (`oxyc validate`, the Vite plugin); the
/// two are one rule written twice, so a keyword or a cap added to one side
/// fails here until the other has it.
#[test]
fn the_sdk_mirror_reads_the_same_keywords_and_caps() {
    const TS: &str = include_str!("../../../sdk/typescript/src/custom-app/function-schema.ts");
    let block = TS
        .split("export const FUNCTION_SCHEMA_KEYWORDS = [")
        .nth(1)
        .and_then(|rest| rest.split("] as const").next())
        .expect("FUNCTION_SCHEMA_KEYWORDS in function-schema.ts");
    let ts_keywords: Vec<&str> = block
        .split(',')
        .map(|k| k.trim().trim_matches('"'))
        .filter(|k| !k.is_empty())
        .collect();
    let rs_keywords: Vec<&str> = KEYWORDS.split(',').map(str::trim).collect();
    assert_eq!(ts_keywords, rs_keywords);

    assert!(
        TS.contains("FUNCTION_SCHEMA_MAX_BYTES = 16 * 1024;") && MAX_SCHEMA_BYTES == 16 * 1024,
        "the size cap differs between function-schema.ts and MAX_SCHEMA_BYTES"
    );
    assert!(
        TS.contains(&format!("FUNCTION_SCHEMA_MAX_DEPTH = {MAX_SCHEMA_DEPTH};")),
        "the depth cap differs between function-schema.ts and MAX_SCHEMA_DEPTH"
    );
    assert!(TS.contains(&format!("FUNCTION_SCHEMA_DIALECT = {DIALECT:?};")));
}
