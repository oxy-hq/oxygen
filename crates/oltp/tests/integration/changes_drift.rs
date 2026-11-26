//! The relay's drift check against a real Postgres (OLTP change stream design
//! §5.2, layer 3): `drift_sql` names each declared table whose capture is no
//! longer as an install left it, and a store without its DDL guard.
//!
//! Each test makes its drift as the writer, on a store without the guard
//! (the local provider's owner may not create it), so its rows also carry
//! `no_guard`; [`tables`] leaves that row out. Only the store with nothing
//! drifted is guarded, so it answers no rows at all.

use std::time::Duration;

use oxy_oltp::changes::{CaptureSpec, drift_sql, owner_install_sql, pin_session_sql};

use crate::changes_capture::{Store, run_in_transaction};
use crate::render_pg_error;

/// A row of `drift_sql`: `writer, table_name, drift`.
type DriftRow = (Option<String>, Option<String>, String);

/// `drift_sql` over the store's writer and each of `tables`, as the relay
/// runs it: as the owner, in a pinned transaction.
async fn drift(store: &Store, tables: &[&str]) -> Result<Vec<DriftRow>, String> {
    let mut client = oxy_oltp::connect::connect(&store.fx.owner_dsn, "changes drift test")
        .await
        .map_err(render_pg_error)?;
    let writers = vec![store.writer.as_str(); tables.len()];
    let txn = client.transaction().await.map_err(render_pg_error)?;
    txn.batch_execute(&pin_session_sql(
        Duration::from_secs(1),
        Duration::from_secs(30),
    ))
    .await
    .map_err(render_pg_error)?;
    let rows = txn
        .query(&drift_sql(), &[&writers, &tables])
        .await
        .map_err(render_pg_error)?;
    txn.commit().await.map_err(render_pg_error)?;
    Ok(rows
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect())
}

/// The rows naming a table, as `(writer, table, drift)`: `no_guard` left out.
fn tables(rows: Vec<DriftRow>) -> Vec<(String, String, String)> {
    rows.into_iter()
        .filter_map(|(writer, table, drift)| Some((writer?, table?, drift)))
        .collect()
}

/// `leases` installed on `store`, then `sql` run as the writer; the drift of
/// `leases`.
async fn drifted_by(store: &Store, sql: &str) -> Result<Vec<DriftRow>, String> {
    store.leases(&[]).await?;
    store.as_writer(sql).await?;
    drift(store, &["leases"]).await
}

/// One row of `table`'s `drift` for `store`'s writer.
fn one(store: &Store, table: &str, drift: &str) -> Vec<(String, String, String)> {
    vec![(store.writer.clone(), table.to_owned(), drift.to_owned())]
}

#[tokio::test]
async fn nothing_drifted_is_no_rows() {
    let Some(store) = Store::guarded("drift_none").await else {
        return;
    };
    let out = async {
        store.leases(&[]).await?;
        drift(&store, &["leases"]).await
    }
    .await;
    store.cleanup().await;

    let rows = out.expect("install, and check");
    assert!(rows.is_empty(), "{rows:?}");
}

#[tokio::test]
async fn a_disabled_trigger_is_drift() {
    let Some(store) = Store::create("drift_disabled").await else {
        return;
    };
    let out = drifted_by(&store, "ALTER TABLE leases DISABLE TRIGGER oxy_capture_i").await;
    store.cleanup().await;

    let rows = out.expect("install, disable, and check");
    assert_eq!(tables(rows), one(&store, "leases", "triggers"));
}

#[tokio::test]
async fn a_dropped_trigger_is_drift() {
    let Some(store) = Store::create("drift_dropped").await else {
        return;
    };
    let out = drifted_by(&store, "DROP TRIGGER oxy_capture_d ON leases").await;
    store.cleanup().await;

    let rows = out.expect("install, drop, and check");
    assert_eq!(tables(rows), one(&store, "leases", "triggers"));
}

/// `notes` is declared beside `leases` and was never installed: only it
/// drifted.
#[tokio::test]
async fn a_declared_table_never_installed_is_drift() {
    let Some(store) = Store::create("drift_never").await else {
        return;
    };
    let out = async {
        store.leases(&[]).await?;
        store
            .as_writer("CREATE TABLE notes (id bigint PRIMARY KEY)")
            .await?;
        drift(&store, &["leases", "notes"]).await
    }
    .await;
    store.cleanup().await;

    let rows = out.expect("install leases, create notes, and check");
    assert_eq!(tables(rows), one(&store, "notes", "not_captured"));
}

/// The owner's step of an install ran and its commit did not.
#[tokio::test]
async fn an_install_left_uncommitted_is_drift() {
    let Some(store) = Store::create("drift_uncommitted").await else {
        return;
    };
    let out = async {
        store
            .as_writer("CREATE TABLE leases (id bigint PRIMARY KEY, name text)")
            .await?;
        let capture = store.shape("leases", &[]).await?;
        let spec = CaptureSpec {
            writer: store.writer.as_str(),
            capture: &capture,
            schema_version: 1,
        };
        let owner = owner_install_sql(
            &spec,
            &store.fx.role_name(&store.app),
            &store.image_role(),
            None,
        )
        .map_err(|e| e.to_string())?;
        // One transaction, as the promote runs it: its head is a `SET LOCAL`.
        run_in_transaction(&store.fx.owner_dsn, &owner).await?;
        drift(&store, &["leases"]).await
    }
    .await;
    store.cleanup().await;

    let rows = out.expect("the owner's install, and check");
    assert_eq!(tables(rows), one(&store, "leases", "not_committed"));
}

/// Capture installed and in place, on a store whose owner could not create
/// the guard: the one row says so.
#[tokio::test]
async fn a_store_without_its_guard_says_so() {
    let Some(store) = Store::create("drift_no_guard").await else {
        return;
    };
    let out = async {
        store.leases(&[]).await?;
        drift(&store, &["leases"]).await
    }
    .await;
    store.cleanup().await;

    let rows = out.expect("install, and check");
    assert_eq!(rows, [(None, None, "no_guard".to_owned())]);
}
