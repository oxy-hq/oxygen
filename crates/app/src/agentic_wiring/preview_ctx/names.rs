//! D5: an automation served inside a preview run names every side effect as
//! `preview:<run_id>:<name>`.
//!
//! The names that select a side effect: each `execute_sql` `database`, each
//! sub-automation `src`, each `agent` `agent_ref` (the built-in builder
//! included — scoped, it resolves to nothing, so a builder step fails rather
//! than editing files), and each `airway` `pipeline`. Nested task lists —
//! loop bodies, both arms of a conditional — are walked too.
//!
//! This platform strips the prefix again for its own run. What the scoping buys
//! is the other pod: one without preview code, handed this run's tasks after a
//! crash or a rollback, finds no automation, database, agent or pipeline under
//! any of these names, so the run fails instead of running as production.
//! `semantic_query` steps carry no database name; they read.
//!
//! An `http_request` names no resource, so its **method** is the fence: every
//! method but `GET`/`HEAD` (and any request with `persist_to_secret`, which
//! writes whatever its verb) becomes `preview:<run_id>:<METHOD>` — `POST` when
//! absent, the executor's default. That is not an HTTP token, so an old pod's
//! request builder refuses it before anything is sent. Its `persist_to_secret`
//! name is scoped too; `:` is not a character the secret store accepts in a
//! name, so even a pod that got that far could not write it.

use serde_json::Value;

use agentic_automation::preview_names::{is_scoped, parse_scoped_method, scoped};

/// `(task type, key)` pairs whose value is a side-effecting name.
const SCOPED_KEYS: &[(&str, &str)] = &[
    ("execute_sql", "database"),
    ("workflow", "src"),
    ("agent", "agent_ref"),
    ("airway", "pipeline"),
];

/// Scope every side-effecting name in an automation definition, in place.
pub(super) fn scope_automation(definition: &mut Value, run_id: &str) {
    if let Some(tasks) = definition.get_mut("tasks") {
        scope_tasks(tasks, run_id);
    }
}

fn scope_tasks(tasks: &mut Value, run_id: &str) {
    let Some(tasks) = tasks.as_array_mut() else {
        return;
    };
    for task in tasks {
        scope_task(task, run_id);
    }
}

fn scope_task(task: &mut Value, run_id: &str) {
    let kind = task
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    for (task_type, key) in SCOPED_KEYS {
        if kind == *task_type
            && let Some(Value::String(name)) = task.get_mut(*key)
            && !is_scoped(name)
        {
            *name = scoped(run_id, name);
        }
    }
    if kind == "http_request" {
        scope_http_request(task, run_id);
    }
    // Nested bodies: a loop's `tasks`, a conditional's branches and `else`,
    // and a sub-automation's pre-resolved tasks when a definition carries them.
    for key in ["tasks", "else", "resolved_tasks"] {
        if let Some(nested) = task.get_mut(key) {
            scope_tasks(nested, run_id);
        }
    }
    if let Some(Value::Array(branches)) = task.get_mut("conditions") {
        for branch in branches {
            if let Some(nested) = branch.get_mut("tasks") {
                scope_tasks(nested, run_id);
            }
        }
    }
}

/// Scope a writing request's method, and its `persist_to_secret` name.
fn scope_http_request(task: &mut Value, run_id: &str) {
    let Some(obj) = task.as_object_mut() else {
        return;
    };
    let persists = obj.contains_key("persist_to_secret");
    let method = obj
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("POST")
        .to_ascii_uppercase();
    let reads = matches!(method.as_str(), "GET" | "HEAD");
    if parse_scoped_method(&method).is_none() && (!reads || persists) {
        obj.insert("method".into(), Value::String(scoped(run_id, &method)));
    }
    if let Some(Value::String(name)) = obj
        .get_mut("persist_to_secret")
        .and_then(|p| p.get_mut("name"))
        && !is_scoped(name)
    {
        *name = scoped(run_id, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_side_effecting_name_is_scoped_at_any_depth() {
        let mut def = json!({
            "name": "je",
            "tasks": [
                { "name": "w", "type": "execute_sql", "database": "clickhouse", "sql_query": "INSERT 1" },
                { "name": "s", "type": "semantic_query", "topic": "orders" },
                { "name": "a", "type": "agent", "agent_ref": "agents/x.agentic.yml", "prompt": "p" },
                { "name": "b", "type": "agent", "agent_ref": "__builder__", "prompt": "p" },
                { "name": "p", "type": "airway", "pipeline": "airway/toast.airway.yml" },
                { "name": "loop", "type": "loop_sequential", "values": [1], "tasks": [
                    { "name": "c", "type": "workflow", "src": "workflows/child.procedure.yml" }
                ]},
                { "name": "cond", "type": "conditional",
                  "conditions": [{ "if": "true", "tasks": [
                      { "name": "d", "type": "execute_sql", "database": "airhouse", "sql_query": "DELETE" }
                  ]}],
                  "else": [{ "name": "e", "type": "airway", "pipeline": "airway/qb.airway.yml" }] }
            ]
        });
        scope_automation(&mut def, "run1");
        let t = &def["tasks"];
        assert_eq!(t[0]["database"], "preview:run1:clickhouse");
        assert!(
            t[1].get("database").is_none(),
            "a semantic query names no database"
        );
        assert_eq!(t[2]["agent_ref"], "preview:run1:agents/x.agentic.yml");
        assert_eq!(t[3]["agent_ref"], "preview:run1:__builder__");
        assert_eq!(t[4]["pipeline"], "preview:run1:airway/toast.airway.yml");
        assert_eq!(
            t[5]["tasks"][0]["src"],
            "preview:run1:workflows/child.procedure.yml"
        );
        assert_eq!(
            t[6]["conditions"][0]["tasks"][0]["database"],
            "preview:run1:airhouse"
        );
        assert_eq!(
            t[6]["else"][0]["pipeline"],
            "preview:run1:airway/qb.airway.yml"
        );
        assert_eq!(t[0]["name"], "w", "step names are not side effects");
    }

    #[test]
    fn scoping_is_idempotent() {
        let mut def = json!({ "tasks": [{ "type": "execute_sql", "database": "ch" }] });
        scope_automation(&mut def, "r");
        scope_automation(&mut def, "r");
        assert_eq!(def["tasks"][0]["database"], "preview:r:ch");
    }

    /// Every writing request's method is scoped — POST when absent — and a
    /// read is left alone unless it persists a secret.
    #[test]
    fn writing_http_requests_have_their_method_scoped() {
        let mut def = json!({ "tasks": [
            { "name": "a", "type": "http_request", "url": "https://x" },
            { "name": "b", "type": "http_request", "method": "put", "url": "https://x" },
            { "name": "c", "type": "http_request", "method": "GET", "url": "https://x" },
            { "name": "d", "type": "http_request", "method": "head", "url": "https://x" },
            { "name": "e", "type": "http_request", "method": "GET", "url": "https://x",
              "persist_to_secret": { "from": "/t", "name": "QB_REFRESH_TOKEN" } },
        ]});
        scope_automation(&mut def, "run1");
        scope_automation(&mut def, "run1");
        let t = &def["tasks"];
        assert_eq!(
            t[0]["method"], "preview:run1:POST",
            "the executor's default is POST"
        );
        assert_eq!(t[1]["method"], "preview:run1:PUT");
        assert_eq!(t[2]["method"], "GET");
        assert_eq!(t[3]["method"], "head");
        assert_eq!(
            t[4]["method"], "preview:run1:GET",
            "persisting a secret is a write"
        );
        assert_eq!(
            t[4]["persist_to_secret"]["name"],
            "preview:run1:QB_REFRESH_TOKEN"
        );
    }
}
