//! The `preview` notes a run's step results carry, read back for the run
//! report. A step's result carries its note wherever the step ran — nested in
//! a loop's iterations or a conditional's arm included — so notes are found by
//! walking the result:
//!
//! * `{"held": true, …}` — a write that was not sent ([`HeldNote`]);
//! * `{"rewritten": true, …}` — a managed-Airhouse step sent into the
//!   preview's own schemas, with every redirect listed ([`RedirectNote`]).

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct HeldNote {
    pub verb: String,
    pub targets: Vec<String>,
    pub reason: String,
    pub sql: Option<String>,
}

/// What a step's managed-Airhouse SQL did in the preview: the tables it wrote
/// and read there instead of live, and the live tables it copied first.
#[derive(Debug, Serialize, Clone, PartialEq, Eq, Default)]
pub struct RedirectNote {
    pub writes: Vec<Redirect>,
    pub reads: Vec<Redirect>,
    pub copies: Vec<CopyNote>,
}

/// `live` (`schema.table`) went to `preview` (`preview_<key>__schema.table`).
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub live: String,
    pub preview: String,
}

/// A copy-on-write of `live`: `shadow` (whole) or `partial` (over the cap, so
/// the copy started empty and holds only what the preview wrote).
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct CopyNote {
    pub live: String,
    pub state: String,
}

/// Every `preview` note in `value` for which `wanted` holds, with the `sql`
/// beside it, depth first.
fn notes<'a>(value: &'a Value, wanted: &str, out: &mut Vec<(&'a Value, Option<&'a str>)>) {
    match value {
        Value::Object(map) => {
            if let Some(note) = map.get("preview").filter(|n| n[wanted] == true) {
                out.push((note, map.get("sql").and_then(Value::as_str)));
            }
            for (key, v) in map {
                if key != "preview" {
                    notes(v, wanted, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| notes(v, wanted, out)),
        _ => {}
    }
}

/// A note in the contract's shape. SQL holds carry `verb`/`targets`; an HTTP
/// hold carries `method`/`url`, reported as the verb and the one target.
fn held_of(note: &Value, sql: Option<&str>) -> HeldNote {
    let text = |k: &str| note.get(k).and_then(Value::as_str).map(str::to_string);
    let targets = note
        .get("targets")
        .and_then(Value::as_array)
        .map(|t| {
            t.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .or_else(|| text("url").map(|u| vec![u]))
        .unwrap_or_default();
    HeldNote {
        verb: text("verb").or_else(|| text("method")).unwrap_or_default(),
        targets,
        reason: text("reason").unwrap_or_default(),
        sql: sql.map(str::to_string),
    }
}

pub(super) fn first_held(value: &Value) -> Option<HeldNote> {
    let mut out = vec![];
    notes(value, "held", &mut out);
    out.first().map(|(note, sql)| held_of(note, *sql))
}

pub(super) fn count_held(results: &Value) -> usize {
    let mut out = vec![];
    notes(results, "held", &mut out);
    out.len()
}

/// Every redirect of every rewritten note in `value`, merged, each once.
/// `None` when the step sent nothing into the preview.
pub(super) fn redirects(value: &Value) -> Option<RedirectNote> {
    let mut out = vec![];
    notes(value, "rewritten", &mut out);
    if out.is_empty() {
        return None;
    }
    let mut merged = RedirectNote::default();
    for (note, _) in out {
        extend(&mut merged.writes, note, "writes", redirect_of);
        extend(&mut merged.reads, note, "reads", redirect_of);
        extend(&mut merged.copies, note, "copies", copy_of);
    }
    Some(merged)
}

fn extend<T: PartialEq>(into: &mut Vec<T>, note: &Value, key: &str, read: fn(&Value) -> T) {
    for item in note
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let item = read(item);
        if !into.contains(&item) {
            into.push(item);
        }
    }
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn redirect_of(v: &Value) -> Redirect {
    Redirect {
        live: text(v, "live"),
        preview: text(v, "preview"),
    }
}

fn copy_of(v: &Value) -> CopyNote {
    CopyNote {
        live: text(v, "live"),
        state: text(v, "state"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redirects_are_merged_across_iterations_and_a_held_step_has_none() {
        let note = |table: &str| {
            json!({ "sql": "…", "preview": {
                "rewritten": true,
                "writes": [{ "live": format!("pos.{table}"), "preview": format!("p__pos.{table}") }],
                "reads": [{ "live": "pos.orders", "preview": "p__pos.orders" }],
                "copies": [{ "live": "pos.orders", "state": "partial" }],
            }})
        };
        let looped = json!([note("a"), note("b"), note("a")]);
        let merged = redirects(&looped).expect("rewritten");
        assert_eq!(
            merged
                .writes
                .iter()
                .map(|r| r.live.as_str())
                .collect::<Vec<_>>(),
            vec!["pos.a", "pos.b"]
        );
        assert_eq!(merged.reads.len(), 1);
        assert_eq!(merged.copies[0].state, "partial");
        assert_eq!(count_held(&looped), 0, "a rewritten step is not held");

        let held = json!({ "preview": { "held": true, "verb": "INSERT", "targets": [] } });
        assert!(redirects(&held).is_none());
        assert_eq!(count_held(&held), 1);
    }
}
