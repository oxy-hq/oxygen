//! Resync (design §5.5): a held table's unshipped rows are discarded and a
//! baseline takes their place; an unheld table keeps its rows; a writer past
//! the relay's ceiling is baselined whole; it is refused with the flag off,
//! and for a table that is not captured.

use oxy_api_custom_apps::oltp_changes::relay::holds::{self, HoldKey, HoldKind};
use oxy_api_custom_apps::oltp_changes::relay::{COPY_BACKLOG, COPY_CHUNK_ROWS, Limits};
use oxy_api_custom_apps::oltp_changes::resync::{
    Discarded, ResyncError, ResyncReport, resync, resync_with,
};

use super::guards::disable_oltp_changes;
use super::{PRODUCTION, Relay, text, with_relay};
use crate::oltp_changes_fixture::{LEASES_SQL, NOTES_SQL};

/// A ceiling of four rows.
const LIMITS: Limits = Limits {
    batch_rows: 4,
    batch_ceiling: 4,
    batches_per_writer: 5,
    budget: std::time::Duration::from_secs(60),
    copy_chunk_rows: COPY_CHUNK_ROWS,
    copy_backlog: COPY_BACKLOG,
};

impl Relay {
    async fn resync(&self, table: Option<&str>) -> Result<ResyncReport, ResyncError> {
        resync(self.db(), self.app.org(), &self.app.slug, table).await
    }

    /// `leases` captured and settled.
    async fn with_leases(&self) {
        self.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        self.settle().await;
    }
}

/// Hold `leases` on its own.
async fn hold_leases(r: &Relay) {
    let writer = r.writer();
    let key = HoldKey {
        org_id: r.app.org(),
        environment: PRODUCTION,
        writer: &writer,
        table: Some("leases"),
        kind: HoldKind::Table,
    };
    holds::hold(r.db(), key, "held by the test")
        .await
        .expect("hold leases");
}

/// Two rows of `leases`, in separate transactions.
async fn insert_two(r: &Relay) {
    for id in 1..=2 {
        let sql = format!("insert into leases (id, name) values ({id}, 'Flat {id}')");
        r.app.write(&sql).await;
    }
}

/// A held table's unshipped rows are removed, its hold lifted, the relay
/// nudged; the rows come back from the baseline's snapshot.
#[tokio::test]
async fn resync_of_a_held_table_discards_its_unshipped_rows_and_releases_it() {
    with_relay(|r| async move {
        r.with_leases().await;
        hold_leases(&r).await;
        insert_two(&r).await;
        r.pass().await;
        assert_eq!(r.waiting(None).await, 2, "the rows wait, held");
        let nudged = r.nudged_at().await;

        let report = r.resync(Some("leases")).await.expect("the resync");

        let expected = ResyncReport {
            writer: r.writer(),
            requested: vec!["leases".to_owned()],
            released: vec!["leases".to_owned()],
            discarded: vec![Discarded {
                table: "leases".to_owned(),
                rows: 2,
            }],
        };
        assert_eq!(report, expected);
        assert!(r.holds().await.is_empty(), "the hold is gone");
        assert!(r.nudged_at().await > nudged, "the relay is nudged");
        r.settle().await;
        assert_eq!(
            r.view("leases"),
            [[text("1"), text("Flat 1")], [text("2"), text("Flat 2")]]
        );
    })
    .await;
}

/// A table nobody held keeps its unshipped rows: they ship, and the
/// snapshot lands beside them.
#[tokio::test]
async fn resync_of_an_unheld_table_keeps_its_unshipped_rows() {
    with_relay(|r| async move {
        r.with_leases().await;
        insert_two(&r).await;

        let report = r.resync(Some("leases")).await.expect("the resync");

        assert!(report.discarded.is_empty());
        assert!(report.released.is_empty());
        r.settle().await;
        let ops = r.changes("leases", "id, _op");
        for id in ["1", "2"] {
            assert!(
                ops.contains(&vec![text(id), text("insert")]),
                "insert {id} shipped: {ops:?}"
            );
        }
    })
    .await;
}

/// A writer past the ceiling is held whole; resyncing one of its tables
/// baselines every table, discards every row waiting, and lifts the hold.
#[tokio::test]
async fn resync_of_a_writer_past_the_ceiling_discards_all_its_rows_and_baselines_every_table() {
    with_relay(|r| async move {
        r.app
            .promote_declaring("chg-1", &["leases", "notes"], &[LEASES_SQL, NOTES_SQL])
            .await
            .expect("the promote installs capture on both tables");
        r.settle().await;
        r.app
            .write(
                "insert into leases (id, name) \
                 select g, 'Flat ' || g from generate_series(1, 5) g",
            )
            .await;
        r.pass_with(&LIMITS).await;
        assert_eq!(r.holds().await.len(), 1, "the writer is held");

        let report = r.resync(Some("leases")).await.expect("the resync");

        let expected = ResyncReport {
            writer: r.writer(),
            requested: vec!["leases".to_owned(), "notes".to_owned()],
            released: vec![String::new()],
            discarded: vec![Discarded {
                table: "leases".to_owned(),
                rows: 5,
            }],
        };
        assert_eq!(report, expected);
        assert!(r.holds().await.is_empty());
        assert_eq!(r.waiting(None).await, 0, "every waiting row is gone");
    })
    .await;
}

#[tokio::test]
async fn resync_is_refused_without_the_flag() {
    with_relay(|r| async move {
        r.with_leases().await;
        disable_oltp_changes(&r.app.t).await;

        let refused = r.resync(Some("leases")).await;

        assert!(matches!(refused, Err(ResyncError::Off)), "{refused:?}");
    })
    .await;
}

#[tokio::test]
async fn resync_of_a_table_not_captured_is_unknown() {
    with_relay(|r| async move {
        r.with_leases().await;

        let refused = r.resync(Some("notes")).await;

        assert!(
            matches!(&refused, Err(ResyncError::UnknownTable(t)) if t == "notes"),
            "{refused:?}"
        );
    })
    .await;
}

/// A held backlog larger than one chunk is discarded whole, and counted.
#[tokio::test]
async fn resync_discards_a_held_backlog_larger_than_a_chunk() {
    with_relay(|r| async move {
        r.with_leases().await;
        hold_leases(&r).await;
        r.app
            .write(
                "insert into leases (id, name) \
                 select g, 'Flat ' || g from generate_series(1, 5) g",
            )
            .await;
        r.pass().await;

        let report = resync_with(r.db(), r.app.org(), &r.app.slug, Some("leases"), 2)
            .await
            .expect("the resync");

        let gone = vec![Discarded {
            table: "leases".to_owned(),
            rows: 5,
        }];
        assert_eq!(report.discarded, gone);
        assert_eq!(r.waiting(None).await, 0);
    })
    .await;
}

/// The final transaction fails after chunks committed: the error still
/// carries what they dropped. A trigger on the log sets `restamping` as the
/// first chunk commits, so the final transaction's re-check refuses.
#[tokio::test]
async fn a_failure_after_committed_chunks_reports_what_they_discarded() {
    with_relay(|r| async move {
        r.with_leases().await;
        hold_leases(&r).await;
        r.app
            .write(
                "insert into leases (id, name) \
                 select g, 'Flat ' || g from generate_series(1, 5) g",
            )
            .await;
        r.pass().await;
        r.app
            .owner()
            .await
            .batch_execute(
                "CREATE FUNCTION oxy_changes.test_restamp() RETURNS trigger LANGUAGE plpgsql \
                 AS $$ BEGIN UPDATE oxy_changes.stream SET restamping = generation; \
                 RETURN NULL; END $$; \
                 CREATE TRIGGER test_restamp AFTER DELETE ON oxy_changes.log \
                 FOR EACH STATEMENT EXECUTE FUNCTION oxy_changes.test_restamp()",
            )
            .await
            .expect("a trigger that re-stamps after a delete");

        let out = resync_with(r.db(), r.app.org(), &r.app.slug, Some("leases"), 2).await;

        let Err(ResyncError::AfterDiscard { discarded, .. }) = out else {
            panic!("expected AfterDiscard, got {out:?}");
        };
        let gone = vec![Discarded {
            table: "leases".to_owned(),
            rows: 5,
        }];
        assert_eq!(discarded, gone);
    })
    .await;
}
