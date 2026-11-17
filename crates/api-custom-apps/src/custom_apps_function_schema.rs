//! A function's `inputSchema` and `outputSchema` in `oxy-app.json`: JSON Schema
//! for the body it is called with and the value it returns.
//!
//! **A documented subset of draft 2020-12**, so that every reader of a schema —
//! this publish gate, `oxyc validate`, the console's Run now check — agrees on
//! what one means without a full schema engine in each:
//!
//! | Kind        | Keywords |
//! | ----------- | -------- |
//! | annotations | `$schema` (root only, the 2020-12 dialect), `$comment`, `title`, `description`, `default`, `examples`, `deprecated`, `format` |
//! | any type    | `type` (a name or a list of names), `enum`, `const`, `anyOf`, `oneOf` |
//! | object      | `properties`, `required`, `additionalProperties` (a boolean or a schema) |
//! | array       | `items` (one schema), `minItems`, `maxItems`, `uniqueItems` |
//! | string      | `minLength`, `maxLength`, `pattern` (ECMA-262) |
//! | number      | `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf` |
//!
//! Anything else — `$ref` / `$defs`, `allOf`, `not`, `if`/`then`, tuples,
//! `patternProperties` — refuses the publish by name rather than being ignored:
//! a keyword a reader skips is a constraint the author believes holds and none
//! enforces. `format` is an annotation, as 2020-12 makes it by default; nothing
//! asserts it. `pattern` is checked to be text here and compiled by the console,
//! whose regex dialect (ECMA-262) is the one the schema names.
//!
//! **Advisory at run time.** The host does not validate a call against these;
//! the console checks Run now's input before sending it, and `oxyc publish`
//! generates them from a function's TypeScript types when the manifest has none.
//!
//! Mirrored by `sdk/typescript/src/custom-app/function-schema.ts`, which says
//! the same thing first, at `oxyc validate` and in the Vite plugin.

use serde_json::Value;

use crate::custom_apps_validate::BundleValidation;

/// Largest schema one function may declare in one direction, serialized. The
/// schema is copied into every `app_functions` row of every build and sent to
/// the console with the function list, so it is bounded like any manifest
/// field that is stored per build; 16 KiB holds a request body with a hundred
/// documented fields.
pub const MAX_SCHEMA_BYTES: usize = 16 * 1024;

/// Deepest nesting of subschemas. The subset has no `$ref`, so a schema is a
/// finite tree; this bounds the walk over it, here and in the console.
pub const MAX_SCHEMA_DEPTH: usize = 12;

/// The dialect a `$schema` may name: the subset is of 2020-12 and nothing else.
pub const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The two manifest keys, each with the word a message uses for it.
pub const SCHEMA_KEYS: [(&str, &str); 2] = [("inputSchema", "input"), ("outputSchema", "output")];

const TYPES: [&str; 7] = [
    "object", "array", "string", "number", "integer", "boolean", "null",
];

/// Every keyword the subset reads, for the message that refuses another.
const KEYWORDS: &str = "$schema, $comment, title, description, default, examples, deprecated, \
     format, type, enum, const, anyOf, oneOf, properties, required, additionalProperties, \
     items, minItems, maxItems, uniqueItems, minLength, maxLength, pattern, minimum, maximum, \
     exclusiveMinimum, exclusiveMaximum, multipleOf";

/// What is wrong with one schema: where (a JSON-pointer-like path inside it,
/// `""` for its root) and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaProblem {
    pub path: String,
    pub message: String,
}

impl SchemaProblem {
    fn at(path: &str, message: impl Into<String>) -> Self {
        Self {
            path: path.to_string(),
            message: message.into(),
        }
    }
}

/// A function's declared schema in one direction, as the console reads it:
/// the value when it is a JSON object, else `None`. Publish refuses anything
/// else, so a stored build only ever holds an object or nothing; this is the
/// read for a row written before the gate existed.
pub fn declared(spec: Option<&Value>, key: &str) -> Option<Value> {
    spec?.get(key).filter(|v| v.is_object()).cloned()
}

/// Refuse a manifest whose functions declare a schema the subset does not
/// cover, or one over [`MAX_SCHEMA_BYTES`]. The first problem only, like the
/// other bundle checks, naming the function, the key and the place in it.
pub fn check_functions(specs: &[(String, Value)]) -> Result<(), BundleValidation> {
    for (name, spec) in specs {
        for (key, word) in SCHEMA_KEYS {
            let Some(schema) = spec.get(key).filter(|v| !v.is_null()) else {
                continue;
            };
            let size = serde_json::to_vec(schema).map(|b| b.len()).unwrap_or(0);
            if size > MAX_SCHEMA_BYTES {
                return Err(BundleValidation::new(
                    "function_schema_too_large",
                    format!(
                        "Function {name:?} declares an {key} of {size} bytes; a function's {word} \
                         schema may be at most {MAX_SCHEMA_BYTES} bytes."
                    ),
                    format!(
                        "Trim functions.{name}.{key}: drop long descriptions and examples, or \
                         describe only the fields a caller sets."
                    ),
                ));
            }
            if let Err(problem) = check_schema(schema) {
                let at = if problem.path.is_empty() {
                    format!("functions.{name}.{key}")
                } else {
                    format!("functions.{name}.{key} at {}", problem.path)
                };
                return Err(BundleValidation::new(
                    "function_schema_invalid",
                    format!("{at}: {}", problem.message),
                    "Fix the schema, or delete it and let `oxyc publish` generate it from the \
                     function's TypeScript types. Supported: a subset of JSON Schema 2020-12 \
                     (see the oxy-app.json reference).",
                ));
            }
        }
    }
    Ok(())
}

/// Whether `schema` is a schema in the supported subset, within
/// [`MAX_SCHEMA_DEPTH`]. Size is the caller's ([`check_functions`]).
pub fn check_schema(schema: &Value) -> Result<(), SchemaProblem> {
    walk(schema, "", 0)
}

fn walk(schema: &Value, path: &str, depth: usize) -> Result<(), SchemaProblem> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(SchemaProblem::at(
            path,
            format!("nested deeper than {MAX_SCHEMA_DEPTH} levels"),
        ));
    }
    let Some(obj) = schema.as_object() else {
        return Err(SchemaProblem::at(
            path,
            "must be a JSON Schema object ({ \"type\": ... })",
        ));
    };
    for (key, value) in obj {
        let here = format!("{path}/{key}");
        keyword(key, value, &here, path.is_empty(), depth)?;
    }
    Ok(())
}

fn keyword(
    key: &str,
    value: &Value,
    here: &str,
    root: bool,
    depth: usize,
) -> Result<(), SchemaProblem> {
    match key {
        "$schema" => {
            if !root {
                return Err(SchemaProblem::at(
                    here,
                    "`$schema` belongs on the root only",
                ));
            }
            if value.as_str() != Some(DIALECT) {
                return Err(SchemaProblem::at(
                    here,
                    format!("must be {DIALECT:?} — the supported subset is of draft 2020-12"),
                ));
            }
        }
        "$comment" | "title" | "description" | "format" => text(value, here)?,
        "default" | "const" => {}
        "examples" => {
            if !value.is_array() {
                return Err(SchemaProblem::at(here, "must be a list of example values"));
            }
        }
        "deprecated" | "uniqueItems" => {
            if !value.is_boolean() {
                return Err(SchemaProblem::at(here, "must be true or false"));
            }
        }
        "type" => type_keyword(value, here)?,
        "enum" => match value.as_array() {
            Some(values) if !values.is_empty() => {}
            _ => {
                return Err(SchemaProblem::at(
                    here,
                    "must be a non-empty list of values",
                ));
            }
        },
        "anyOf" | "oneOf" => match value.as_array() {
            Some(branches) if !branches.is_empty() => {
                for (i, branch) in branches.iter().enumerate() {
                    walk(branch, &format!("{here}/{i}"), depth + 1)?;
                }
            }
            _ => {
                return Err(SchemaProblem::at(
                    here,
                    "must be a non-empty list of schemas",
                ));
            }
        },
        "properties" => {
            let Some(props) = value.as_object() else {
                return Err(SchemaProblem::at(
                    here,
                    "must be an object mapping each field to its schema",
                ));
            };
            for (name, sub) in props {
                walk(sub, &format!("{here}/{name}"), depth + 1)?;
            }
        }
        "required" => {
            let Some(names) = value.as_array() else {
                return Err(SchemaProblem::at(here, "must be a list of field names"));
            };
            let mut seen = std::collections::HashSet::new();
            for name in names {
                let Some(name) = name.as_str() else {
                    return Err(SchemaProblem::at(here, "must list field names as strings"));
                };
                if !seen.insert(name) {
                    return Err(SchemaProblem::at(here, format!("lists {name:?} twice")));
                }
            }
        }
        "additionalProperties" => {
            if !value.is_boolean() {
                walk(value, here, depth + 1)?;
            }
        }
        "items" => {
            if value.is_array() {
                return Err(SchemaProblem::at(
                    here,
                    "must be one schema for every item; tuples (`prefixItems`) are not supported",
                ));
            }
            walk(value, here, depth + 1)?;
        }
        "minItems" | "maxItems" | "minLength" | "maxLength" => {
            if value.as_u64().is_none() {
                return Err(SchemaProblem::at(here, "must be a whole number, 0 or more"));
            }
        }
        "pattern" => text(value, here)?,
        "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
            if !value.is_number() {
                return Err(SchemaProblem::at(here, "must be a number"));
            }
        }
        "multipleOf" => {
            if !value.as_f64().is_some_and(|n| n > 0.0) {
                return Err(SchemaProblem::at(here, "must be a number greater than 0"));
            }
        }
        "$ref" | "$defs" | "definitions" | "$dynamicRef" | "$anchor" => {
            return Err(SchemaProblem::at(
                here,
                format!(
                    "`{key}` is not supported: write the referenced schema inline (the subset \
                     has no references)"
                ),
            ));
        }
        "nullable" => {
            return Err(SchemaProblem::at(
                here,
                "`nullable` is OpenAPI, not JSON Schema: add \"null\" to `type` instead",
            ));
        }
        other => {
            return Err(SchemaProblem::at(
                here,
                format!("`{other}` is not in the supported subset ({KEYWORDS})"),
            ));
        }
    }
    Ok(())
}

fn text(value: &Value, here: &str) -> Result<(), SchemaProblem> {
    if value.is_string() {
        Ok(())
    } else {
        Err(SchemaProblem::at(here, "must be text"))
    }
}

fn type_keyword(value: &Value, here: &str) -> Result<(), SchemaProblem> {
    let known = |name: &str| TYPES.contains(&name);
    let unknown = |name: &str| {
        SchemaProblem::at(
            here,
            format!(
                "{name:?} is not a JSON Schema type (one of {})",
                TYPES.join(", ")
            ),
        )
    };
    match value {
        Value::String(name) if known(name) => Ok(()),
        Value::String(name) => Err(unknown(name)),
        Value::Array(names) if !names.is_empty() => {
            let mut seen = std::collections::HashSet::new();
            for name in names {
                let Some(name) = name.as_str() else {
                    return Err(SchemaProblem::at(here, "must list type names as strings"));
                };
                if !known(name) {
                    return Err(unknown(name));
                }
                if !seen.insert(name) {
                    return Err(SchemaProblem::at(here, format!("lists {name:?} twice")));
                }
            }
            Ok(())
        }
        _ => Err(SchemaProblem::at(
            here,
            "must be a type name or a non-empty list of them",
        )),
    }
}

#[cfg(test)]
#[path = "custom_apps_function_schema_tests.rs"]
mod tests;
