//! A production write reaches its change table signed and typed, and each
//! change lands exactly once: after a second pass, after a pass that died
//! between the Airhouse commit and its confirm, and when one source
//! transaction is larger than a batch.

use std::sync::Arc;

use oxy_api_custom_apps::oltp_changes::relay::sink::install_sink;
use oxy_api_custom_apps::oltp_changes::relay::{Limits, RelayStats};

use super::duck::{FailsOnceAfterCommit, LOST_AFTER_COMMIT};
use super::{PRODUCTION, Relay, text, with_relay};
use crate::custom_app_functions_fixture::invocations;
use crate::oltp_changes_fixture::{LEASES_SQL, WRITE};

/// The change table's columns: the declared table's, typed for DuckDB, then
/// the relay's own.
const LEASES_CHANGES: [(&str, &str); 9] = [
    ("id", "BIGINT"),
    ("name", "VARCHAR"),
    ("_generation", "BIGINT"),
    ("_seq", "BIGINT"),
    ("_txid", "BIGINT"),
    ("_op", "VARCHAR"),
    ("_changed_at", "TIMESTAMP WITH TIME ZONE"),
    ("_actor", "VARCHAR"),
    ("_invocation_id", "UUID"),
];

fn stats(shipped: u64, batches: u64) -> RelayStats {
    RelayStats {
        shipped,
        batches,
        ..RelayStats::default()
    }
}

/// `leases` captured by a first promote, and its install's baseline shipped.
async fn installed(r: &Relay) {
    r.app
        .promote("chg-1", true, &[LEASES_SQL])
        .await
        .expect("the promote installs capture on leases");
    r.settle().await;
}

/// Airhouse's `_progress` and the store's `oxy_changes.progress` agree: the
/// last batch declared was shipped, and it is the one Airhouse holds.
async fn assert_caught_up(r: &Relay) {
    let (intent, shipped) = r.progress().await;
    assert!(shipped > 0, "a batch was shipped");
    assert_eq!(intent, shipped, "nothing declared is left unconfirmed");
    assert_eq!(r.shipped().1, shipped, "_progress.batch is the store's");
}

/// An insert, an update and a delete through the serve route: the host
/// nudged the relay before each commit, and one pass ships all three, typed,
/// with the caller as `_actor` and each call's run as `_invocation_id`, then
/// deletes them from the log.
#[tokio::test]
async fn a_production_write_reaches_the_change_table_signed() {
    with_relay(|r| async move {
        let published = r
            .app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.settle().await;
        r.app
            .write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        r.app
            .write("update leases set name = 'Flat 1b' where id = 1")
            .await;
        r.app.write("delete from leases where id = 1").await;
        assert!(r.nudged_at().await.is_some(), "nudged before the commits");
        assert_eq!(r.waiting(None).await, 3);

        assert_eq!(r.pass().await, stats(3, 1));

        let columns: Vec<(String, String)> = LEASES_CHANGES
            .iter()
            .map(|(name, ty)| (name.to_string(), ty.to_string()))
            .collect();
        assert_eq!(r.columns("leases"), columns);
        let runs = invocations(r.db(), published.app_id, WRITE).await;
        assert_eq!(runs.len(), 3, "{runs:?}");
        let actor = r.app.t.guest_id.to_string();
        let shipped = r.changes(
            "leases",
            "id, name, _op, _actor, CAST(_invocation_id AS VARCHAR)",
        );
        let expected: Vec<Vec<Option<String>>> = [
            ("Flat 1", "insert"),
            ("Flat 1b", "update"),
            ("Flat 1b", "delete"),
        ]
        .iter()
        .zip(&runs)
        .map(|((name, op), run)| {
            let run = run.id.to_string();
            vec![text("1"), text(name), text(op), text(&actor), text(&run)]
        })
        .collect();
        assert_eq!(shipped, expected, "typed, signed, in order");
        assert_eq!(r.waiting(None).await, 0, "the log keeps none of them");
        assert_caught_up(&r).await;
    })
    .await;
}

/// A second pass with nothing new ships nothing and moves no progress; a
/// later write ships alone, so no change is in the table twice.
#[tokio::test]
async fn a_second_pass_ships_nothing_twice() {
    with_relay(|r| async move {
        installed(&r).await;
        r.app
            .write("insert into leases (id, name) values (1, 'Flat 1'), (2, 'Flat 2')")
            .await;
        assert_eq!(r.pass().await, stats(2, 1));
        let first = r.changes("leases", "_seq, id");
        let progress = (r.progress().await, r.shipped());

        assert_eq!(r.pass().await, RelayStats::default(), "nothing to ship");
        assert_eq!(r.changes("leases", "_seq, id"), first);
        assert_eq!((r.progress().await, r.shipped()), progress);

        r.app
            .write("insert into leases (id, name) values (3, 'Flat 3')")
            .await;
        assert_eq!(r.pass().await, stats(1, 1));
        let ids = r.changes("leases", "id");
        assert_eq!(ids, [[text("1")], [text("2")], [text("3")]]);
        assert_eq!(r.aggregate("leases", "count(DISTINCT _seq)"), text("3"));
        assert_eq!(r.waiting(None).await, 0);
        assert_caught_up(&r).await;
    })
    .await;
}

/// The first pass's Airhouse transaction commits, then its sink fails before
/// the store hears of it: the pass fails, backing its mark off, the rows stay
/// in the log, and Airhouse is a batch ahead of the store's confirm. The next
/// pass ships the same rows again under a new batch, the anti-join adds none,
/// the log is emptied, and the backoff is over.
#[tokio::test]
async fn a_pass_that_dies_after_the_airhouse_commit_lands_once() {
    with_relay(|r| async move {
        installed(&r).await;
        r.app
            .write("insert into leases (id, name) values (1, 'A'), (2, 'B'), (3, 'C')")
            .await;
        let written = r.waiting(None).await;
        assert_eq!(written, 3);
        install_sink(Arc::new(FailsOnceAfterCommit::new(r.duck.clone())));

        let died = r
            .try_pass(PRODUCTION, &Limits::default())
            .await
            .expect_err("the pass fails after the commit");
        assert!(died.contains(&r.writer()), "{died}");
        assert!(died.contains(LOST_AFTER_COMMIT), "{died}");
        assert_eq!(r.aggregate("leases", "count(*)"), text("3"), "it landed");
        assert_eq!(r.waiting(None).await, written, "the log keeps the rows");
        let (intent, shipped) = r.progress().await;
        assert!(intent > shipped, "declared, never confirmed");
        assert_eq!(r.shipped().1, intent, "Airhouse holds the batch");
        assert_eq!(r.backoff().await, (1, true, true), "the failure backs off");

        assert_eq!(r.pass().await, stats(3, 1), "shipped again, a new batch");
        assert_eq!(
            r.backoff().await,
            (0, false, false),
            "a pass that ships ends it"
        );
        let landed = r.aggregate("leases", "count(*)");
        assert_eq!(landed, Some(written.to_string()), "once each");
        assert_eq!(r.aggregate("leases", "count(DISTINCT _seq)"), text("3"));
        assert_eq!(r.waiting(None).await, 0, "and the log is empty");
        assert_caught_up(&r).await;
    })
    .await;
}

/// With two rows a batch, one pass allowed one batch, and one transaction of
/// five rows: the batch extends to the transaction's end, so that one batch
/// ships all five under one `_progress.batch`.
#[tokio::test]
async fn a_transaction_larger_than_a_batch_ships_whole() {
    with_relay(|r| async move {
        installed(&r).await;
        r.app
            .write(
                "insert into leases (id, name) \
                 select g, 'Flat ' || g from generate_series(1, 5) g",
            )
            .await;
        assert_eq!(r.waiting(None).await, 5);
        let limits = Limits {
            batch_rows: 2,
            batches_per_writer: 1,
            ..Limits::default()
        };

        assert_eq!(r.pass_with(&limits).await, stats(5, 1));
        let ids = r.changes("leases", "id");
        let want: Vec<Vec<Option<String>>> =
            (1..=5).map(|id| vec![text(&id.to_string())]).collect();
        assert_eq!(ids, want);
        assert_eq!(
            r.aggregate("leases", "count(DISTINCT _txid)"),
            text("1"),
            "one source transaction"
        );
        assert_eq!(r.waiting(None).await, 0);
        assert_caught_up(&r).await;
    })
    .await;
}
