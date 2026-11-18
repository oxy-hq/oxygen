//! What a build may declare to watch (app events design §7.1), checked at
//! publish: each `on: { table }` entry of each function.
//!
//! The shape is [`oxy_custom_apps::events::table::TableRef::parse`]'s. Two
//! more things a manifest cannot say:
//!
//! * **An environment's copy is never named.** `app_x__staging.lines` is where
//!   staging writes `app_x.lines`; the watch names `app_x.lines` and the
//!   tracker finds the copy (§7.3, "Environments").
//! * **Nothing the app could not read itself** ([`check_readable`]), and only a
//!   function that declares the `airhouse` capability.

use airhouse::app_schema::is_environment_schema;
use oxy_custom_apps::events::table::{TableRef, function_table_watches};
use serde_json::Value;

/// Whether the app's own `ctx.airhouse.query` could read `table`: the same
/// `airhouse::sql_rules` read check, on the `SELECT` the platform would send.
/// DuckLake's internal schemas (`__ducklake_metadata_*`), `information_schema`
/// and any other the rules refuse are therefore refused here too, without a
/// list kept in step by hand. Run at publish and again before every read.
#[inline(never)]
pub fn check_readable(table: &TableRef) -> Result<(), String> {
    let sql = format!(
        "SELECT * FROM \"{}\".\"{}\"",
        table.schema.replace('"', "\"\""),
        table.table.replace('"', "\"\"")
    );
    airhouse::sql_rules::check(&sql, &table.schema, airhouse::sql_rules::Access::Read)
        .map(|_| ())
        .map_err(|e| {
            format!(
                "`{}` cannot be watched: ctx.airhouse.query could not read it ({e})",
                table.spec()
            )
        })
}

fn airhouse_enabled(spec: &Value) -> bool {
    spec.pointer("/airhouse/enabled").and_then(Value::as_bool) == Some(true)
}

/// Every table the manifest's functions watch, as `(function, table)`, or the
/// first problem, naming the function.
#[inline(never)]
pub fn declared_watches(manifest: Option<&Value>) -> Result<Vec<(String, TableRef)>, String> {
    let Some(functions) = manifest
        .and_then(|m| m.get("functions"))
        .and_then(Value::as_object)
    else {
        return Ok(Vec::new());
    };
    let mut watches = Vec::new();
    for (name, spec) in functions {
        let tables = function_table_watches(spec).map_err(|e| format!("function `{name}`: {e}"))?;
        for table in tables {
            if is_environment_schema(&table.schema) {
                return Err(format!(
                    "function `{name}`: `{}` is an environment's copy of a table; watch the table \
                     it copies and the platform reads the environment's copy where there is one",
                    table.spec()
                ));
            }
            check_readable(&table).map_err(|e| format!("function `{name}`: {e}"))?;
            // Rows ride the event or `ctx.tables.changes`: a function that
            // may not read Airhouse may not watch it.
            if !airhouse_enabled(spec) {
                return Err(format!(
                    "function `{name}`: watching `{}` needs the `airhouse` capability; add \
                     \"airhouse\": {{ \"enabled\": true }} to its entry",
                    table.spec()
                ));
            }
            watches.push((name.clone(), table));
        }
    }
    Ok(watches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_manifest_lists_what_its_functions_watch() {
        let manifest = json!({ "functions": {
            "a": { "on": { "table": "pokehouse:bookkeeping.lines" }, "airhouse": { "enabled": true } },
            "b": { "on": ["x/y.z", { "table": "app_loc.visits" }], "airhouse": { "enabled": true } },
            "c": {}
        }});
        let got = declared_watches(Some(&manifest)).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "a");
        assert_eq!(got[1].1.spec(), "app_loc.visits");
        assert!(declared_watches(None).unwrap().is_empty());
    }

    #[test]
    fn a_bad_table_names_the_function() {
        let manifest = json!({ "functions": { "a": { "on": { "table": "lines" }, "airhouse": { "enabled": true } } } });
        let err = declared_watches(Some(&manifest)).unwrap_err();
        assert!(err.starts_with("function `a`:"), "{err}");
    }

    #[test]
    fn an_environments_copy_and_a_platform_schema_are_refused() {
        for table in [
            "app_x__staging.lines",
            "information_schema.tables",
            "oxy_event_log.events",
        ] {
            let manifest = json!({ "functions": { "a": { "on": { "table": table } } } });
            assert!(declared_watches(Some(&manifest)).is_err(), "{table}");
        }
    }
}
