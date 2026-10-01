//! Procedure definitions shaped like pokehouse's (synthetic SQL: the shapes
//! and table names follow the phase 2 plan's walk-through; no customer SQL),
//! for the transform-detection tests.

use serde_json::{Value, json};

use super::transforms::DatabaseTypes;

pub(super) fn databases() -> DatabaseTypes {
    [
        ("pokehouse", "airhouse_managed"),
        ("clickhouse", "clickhouse"),
        ("legacy_lake", "airhouse"),
    ]
    .into_iter()
    .map(|(n, t)| (n.to_string(), t.to_string()))
    .collect()
}

pub(super) fn sql(name: &str, sql: &str) -> Value {
    json!({ "name": name, "type": "execute_sql", "database": "pokehouse", "sql_query": sql })
}

pub(super) fn note() -> Value {
    json!({ "name": "note", "type": "formatter", "template": "done" })
}

/// `sql/camera_tts.sql` in the staging revision.
pub(super) fn sql_file(path: &str) -> Option<String> {
    (path == "sql/camera_tts.sql").then(|| {
        "INSERT INTO cameras.time_to_serve_daily SELECT date_trunc('day', seen_at), \
         avg(seconds) FROM cameras.serves GROUP BY 1"
            .to_string()
    })
}

/// A procedure definition shaped like pokehouse's `name`.
pub(super) fn pokehouse(name: &str) -> Value {
    let tasks = match name {
        "rollups/restaurant_analytics_daily_rollups" => json!([
            sql(
                "sales",
                "CREATE OR REPLACE TABLE toast_pos.sales_daily_metrics AS \
                 SELECT business_date, restaurant_id, sum(net_sales) AS net_sales \
                 FROM toast_pos.orders GROUP BY 1, 2"
            ),
            note()
        ]),
        "rollups/camera_time_to_serve_rollups" => json!([
            { "name": "tts", "type": "execute_sql", "database": "pokehouse",
              "sql_file": "sql/camera_tts.sql" }
        ]),
        "site_selection_refresh" => json!([
            { "name": "stale", "type": "conditional",
              "conditions": [{ "if": "{{ full }}", "tasks": [
                  sql("cells", "DELETE FROM site_selection.site_hex_cells \
                       WHERE refreshed_at < now() - INTERVAL 7 DAY")
              ]}],
              "else": [note()] },
            { "name": "each", "type": "loop_sequential", "values": ["sf", "oak"], "tasks": [
                sql("score", "INSERT INTO site_selection.site_scores \
                     SELECT h3, count(*) FROM site_selection.site_hex_cells GROUP BY 1")
            ]}
        ]),
        "compute_toast_journal_entry_airhouse" => json!([
            sql(
                "clear",
                "DELETE FROM toast_pos.toast_journal_entry_lines \
                 WHERE business_date = '{{ date }}'"
            ),
            sql(
                "lines",
                "INSERT INTO toast_pos.toast_journal_entry_lines \
                 SELECT * FROM toast_pos.journal_staging"
            ),
            sql(
                "balance_check",
                "SELECT sum(debit) - sum(credit) \
                 FROM toast_pos.toast_journal_entry_lines"
            )
        ]),
        "qb_je_account_map_seed_airhouse" => json!([sql(
            "seed",
            "CREATE SCHEMA IF NOT EXISTS bookkeeping; \
                 CREATE TABLE IF NOT EXISTS bookkeeping.qb_je_account_map \
                 (toast_account VARCHAR, qb_account VARCHAR); \
                 INSERT INTO bookkeeping.qb_je_account_map VALUES ('sales', '4000')"
        )]),
        "bookkeeping_provision_airhouse" => json!([sql(
            "ensure_source_tables",
            "DROP TABLE IF EXISTS bookkeeping.stage; \
                 CREATE TABLE bookkeeping.stage (id INTEGER); \
                 ALTER TABLE bookkeeping.stage ADD COLUMN amount DOUBLE"
        )]),
        "toast_ingest_and_rollups" => json!([
            { "name": "ingest", "type": "airway", "pipeline": "airway/toast.airway.yml" },
            { "name": "rollups", "type": "workflow",
              "src": "workflows/rollups/restaurant_analytics_daily_rollups.procedure.yml" }
        ]),
        "compute_toast_journal_entry" => json!([
            { "name": "clear", "type": "execute_sql", "database": "clickhouse",
              "sql_query": "ALTER TABLE journal DELETE WHERE business_date = today()" }
        ]),
        "restaurant_insights" => json!([
            sql("read", "SELECT * FROM toast_pos.sales_daily_metrics"),
            { "name": "explain", "type": "agent", "agent_ref": "agents/analyst.agentic.yml",
              "prompt": "why" }
        ]),
        "weekly_report" => json!([sql("read", "SELECT count(*) FROM toast_pos.orders"), note()]),
        other => panic!("no fixture {other}"),
    };
    json!({ "name": name, "tasks": tasks })
}
