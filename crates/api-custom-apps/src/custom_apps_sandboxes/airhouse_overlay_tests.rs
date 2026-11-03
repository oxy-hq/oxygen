use chrono::{TimeZone, Utc};

use super::*;

fn held(schema: &str, table: &str, state: &str, rows: Option<u64>) -> HeldTable {
    HeldTable {
        schema: schema.into(),
        table: table.into(),
        state: state.into(),
        rows,
    }
}

#[test]
fn the_diff_names_the_mode_the_pin_and_the_tables_sorted() {
    let pinned = Utc.with_ymd_and_hms(2026, 11, 2, 9, 30, 0).unwrap();
    let diff = diff_json(
        AirhouseMode::Snapshot,
        Some(pinned),
        vec![
            held("app_w", "orders", "partial", Some(2)),
            held("app_w", "events", "shadow", Some(41)),
            held("app_a", "zeta", "shadow", None),
        ],
    );
    assert_eq!(
        diff,
        json!({
            "mode": "snapshot",
            "pinned_at": "2026-11-02T09:30:00+00:00",
            "tables": [
                { "schema": "app_a", "table": "zeta", "state": "shadow", "rows": null },
                { "schema": "app_w", "table": "events", "state": "shadow", "rows": 41 },
                { "schema": "app_w", "table": "orders", "state": "partial", "rows": 2 },
            ],
        })
    );
}

#[test]
fn an_overlay_with_no_pin_and_no_tables_reports_both_empty() {
    assert_eq!(
        diff_json(AirhouseMode::Branch, None, Vec::new()),
        json!({ "mode": "branch", "pinned_at": null, "tables": [] })
    );
}

#[test]
fn identifiers_are_quoted_with_embedded_quotes_doubled() {
    assert_eq!(quote("events"), "\"events\"");
    assert_eq!(quote("we\"ird"), "\"we\"\"ird\"");
}
