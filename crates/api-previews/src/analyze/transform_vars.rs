//! Variables a transform build would run without. A build is queued with no
//! variables, so a procedure that needs a value only a person can give is
//! `manual` ("needs variables"), not built:
//!
//! * a variable **declared** with no default — `null`, or metadata
//!   (`description` / `type` / `required`) and no `default`:
//!   `agentic_automation::variables` would hand the template the metadata
//!   object itself;
//! * a name a SQL body **references** (`{{ date }}`, `{% if full %}`) that
//!   nothing provides: not a declared variable with a value, not a step's
//!   name (a step's result is in scope by its name, a loop's too), not a key
//!   of a step's own `variables:`, not a name the template binds itself
//!   (`{% for x in … %}`, `{% set x = … %}`), and not a template global.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// The first name in a `{{ … }}` expression, or after `if` / `elif` / `for …
/// in` in a `{% … %}` statement.
static REFERENCED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\{\{-?\s*([A-Za-z_][A-Za-z0-9_]*)|\{%-?\s*(?:if|elif)\s+(?:not\s+)?([A-Za-z_][A-Za-z0-9_]*)|\{%-?\s*for\s+[A-Za-z_][A-Za-z0-9_,\s]*\s+in\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("static regex")
});

/// Names a template binds itself.
static BOUND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{%-?\s*(?:for|set)\s+([A-Za-z_][A-Za-z0-9_]*(?:\s*,\s*[A-Za-z_][A-Za-z0-9_]*)*)")
        .expect("static regex")
});

/// Names the template engine provides.
const GLOBALS: &[&str] = &[
    "loop",
    "true",
    "false",
    "none",
    "True",
    "False",
    "None",
    "range",
    "namespace",
    "self",
    "caller",
    "varargs",
    "kwargs",
];

/// Every variable a build would lack, sorted: declared without a default, or
/// referenced by one of `bodies` with nothing providing it.
pub(super) fn unset_variables(definition: &Value, bodies: &[String]) -> Vec<String> {
    let declared = definition.get("variables").and_then(Value::as_object);
    let mut unset: HashSet<String> = declared
        .into_iter()
        .flatten()
        .filter(|(_, v)| !has_value(v))
        .map(|(name, _)| name.clone())
        .collect();
    let mut provided: HashSet<String> = declared
        .into_iter()
        .flatten()
        .filter(|(_, v)| has_value(v))
        .map(|(name, _)| name.clone())
        .collect();
    step_names(
        definition.get("tasks").unwrap_or(&Value::Null),
        &mut provided,
    );
    provided.extend(GLOBALS.iter().map(|g| g.to_string()));
    for body in bodies {
        let mut bound = provided.clone();
        for binding in BOUND.captures_iter(body) {
            bound.extend(binding[1].split(',').map(|n| n.trim().to_string()));
        }
        for reference in REFERENCED.captures_iter(body) {
            let name = (1..=3).find_map(|i| reference.get(i)).map(|m| m.as_str());
            if let Some(name) = name.filter(|n| !bound.contains(*n)) {
                unset.insert(name.to_string());
            }
        }
    }
    let mut unset: Vec<String> = unset.into_iter().collect();
    unset.sort();
    unset
}

/// A declaration a template can use: a plain value, or one with a `default`.
fn has_value(declared: &Value) -> bool {
    const METADATA: [&str; 3] = ["description", "type", "required"];
    match declared {
        Value::Null => false,
        Value::Object(meta) => {
            meta.contains_key("default") || !METADATA.iter().any(|k| meta.contains_key(*k))
        }
        _ => true,
    }
}

/// Every step's name and every key of a step's `variables:`, at any depth.
fn step_names(tasks: &Value, out: &mut HashSet<String>) {
    for task in tasks.as_array().into_iter().flatten() {
        if let Some(name) = task.get("name").and_then(Value::as_str) {
            out.insert(name.to_string());
        }
        if let Some(vars) = task.get("variables").and_then(Value::as_object) {
            out.extend(vars.keys().cloned());
        }
        for key in ["tasks", "else"] {
            step_names(task.get(key).unwrap_or(&Value::Null), out);
        }
        for branch in task
            .get("conditions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            step_names(branch.get("tasks").unwrap_or(&Value::Null), out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn referenced_names_nothing_provides_are_unset() {
        let def = json!({
            "variables": { "region": "west", "label": { "default": "all" },
                           "date": { "description": "business date" } },
            "tasks": [
                { "name": "prior", "type": "execute_sql", "database": "p", "sql_query": "SELECT 1" },
                { "name": "each", "type": "loop_sequential", "values": [1], "tasks": [
                    { "name": "w", "type": "execute_sql", "database": "p",
                      "variables": { "cutoff": "{{ each.value }}" }, "sql_query": "…" }
                ]}
            ]
        });
        let bodies = vec![
            "DELETE FROM s.t WHERE d = '{{ date }}' AND r = '{{region}}' AND l = '{{ label }}'"
                .to_string(),
            "INSERT INTO s.t SELECT {{ prior.rows }} WHERE c < '{{ cutoff }}' \
             {% if full %}AND 1=1{% endif %} {% for x in each %}{{ x }}{% endfor %} \
             {% set y = 2 %}{{ y }} {{- as_of -}} {{ loop.index }}"
                .to_string(),
        ];
        assert_eq!(
            unset_variables(&def, &bodies),
            vec!["as_of", "date", "full"]
        );
        assert!(unset_variables(&json!({ "tasks": [] }), &["SELECT 1".into()]).is_empty());
    }
}
