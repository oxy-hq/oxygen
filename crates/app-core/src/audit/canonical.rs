//! Put an entry's JSON in the order Postgres will hand it back in.
//!
//! The hash chain digests `metadata`, `before` and `after` as text, once when
//! the row is written and again, from the stored row, when the chain is
//! verified. `jsonb` does not keep key order — it stores an object's keys
//! sorted by **length, then bytes** — and `serde_json` here is built with
//! `preserve_order`, so it reads them back in that order. An object written
//! with keys in any other order therefore hashed one way and verified another:
//! the chain reported a break nobody caused (`{"org_slug", "surface"}` comes
//! back as `{"surface", "org_slug"}`).
//!
//! Sorting on the way in makes the two sides agree by construction. It is the
//! write side that moves, so every row already stored verifies exactly as it
//! did before.

use serde_json::{Map, Value};

/// `v` with every object's keys in `jsonb` storage order, recursively.
pub(super) fn jsonb_order(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| {
                a.len()
                    .cmp(&b.len())
                    .then_with(|| a.as_bytes().cmp(b.as_bytes()))
            });
            let mut out = Map::with_capacity(map.len());
            for key in keys {
                out.insert(key.clone(), jsonb_order(&map[key]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(jsonb_order).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_sort_by_length_then_bytes() {
        let v = json!({ "org_slug": "acme", "surface": "admin", "b": 1, "aa": 2, "a": 3 });
        assert_eq!(
            jsonb_order(&v).to_string(),
            r#"{"a":3,"b":1,"aa":2,"surface":"admin","org_slug":"acme"}"#
        );
    }

    #[test]
    fn nested_objects_and_arrays_are_ordered_too() {
        let v = json!({ "zz": [{ "bb": 1, "a": 2 }], "y": { "dd": 1, "c": 2 } });
        assert_eq!(
            jsonb_order(&v).to_string(),
            r#"{"y":{"c":2,"dd":1},"zz":[{"a":2,"bb":1}]}"#
        );
    }

    #[test]
    fn scalars_and_array_order_are_untouched() {
        for v in [json!(null), json!(1.5), json!("x"), json!([3, 1, 2])] {
            assert_eq!(jsonb_order(&v), v);
        }
    }

    #[test]
    fn ordering_is_idempotent() {
        let v =
            json!({ "token_id": "t", "token_name": "n", "token_kind": "k", "display_prefix": "p" });
        let once = jsonb_order(&v);
        assert_eq!(jsonb_order(&once).to_string(), once.to_string());
        assert_eq!(
            once.to_string(),
            r#"{"token_id":"t","token_kind":"k","token_name":"n","display_prefix":"p"}"#
        );
    }
}
