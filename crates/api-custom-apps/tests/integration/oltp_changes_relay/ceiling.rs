//! A batch never passes the relay's ceiling, and never splits a source
//! transaction to stay under it: transactions that pass it together ship one
//! batch each, and one that passes it alone holds its writer, with nothing
//! declared.

use oxy_api_custom_apps::oltp_changes::relay::{COPY_BACKLOG, COPY_CHUNK_ROWS, Limits, RelayStats};

use super::{Relay, text, with_relay};
use crate::oltp_changes_fixture::LEASES_SQL;

/// A ceiling of four rows, a head of four.
const LIMITS: Limits = Limits {
    batch_rows: 4,
    batch_ceiling: 4,
    batches_per_writer: 5,
    budget: std::time::Duration::from_secs(60),
    copy_chunk_rows: COPY_CHUNK_ROWS,
    copy_backlog: COPY_BACKLOG,
};

/// `leases` captured, then one transaction inserting `ids`.
async fn insert(r: &Relay, ids: std::ops::RangeInclusive<i64>) {
    let sql = format!(
        "insert into leases (id, name) select g, 'Flat ' || g from generate_series({}, {}) g",
        ids.start(),
        ids.end()
    );
    r.app.write(&sql).await;
}

/// Two transactions of three rows: a head of four takes both, six rows, past
/// the ceiling. The read is cut back to the oldest alone, and the pass ships
/// the two as two batches, each whole.
#[tokio::test]
async fn transactions_past_the_ceiling_together_ship_one_at_a_time() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.settle().await;
        insert(&r, 1..=3).await;
        insert(&r, 4..=6).await;
        assert_eq!(r.waiting(None).await, 6);

        let shipped = RelayStats {
            shipped: 6,
            batches: 2,
            ..RelayStats::default()
        };
        assert_eq!(r.pass_with(&LIMITS).await, shipped);
        assert_eq!(r.aggregate("leases", "count(DISTINCT _txid)"), text("2"));
        assert_eq!(r.waiting(None).await, 0);
        assert!(r.holds().await.is_empty());
    })
    .await;
}

/// One transaction of five rows, past a ceiling of four: nothing ships,
/// nothing is declared, nothing more is made in Airhouse than the install's
/// baseline made, its rows stay in the log, and the whole writer is held with
/// the transaction's size, never its values.
#[tokio::test]
async fn a_transaction_past_the_ceiling_holds_its_writer() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.settle().await;
        let tables = r.tables();
        insert(&r, 1..=5).await;
        let before = r.progress().await;

        let held = RelayStats {
            held: 5,
            ..RelayStats::default()
        };
        assert_eq!(r.pass_with(&LIMITS).await, held);
        assert_eq!(r.waiting(None).await, 5, "the rows stay in the log");
        assert_eq!(r.progress().await, before, "no batch declared");
        assert_eq!(r.tables(), tables, "nothing made in Airhouse");
        let holds = r.holds().await;
        let reason = "a transaction of 5 rows exceeds the relay's ceiling of 4 (resync after PR 3)";
        assert_eq!(holds, [(r.writer(), String::new(), reason.to_owned())]);
    })
    .await;
}
