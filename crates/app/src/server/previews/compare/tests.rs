//! The compare's SQL against in-process DuckDB (the engine Airhouse runs):
//! live `toast_pos.sales` beside the preview's copy in
//! `preview_feat_x_abc123__toast_pos.sales`, through a connector that records
//! every statement.

use std::sync::Mutex;

use agentic_connector::{
    ConnectorError, DatabaseConnector, DuckDbConnector, ExecutionResult, SqlDialect,
};
use async_trait::async_trait;

use super::table::{Limits, ReadFailed, Sides, TableCompare, compare_table};
use super::{CompareError, PREEXISTING_CAVEAT, caveats, reads};

const PREVIEW: &str = "preview_feat_x_abc123__toast_pos";

/// DuckDB, recording what reached it; a statement containing `fail_on` fails
/// with `error` instead, as an engine would.
struct Recording {
    inner: DuckDbConnector,
    sent: Mutex<Vec<String>>,
    fail_on: Option<(&'static str, &'static str)>,
}

#[async_trait]
impl DatabaseConnector for Recording {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::DuckDb
    }

    async fn execute_query(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        self.sent.lock().unwrap().push(sql.to_string());
        if let Some((needle, error)) = self.fail_on
            && sql.contains(needle)
        {
            return Err(ConnectorError::query_failed(
                sql.to_string(),
                error.to_string(),
            ));
        }
        self.inner.execute_query(sql, sample_limit).await
    }
}

/// Live and preview `sales`, each built by `live` / `preview` (SQL naming
/// `{t}` for the table).
fn lake(live: &str, preview: &str) -> Recording {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    let setup = format!(
        "CREATE SCHEMA toast_pos; CREATE SCHEMA \"{PREVIEW}\"; {}; {};",
        live.replace("{t}", "toast_pos.sales"),
        preview.replace("{t}", &format!("\"{PREVIEW}\".sales")),
    );
    conn.execute_batch(&setup).unwrap();
    Recording {
        inner: DuckDbConnector::new(conn),
        sent: Mutex::new(vec![]),
        fail_on: None,
    }
}

const SALES: &str = "CREATE TABLE {t} (business_date DATE, restaurant_id INTEGER, \
                     net_sales DECIMAL(12,2), order_count INTEGER)";
const ROWS: &str = "INSERT INTO {t} VALUES ('2026-09-01', 1, 1500.25, 40), \
                    ('2026-09-01', 2, 830.00, 22), ('2026-09-02', 1, 1720.10, 45)";

fn limits(diff_max_rows: u64) -> Limits {
    Limits {
        diff_max_rows,
        ignore_columns: vec![],
        catalog: None,
    }
}

fn sides(partial: bool) -> Sides {
    Sides {
        live: ("toast_pos".into(), "sales".into()),
        preview_schema: PREVIEW.into(),
        partial,
        dropped: false,
        preexisting: false,
    }
}

async fn run(lake: &Recording, partial: bool, limits: &Limits) -> TableCompare {
    compare_table(lake, &sides(partial), limits)
        .await
        .expect("compare")
}

fn both(sql: &[&str]) -> String {
    sql.join("; ")
}

#[tokio::test]
async fn equal_tables_report_equal() {
    let same = both(&[SALES, ROWS]);
    let lake = lake(&same, &same);
    let out = run(&lake, false, &limits(1_000)).await;
    assert!(out.equal, "{out:?}");
    assert_eq!((out.live_rows, out.preview_rows), (Some(3), Some(3)));
    assert_eq!((out.only_in_preview, out.only_in_live), (Some(0), Some(0)));
    assert!(out.columns_added.is_empty() && out.columns_removed.is_empty());
    assert!(out.columns_retyped.is_empty() && out.skipped_reason.is_none());
    assert!(
        !lake
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("EXCEPT ALL")),
        "equal fingerprints need no difference"
    );

    // Control: one changed value makes them unequal, one row each way.
    let changed = both(&[SALES, &ROWS.replace("830.00", "831.00")]);
    let out = run(&lake_of(&same, &changed), false, &limits(1_000)).await;
    assert!(!out.equal);
    assert_eq!((out.only_in_preview, out.only_in_live), (Some(1), Some(1)));
}

fn lake_of(live: &str, preview: &str) -> Recording {
    lake(live, preview)
}

#[tokio::test]
async fn retyped_column_is_reported() {
    let added = SALES.replace(
        "order_count INTEGER)",
        "order_count BIGINT, channel VARCHAR)",
    );
    let lake = lake(
        &both(&[SALES, ROWS]),
        &both(&[
            &added,
            "INSERT INTO {t} SELECT *, 'dine_in' FROM toast_pos.sales",
        ]),
    );
    let out = run(&lake, false, &limits(1_000)).await;
    assert!(!out.equal, "{out:?}");
    assert_eq!(out.columns_retyped.len(), 1, "{out:?}");
    let retyped = &out.columns_retyped[0];
    assert_eq!(retyped.column, "order_count");
    assert_eq!(
        (retyped.live.as_str(), retyped.preview.as_str()),
        ("INTEGER", "BIGINT")
    );
    assert_eq!(out.columns_added, vec!["channel".to_string()]);
    assert!(out.columns_removed.is_empty());
    assert_eq!((out.live_rows, out.preview_rows), (Some(3), Some(3)));
}

/// A partial copy started empty and holds only what the preview wrote: the
/// rows it added are counted, and nothing is claimed about live's side.
#[tokio::test]
async fn partial_table_reports_only_in_preview() {
    let lake = lake(
        &both(&[SALES, ROWS]),
        &both(&[
            SALES,
            "INSERT INTO {t} VALUES ('2026-09-03', 1, 99.00, 3), ('2026-09-03', 2, 42.00, 2)",
        ]),
    );
    let out = run(&lake, true, &limits(1_000)).await;
    assert!(out.partial && !out.equal, "{out:?}");
    assert_eq!(out.only_in_preview, Some(2));
    assert_eq!(out.only_in_live, None, "not meaningful for a partial copy");
    assert_eq!((out.live_rows, out.preview_rows), (Some(3), Some(2)));
}

#[tokio::test]
async fn over_limit_skips_except_all() {
    let lake = lake(
        &both(&[SALES, ROWS]),
        &both(&[SALES, &ROWS.replace("830.00", "831.00")]),
    );
    let out = run(&lake, false, &limits(2)).await;
    assert!(!out.equal);
    assert_eq!((out.only_in_preview, out.only_in_live), (None, None));
    let why = out.skipped_reason.expect("skipped");
    assert!(why.contains("OXY_PREVIEW_DIFF_MAX_ROWS"), "{why}");
    assert!(
        !lake
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("EXCEPT ALL")),
        "no difference over the limit"
    );
    // Control: under the limit the difference runs.
    let under = run(&lake, false, &limits(3)).await;
    assert_eq!(under.only_in_preview, Some(1));
}

/// The outcome is counts and names: no value from either table's rows.
#[tokio::test]
async fn outcome_carries_counts_not_values() {
    let secret = "INSERT INTO {t} VALUES ('2031-01-07', 424242, 98765.43, 777)";
    let lake = lake(&both(&[SALES, ROWS]), &both(&[SALES, ROWS, secret]));
    let out = run(&lake, false, &limits(1_000)).await;
    assert_eq!(out.only_in_preview, Some(1), "{out:?}");
    let json = serde_json::to_string(&out).unwrap();
    for value in ["2031-01-07", "424242", "98765", "777", "1500.25", "2026-09"] {
        assert!(!json.contains(value), "`{value}` leaked into {json}");
    }
    assert!(json.contains("toast_pos.sales"), "names are fine: {json}");
}

/// An engine error can quote a row value. A failed read leaves only fixed
/// words in the outcome, and a failed compare only fixed words in its stored
/// failure; the value is in neither, nor in the logged class.
#[tokio::test]
async fn a_failed_read_leaves_no_engine_words_in_the_outcome() {
    let error = "Conversion Error: Could not convert string '424242-secret' to INT32";
    let mut failing = lake(
        &both(&[SALES, ROWS]),
        &both(&[SALES, &ROWS.replace("830.00", "831.00")]),
    );
    failing.fail_on = Some(("EXCEPT ALL", error));
    let why = compare_table(&failing, &sides(false), &limits(1_000))
        .await
        .expect_err("the difference failed");
    assert_eq!(why, ReadFailed::Difference);
    let stored = serde_json::to_string(&TableCompare::skipped(&sides(false), why)).unwrap();
    assert!(!stored.contains("424242"), "{stored}");
    assert!(stored.contains(ReadFailed::Difference.reason()), "{stored}");
    let class = reads::error_class(error);
    assert!(!class.contains("424242"), "{class}");
    assert!(class.starts_with("Conversion Error"), "{class}");

    let db = sea_orm::DbErr::Custom(format!("duplicate key value (424242) {error}"));
    let failed = format!("preview compare failed: {}", CompareError::Database(db));
    assert!(!failed.contains("424242"), "{failed}");
}

/// With no catalog named, nothing is filtered by catalog: a table in a
/// catalog that is not the session's current one is still found. Naming the
/// current one (the guess the old SQL made) finds nothing — the control.
#[tokio::test]
async fn the_columns_are_found_in_any_catalog_when_none_is_named() {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute_batch(&format!(
        "ATTACH ':memory:' AS lake; CREATE SCHEMA lake.toast_pos; {}",
        SALES.replace("{t}", "lake.toast_pos.sales")
    ))
    .unwrap();
    let conn = DuckDbConnector::new(conn);
    let found = reads::columns(&conn, "toast_pos", "sales", &limits(1))
        .await
        .unwrap();
    assert_eq!(found.len(), 4, "{found:?}");
    let guessed = Limits {
        catalog: Some("memory".into()),
        ..limits(1)
    };
    let none = reads::columns(&conn, "toast_pos", "sales", &guessed)
        .await
        .unwrap();
    assert!(none.is_empty(), "control: {none:?}");
}

/// A table the build dropped reports live's rows and why; a partial copy over
/// the limit says it is partial; a preexisting table is flagged and adds its
/// caveat.
#[tokio::test]
async fn dropped_partial_and_preexisting_tables_say_so() {
    let lake = lake(&both(&[SALES, ROWS]), &both(&[SALES, ROWS]));
    let dropped = Sides {
        dropped: true,
        ..sides(false)
    };
    let out = compare_table(&lake, &dropped, &limits(1_000))
        .await
        .unwrap();
    assert!(out.dropped, "{out:?}");
    assert_eq!((out.live_rows, out.preview_rows), (Some(3), None));
    assert_eq!(
        out.skipped_reason.as_deref(),
        Some("the build dropped this table")
    );

    let partial = run(&lake, true, &limits(2)).await;
    let why = partial.skipped_reason.expect("skipped");
    assert!(why.starts_with("partial copy: diff skipped"), "{why}");

    let preexisting = Sides {
        preexisting: true,
        ..sides(false)
    };
    let out = compare_table(&lake, &preexisting, &limits(1_000))
        .await
        .unwrap();
    assert!(out.preexisting && out.equal, "{out:?}");
    let none = std::collections::HashSet::new();
    let preexisting_caveat = PREEXISTING_CAVEAT.to_string();
    assert!(caveats(std::slice::from_ref(&out), &none).contains(&preexisting_caveat));
    let fresh = run(&lake, false, &limits(1_000)).await;
    assert_eq!(
        caveats(std::slice::from_ref(&fresh), &none).len(),
        1,
        "only the freshness caveat"
    );

    // The build only READ a table the preview held (its reads went to the
    // preview's copy): no compared table is preexisting, the caveat is said.
    let held = std::collections::HashSet::from([("gl".to_string(), "rates".to_string())]);
    assert!(!fresh.preexisting);
    assert!(caveats(&[fresh], &held).contains(&preexisting_caveat));
}
