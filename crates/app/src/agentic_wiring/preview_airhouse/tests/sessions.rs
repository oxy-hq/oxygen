//! How a batch uses its connection: one connection per batch, a transaction
//! rolled back or its connection poisoned, reads and streams shared.

use std::sync::Arc;
use std::time::Duration;

use agentic_connector::DatabaseConnector;
use futures::FutureExt;

use super::fake::FakeAirhouse;
use super::{connector, p, q};
use crate::agentic_wiring::preview_airhouse::{Scope, Use};

/// Wait (bounded) for the transaction lease to go back.
async fn wait_released(fake: &FakeAirhouse) {
    for _ in 0..200 {
        if fake.is_released() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the lease was never released: {:?}", fake.sent());
}

/// A released transaction connection is clean: its last statement is a
/// ROLLBACK that succeeded, or it was poisoned and is never handed out again.
fn released_clean(fake: &FakeAirhouse) -> bool {
    let last = fake.sent().last().cloned().unwrap_or_default();
    let rolled_back = last.ends_with("statement ROLLBACK") && !fake.fail_on.contains(&"ROLLBACK");
    fake.is_poisoned() || rolled_back
}

#[tokio::test]
async fn a_batch_runs_on_one_connection_and_rolls_back_a_mid_batch_failure() {
    let (pos, site) = (
        format!("{}.\"orders\"", q("toast_pos")),
        format!("{}.\"boom\"", q("site_selection")),
    );
    let batch = format!(
        "BEGIN; INSERT INTO {pos} VALUES (1); INSERT INTO {site} VALUES (1); \
         INSERT INTO {pos} VALUES (2); COMMIT"
    );
    let fake = Arc::new(FakeAirhouse::failing_on(&["boom"]));
    let err = connector(&fake)
        .execute_statement(&batch)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("the fake Airhouse failed it"),
        "{err}"
    );
    // One connection, held for the transaction, on one Writer scoped to both
    // schemas the batch writes.
    assert_eq!(
        fake.checkouts(),
        vec![Scope::Writer(vec![p("site_selection"), p("toast_pos")])]
    );
    assert_eq!(fake.usages(), vec![Use::Transaction]);
    assert_eq!(
        fake.sent(),
        vec![
            "#1 statement BEGIN".to_string(),
            format!("#1 statement INSERT INTO {pos} VALUES (1)"),
            format!("#1 statement INSERT INTO {site} VALUES (1)"),
            "#1 statement ROLLBACK".to_string(),
        ]
    );
    assert!(!fake.is_poisoned(), "the rollback succeeded");

    // Without the failure: the same one connection, committed, no rollback,
    // and the result statement is the one read for rows.
    let fake = Arc::new(FakeAirhouse::default());
    let ok = batch.replace("boom", "cells") + ";";
    connector(&fake)
        .execute_query(&ok.replace("COMMIT;", "SELECT 1; COMMIT"), 5)
        .await
        .unwrap();
    let sent = fake.sent();
    assert_eq!(fake.checkouts().len(), 1);
    assert!(sent.iter().all(|s| s.starts_with("#1 ")), "{sent:?}");
    assert_eq!(sent.first().unwrap(), "#1 statement BEGIN");
    assert_eq!(sent[sent.len() - 2], "#1 query SELECT 1");
    assert_eq!(sent.last().unwrap(), "#1 statement COMMIT");

    // No transaction: a shared connection, nothing to roll back, the batch
    // just stops.
    let fake = Arc::new(FakeAirhouse::failing_on(&["boom"]));
    let plain = format!("INSERT INTO {site} VALUES (1); INSERT INTO {pos} VALUES (2)");
    connector(&fake)
        .execute_statement(&plain)
        .await
        .unwrap_err();
    assert_eq!(fake.usages(), vec![Use::Shared]);
    assert_eq!(
        fake.sent(),
        vec![format!("#1 statement INSERT INTO {site} VALUES (1)")]
    );
}

/// A read whose rows nobody asked for runs only as `EXPLAIN`: its rows have
/// no effect to keep, so running it in full would only cost a scan, but its
/// binding still runs — see
/// `a_non_final_reads_missing_relation_still_fails_the_batch`. The result
/// read of an `execute_statement` still runs in full, once.
#[tokio::test]
async fn a_read_whose_rows_nobody_asked_for_runs_only_as_explain() {
    let own = format!("{}.\"t\"", q("toast_pos"));
    let fake = Arc::new(FakeAirhouse::default());
    let conn = connector(&fake);
    conn.execute_query("SELECT 1; SELECT 2", 5).await.unwrap();
    conn.execute_statement(&format!("SELECT 3; INSERT INTO {own} VALUES (1)"))
        .await
        .unwrap();
    let stream = conn.execute_query_full("SELECT 4; SELECT 5").await.unwrap();
    drop(stream);
    conn.execute_statement("SELECT 6").await.unwrap();
    assert_eq!(
        fake.sent(),
        vec![
            "#1 statement EXPLAIN SELECT 1".to_string(),
            "#1 query SELECT 2".to_string(),
            "#2 statement EXPLAIN SELECT 3".to_string(),
            format!("#2 statement INSERT INTO {own} VALUES (1)"),
            "#3 statement EXPLAIN SELECT 4".to_string(),
            "#3 full SELECT 5".to_string(),
            "#4 untyped SELECT 6".to_string(),
        ]
    );
}

/// A non-final read's rows are dropped, but not its binding: it runs as
/// `EXPLAIN`, so a missing table still fails the batch exactly as running
/// the read for real would have — the whole point being that a preview dry
/// run should not go green where production would fail on that read.
#[tokio::test]
async fn a_non_final_reads_missing_relation_still_fails_the_batch() {
    let own = format!("{}.\"orders\"", q("toast_pos"));
    let fake = Arc::new(FakeAirhouse::failing_on(&["ghost_table"]));
    let err = connector(&fake)
        .execute_statement(&format!(
            "SELECT * FROM toast_pos.ghost_table; INSERT INTO {own} VALUES (1)"
        ))
        .await
        .expect_err("a missing relation must fail the batch, not just be skipped");
    assert!(
        err.to_string().contains("the fake Airhouse failed it"),
        "{err}"
    );
    let sent = fake.sent();
    assert_eq!(
        sent.len(),
        1,
        "the INSERT after the bad read never sent: {sent:?}"
    );
    assert!(
        sent[0].starts_with("#1 statement EXPLAIN SELECT"),
        "{sent:?}"
    );
    // Unshadowed, a read overlays to nothing (it stays live-table, read-only)
    // — see `reads_get_the_overlay` — so the missing name survives untouched
    // into the `EXPLAIN` that binds it.
    assert!(sent[0].contains("toast_pos.ghost_table"), "{sent:?}");
}

/// `DESCRIBE`, `SHOW` and a top-level `EXPLAIN` are reads too (no rows kept,
/// non-final), but DuckDB cannot wrap them — `EXPLAIN DESCRIBE t` errors —
/// unlike a `SELECT`/`WITH`/`FROM`/`TABLE`/`VALUES`-shaped read. Each must
/// run in full instead, or the batch fails on a shape DuckDB genuinely
/// accepts. The `fail_on` needles stand in for that DuckDB refusal: they
/// only appear in what is sent if the gate is missing.
#[tokio::test]
async fn a_non_final_show_describe_or_explain_runs_in_full_not_wrapped() {
    let own = format!("{}.\"orders\"", q("toast_pos"));
    let fake = Arc::new(FakeAirhouse::failing_on(&[
        "EXPLAIN DESCRIBE",
        "EXPLAIN SHOW",
        "EXPLAIN EXPLAIN",
    ]));
    let batch = format!(
        "DESCRIBE toast_pos.orders; SHOW TABLES; EXPLAIN SELECT 1; INSERT INTO {own} VALUES (1)"
    );
    connector(&fake)
        .execute_statement(&batch)
        .await
        .expect("DESCRIBE/SHOW/EXPLAIN ahead of a write must not be wrapped in EXPLAIN");
    let sent = fake.sent();
    assert!(
        sent.iter().any(|s| s.contains("DESCRIBE")),
        "the DESCRIBE never sent: {sent:?}"
    );
    assert!(
        sent.iter().any(|s| s.contains("SHOW TABLES")),
        "the SHOW never sent: {sent:?}"
    );
    assert!(
        sent.iter().any(|s| s.contains("EXPLAIN SELECT")),
        "the EXPLAIN never sent: {sent:?}"
    );
    assert!(
        sent.iter()
            .any(|s| s.ends_with(&format!("INSERT INTO {own} VALUES (1)"))),
        "the write after them never sent: {sent:?}"
    );
}

#[tokio::test]
async fn a_batch_abandoned_mid_transaction_is_rolled_back() {
    let pos = format!("{}.\"orders\"", q("toast_pos"));
    let batch = format!("BEGIN; INSERT INTO {pos} SELECT * FROM slow_source.t; COMMIT");
    let fake = Arc::new(FakeAirhouse::hanging_on("slow_source"));
    let conn = connector(&fake);
    let call = conn.execute_statement(&batch);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), call)
            .await
            .is_err(),
        "the fake hangs, so the call is abandoned"
    );
    // The rollback runs on the same connection, and only then is it released.
    wait_released(&fake).await;
    assert_eq!(
        fake.sent().last().map(String::as_str),
        Some("#1 statement ROLLBACK"),
        "{:?}",
        fake.sent()
    );
    assert!(released_clean(&fake));
}

/// A `BEGIN` cancelled in flight may have run: the connection is rolled back
/// (harmlessly, if it had not) before anyone else gets it.
#[tokio::test]
async fn a_cancelled_begin_never_leaves_a_dirty_connection() {
    let pos = format!("{}.\"orders\"", q("toast_pos"));
    let fake = Arc::new(FakeAirhouse::hanging_on("BEGIN"));
    let conn = connector(&fake);
    let batch = format!("BEGIN; INSERT INTO {pos} VALUES (1); COMMIT");
    let call = conn.execute_statement(&batch);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), call)
            .await
            .is_err()
    );
    wait_released(&fake).await;
    assert!(released_clean(&fake), "{:?}", fake.sent());
    assert_eq!(
        fake.sent().last().map(String::as_str),
        Some("#1 statement ROLLBACK")
    );
}

/// A rollback that fails leaves the connection mid-transaction: it is
/// poisoned, whether the batch failed or was abandoned.
#[tokio::test]
async fn a_failed_rollback_poisons_the_connection() {
    let (pos, site) = (
        format!("{}.\"orders\"", q("toast_pos")),
        format!("{}.\"boom\"", q("site_selection")),
    );
    let fake = Arc::new(FakeAirhouse::failing_on(&["boom", "ROLLBACK"]));
    let batch =
        format!("BEGIN; INSERT INTO {pos} VALUES (1); INSERT INTO {site} VALUES (1); COMMIT");
    connector(&fake)
        .execute_statement(&batch)
        .await
        .unwrap_err();
    assert!(fake.is_poisoned(), "{:?}", fake.sent());

    let fake = Arc::new(FakeAirhouse {
        hang_on: Some("slow_source"),
        fail_on: vec!["ROLLBACK"],
        ..FakeAirhouse::default()
    });
    let conn = connector(&fake);
    let slow = format!("BEGIN; INSERT INTO {pos} SELECT * FROM slow_source.t; COMMIT");
    let call = conn.execute_statement(&slow);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), call)
            .await
            .is_err()
    );
    wait_released(&fake).await;
    assert!(fake.is_poisoned(), "{:?}", fake.sent());
}

/// A rollback that never answers (it queues behind whatever an abandoned
/// statement still runs) is given up on in bounded time: the connection is
/// poisoned and the slot released, not held for as long as that runs.
#[tokio::test(start_paused = true)]
async fn a_rollback_that_never_answers_poisons_the_connection() {
    let (pos, site) = (
        format!("{}.\"orders\"", q("toast_pos")),
        format!("{}.\"boom\"", q("site_selection")),
    );
    let fake = Arc::new(FakeAirhouse {
        fail_on: vec!["boom"],
        hang_on: Some("ROLLBACK"),
        ..FakeAirhouse::default()
    });
    let batch =
        format!("BEGIN; INSERT INTO {pos} VALUES (1); INSERT INTO {site} VALUES (1); COMMIT");
    let conn = connector(&fake);
    let call = conn.execute_statement(&batch);
    let err = tokio::time::timeout(Duration::from_secs(600), call)
        .await
        .expect("the batch waited on the rollback without bound")
        .unwrap_err();
    assert!(
        err.to_string().contains("the fake Airhouse failed it"),
        "{err}"
    );
    assert!(
        fake.is_poisoned() && fake.is_released(),
        "{:?}",
        fake.sent()
    );
}

/// With no runtime to roll back on, an abandoned transaction's connection is
/// poisoned on the spot.
#[test]
fn a_batch_dropped_without_a_runtime_poisons_its_connection() {
    let pos = format!("{}.\"orders\"", q("toast_pos"));
    let fake = Arc::new(FakeAirhouse::hanging_on("slow_source"));
    let conn = connector(&fake);
    let batch = format!("BEGIN; INSERT INTO {pos} SELECT * FROM slow_source.t; COMMIT");
    let mut call = Box::pin(conn.execute_statement(&batch));
    assert!(call.as_mut().now_or_never().is_none(), "the fake hangs");
    drop(call);
    assert!(
        fake.is_released() && fake.is_poisoned(),
        "{:?}",
        fake.sent()
    );
}

#[tokio::test]
async fn a_stream_never_spans_a_transaction() {
    let fake = Arc::new(FakeAirhouse::default());
    let conn = connector(&fake);
    for sql in [
        "BEGIN; SELECT 1; COMMIT".to_string(),
        format!("SELECT 1; INSERT INTO {}.\"t\" VALUES (1)", q("toast_pos")),
    ] {
        let Err(err) = conn.execute_query_full(&sql).await else {
            panic!("`{sql}` was streamed");
        };
        assert!(err.to_string().contains("Nothing was sent"), "{err}");
    }
    assert!(fake.sent().is_empty(), "{:?}", fake.sent());
}

/// A stream holds nothing another query waits on: a task that queries while
/// it drains a stream carries on.
#[tokio::test]
async fn a_task_can_query_while_it_drains_a_stream() {
    let fake = Arc::new(FakeAirhouse::default());
    let conn = connector(&fake);
    let stream = conn.execute_query_full("SELECT 1").await.unwrap();
    let query = conn.execute_query("SELECT 2", 5);
    tokio::time::timeout(Duration::from_secs(2), query)
        .await
        .expect("the query waited on the open stream")
        .unwrap();
    drop(stream);
    assert_eq!(fake.usages(), vec![Use::Shared, Use::Shared]);
}
