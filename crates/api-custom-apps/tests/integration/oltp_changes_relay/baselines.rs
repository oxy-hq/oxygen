//! Baselines (design §5.5) through the relay: an install's baseline copies
//! the rows already there into the change stream, and a reshape's carries a
//! backfill its migration made; a copy paced by its writer's backlog resumes
//! at its cursor, never copying a row twice; writes during a copy win over
//! its snapshots; markers a dying pass committed land once; a held table's
//! baseline waits without keeping the mark due; a begin behind a long
//! transaction backs its baseline off, not the writer, and wakes the mark at
//! its retry; a view Airhouse refuses is left pending while the batch ships,
//! and made by a later pass; a view whose bookkeeping fails never stops its
//! batch.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;

use oxy_api_custom_apps::oltp_changes::relay::holds::{self, HoldKey};
use oxy_api_custom_apps::oltp_changes::relay::marks::due;
use oxy_api_custom_apps::oltp_changes::relay::render::changes_table_name;
use oxy_api_custom_apps::oltp_changes::relay::sink::install_sink;
use oxy_api_custom_apps::oltp_changes::relay::{Limits, REDRAIN_EVERY, REDRAIN_MARGIN, RelayStats};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::duck::{DiesAtFirstView, FailsOnceAfterCommit, LOST_AFTER_COMMIT, RefusesViews};
use super::{PRODUCTION, Relay, text, with_relay};
use crate::oltp_changes_fixture::{LEASES_SQL, connect};

/// The production mark, as a pass settled it.
#[derive(Debug)]
struct Mark {
    drained: bool,
    /// Whether `wake_at` is set, and still ahead.
    woken_later: Option<bool>,
    failures: i32,
}

/// The first migration: `leases`, with two rows written before capture is
/// installed, so no change of theirs is logged.
const LEASES_AB_SQL: (&str, &[u8]) = (
    "migrations/0001_leases.sql",
    b"create table leases (id bigint primary key, name text); \
      insert into leases (id, name) values (1, 'A'), (2, 'B');",
);

/// The first migration: `leases`, with five rows (`Flat 1` to `Flat 5`)
/// written before capture is installed.
const LEASES_FIVE_SQL: (&str, &[u8]) = (
    "migrations/0001_leases.sql",
    b"create table leases (id bigint primary key, name text); \
      insert into leases (id, name) select g, 'Flat ' || g from generate_series(1, 5) g;",
);

/// The next migration: a column added to `leases` and backfilled in the same
/// file.
const REGION_SQL: (&str, &[u8]) = (
    "migrations/0002_region.sql",
    b"alter table leases add column region text; update leases set region = 'north';",
);

/// Two rows a chunk, and copies paused at two unshipped rows.
fn paced() -> Limits {
    Limits {
        copy_chunk_rows: 2,
        copy_backlog: 2,
        ..Limits::default()
    }
}

/// A baseline of `leases`, as [`Relay::baselines`] reads it.
fn leases(reason: &str, state: &str) -> (String, String, String) {
    ("leases".to_owned(), reason.to_owned(), state.to_owned())
}

/// `(marker, begin_seq)` of one baseline's begin and complete.
fn begun_and_completed(begin_seq: i64) -> [(String, i64); 2] {
    [
        ("begin".to_owned(), begin_seq),
        ("complete".to_owned(), begin_seq),
    ]
}

impl Relay {
    /// The keys of the writer's snapshot rows still in the log, as `jsonb`
    /// text, by `seq`.
    async fn snapshots_waiting(&self) -> Vec<String> {
        let rows = self
            .app
            .owner()
            .await
            .query(
                "SELECT key::text FROM oxy_changes.log \
                  WHERE writer = $1 AND op = 's' ORDER BY seq",
                &[&self.writer()],
            )
            .await
            .expect("read the writer's snapshot rows");
        rows.iter().map(|row| row.get(0)).collect()
    }

    /// `table`'s open baseline's backoff: its `attempts`, and whether its
    /// `retry_at` is still ahead.
    async fn backoff_of(&self, table: &str) -> (i32, bool) {
        let row = self
            .app
            .owner()
            .await
            .query_one(
                "SELECT attempts, coalesce(retry_at > now(), false) FROM oxy_changes.baselines \
                  WHERE writer = $1 AND table_name = $2 AND state IN ('requested', 'copying')",
                &[&self.writer(), &table],
            )
            .await
            .expect("read the baseline's backoff");
        (row.get(0), row.get(1))
    }

    /// Run `sql` against `table`'s open baseline, as the store's owner.
    async fn set_baseline(&self, table: &str, set: &str) {
        let sql = format!(
            "UPDATE oxy_changes.baselines SET {set} \
              WHERE writer = $1 AND table_name = $2 AND state IN ('requested', 'copying')"
        );
        self.app
            .owner()
            .await
            .execute(sql.as_str(), &[&self.writer(), &table])
            .await
            .expect("update the baseline");
    }

    /// The production mark.
    async fn mark(&self) -> Mark {
        let row = self
            .db()
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT drain_started_at IS NOT NULL AS drained, wake_at > now() AS woken_later, failures \
                   FROM oltp_change_relays WHERE org_id = $1 AND environment = $2",
                [self.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("read the relay's mark")
            .expect("the mark");
        Mark {
            drained: row.try_get("", "drained").expect("drained"),
            woken_later: row.try_get("", "woken_later").expect("woken_later"),
            failures: row.try_get("", "failures").expect("failures"),
        }
    }

    /// Whether the sweep would queue a pass for the production mark.
    async fn is_due(&self) -> bool {
        due(self.db(), REDRAIN_MARGIN, REDRAIN_EVERY)
            .await
            .expect("due")
            .iter()
            .any(|d| d.org_id == self.app.org() && d.environment == PRODUCTION)
    }

    /// The writer's baselines whose current-state view waits to be made.
    async fn pending_views(&self) -> i64 {
        self.app
            .owner()
            .await
            .query_one(
                "SELECT count(*) FROM oxy_changes.baselines WHERE writer = $1 AND view_pending",
                &[&self.writer()],
            )
            .await
            .expect("count the pending views")
            .get(0)
    }

    /// Whether a pass drained the production mark.
    async fn drained(&self) -> bool {
        let row = self
            .db()
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT drain_started_at IS NOT NULL AS drained FROM oltp_change_relays \
                  WHERE org_id = $1 AND environment = $2",
                [self.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("read the relay's mark");
        row.is_some_and(|row| row.try_get("", "drained").expect("drained"))
    }
}

/// Two rows written before the install: the install's baseline copies them
/// into the stream as snapshots, signed as the platform, between a begin and
/// a complete marker, and the view shows them.
#[tokio::test]
async fn an_opt_in_baselines_the_rows_already_there() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_AB_SQL])
            .await
            .expect("the promote installs capture on leases");
        assert_eq!(r.waiting(None).await, 0, "nothing captured before it");

        r.settle().await;
        let markers = r.markers("leases");
        let begin_seq = markers.first().expect("a begin marker").1;
        assert_eq!(markers, begun_and_completed(begin_seq));
        let platform = "count(*) FILTER (WHERE _op = 'snapshot' AND _actor = 'platform')";
        assert_eq!(r.aggregate("leases", platform), text("2"), "signed");
        assert_eq!(r.aggregate("leases", "count(*)"), text("2"));
        assert_eq!(
            r.view("leases"),
            [[text("1"), text("A")], [text("2"), text("B")]]
        );
        assert_eq!(r.waiting(None).await, 0, "the log is empty");
        assert_eq!(r.baselines().await, [leases("opt_in", "complete")]);
    })
    .await;
}

/// The acceptance test for the reshape baseline (design §5.5, review F14):
/// `leases` captured and two rows written, then a promote whose migration
/// adds `region` and backfills it while the triggers still bake in the old
/// shape, so the backfill logs nothing. The reshape's baseline copies the
/// rows as they stand, so every live row's latest change is a snapshot
/// carrying `region`, and the view shows it.
#[tokio::test]
async fn a_backfill_in_the_migration_that_adds_a_column_reaches_the_change_stream() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.settle().await;
        r.app
            .write("insert into leases (id, name) values (1, 'Flat 1'), (2, 'Flat 2')")
            .await;
        r.app
            .promote("chg-2", true, &[LEASES_SQL, REGION_SQL])
            .await
            .expect("the backfill applies and the install reshapes leases");

        r.settle().await;
        let changes = r.qualified(&changes_table_name("leases"));
        let latest = r.duck.read(&format!(
            "SELECT id, _op, region FROM {changes} \
             QUALIFY row_number() OVER (PARTITION BY id ORDER BY _seq DESC) = 1 ORDER BY id"
        ));
        let snapshot = |id: &str| [text(id), text("snapshot"), text("north")];
        assert_eq!(latest, [snapshot("1"), snapshot("2")]);
        assert_eq!(
            r.view("leases"),
            [
                [text("1"), text("Flat 1"), text("north")],
                [text("2"), text("Flat 2"), text("north")],
            ]
        );
        let baselines = [leases("opt_in", "complete"), leases("reshape", "complete")];
        assert_eq!(r.baselines().await, baselines);
    })
    .await;
}

/// Five rows, copied two a chunk while fewer than two rows wait to ship. The
/// first pass begins the baseline, and its `b` marker counts toward the
/// backlog, so one chunk of one row follows and the copy pauses, still
/// `copying`. Each later pass ships what waits, then copies on from the
/// cursor: every row is copied once, and the baseline completes.
#[tokio::test]
async fn a_copy_paced_by_its_backlog_resumes_at_its_cursor() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_FIVE_SQL])
            .await
            .expect("the promote installs capture on leases");

        let first = r.pass_with(&paced()).await;
        assert_eq!((first.begun, first.copied, first.completed), (1, 1, 0));
        assert_eq!(r.snapshots_waiting().await, [r#"{"id": 1}"#]);
        assert_eq!(r.baselines().await, [leases("opt_in", "copying")]);

        r.settle_with(&paced()).await;
        let snapshots = "count(DISTINCT id) FILTER (WHERE _op = 'snapshot')";
        assert_eq!(r.aggregate("leases", snapshots), text("5"));
        let copies = "count(*) FILTER (WHERE _op = 'snapshot')";
        assert_eq!(r.aggregate("leases", copies), text("5"), "no chunk twice");
        assert_eq!(r.baselines().await, [leases("opt_in", "complete")]);
    })
    .await;
}

/// The copy paused after `id` 1; then `id` 1 (copied) and `id` 5 (not yet)
/// are updated. The update of a copied row outranks its snapshot, and the
/// snapshot of a row not yet copied reads its update: the view shows both.
#[tokio::test]
async fn writes_during_the_copy_win_over_its_snapshots() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_FIVE_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.pass_with(&paced()).await;
        assert_eq!(r.snapshots_waiting().await, [r#"{"id": 1}"#]);

        r.app
            .write("update leases set name = 'Copied, then changed' where id = 1")
            .await;
        r.app
            .write("update leases set name = 'Changed, then copied' where id = 5")
            .await;
        r.settle_with(&paced()).await;
        let row = |id: &str, name: &str| [text(id), text(name)];
        assert_eq!(
            r.view("leases"),
            [
                row("1", "Copied, then changed"),
                row("2", "Flat 2"),
                row("3", "Flat 3"),
                row("4", "Flat 4"),
                row("5", "Changed, then copied"),
            ]
        );
    })
    .await;
}

/// The pass that ships a baseline's `b`, `s` and `c` rows commits them to
/// Airhouse and dies before its confirm. The next pass ships them again
/// under a new batch, and the anti-joins land each once: one begin and one
/// complete in `_baselines`, each snapshot's `_seq` once.
#[tokio::test]
async fn a_pass_that_dies_after_committing_markers_lands_them_once() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_AB_SQL])
            .await
            .expect("the promote installs capture on leases");
        let copied = r.pass().await;
        assert_eq!((copied.begun, copied.copied, copied.completed), (1, 2, 1));
        assert_eq!(r.waiting(None).await, 4, "b, two s and c wait");

        install_sink(Arc::new(FailsOnceAfterCommit::new(r.duck.clone())));
        let died = r
            .try_pass(PRODUCTION, &Limits::default())
            .await
            .expect_err("the pass fails after the commit");
        assert!(died.contains(LOST_AFTER_COMMIT), "{died}");
        assert_eq!(r.waiting(None).await, 4, "the log keeps them");

        assert_eq!(r.pass().await.shipped, 4, "shipped again");
        assert_eq!(r.waiting(None).await, 0);
        let markers = r.markers("leases");
        let begin_seq = markers.first().expect("a begin marker").1;
        assert_eq!(markers, begun_and_completed(begin_seq), "each once");
        let once = "count(DISTINCT _seq) FILTER (WHERE _op = 'snapshot')";
        assert_eq!(r.aggregate("leases", once), text("2"));
        let all = "count(*) FILTER (WHERE _op = 'snapshot')";
        assert_eq!(r.aggregate("leases", all), text("2"));
    })
    .await;
}

/// `leases` held before the install: its baseline is requested, and a pass
/// neither begins nor copies it, and leaves nothing behind it, so the mark
/// drains; the hold's removal nudges.
#[tokio::test]
async fn a_held_tables_baseline_waits() {
    with_relay(|r| async move {
        let writer = r.writer();
        let key = HoldKey {
            org_id: r.app.org(),
            environment: PRODUCTION,
            writer: &writer,
            table: Some("leases"),
        };
        holds::hold(r.db(), key, "held by the test")
            .await
            .expect("hold leases");
        r.app
            .promote("chg-1", true, &[LEASES_AB_SQL])
            .await
            .expect("the promote installs capture on leases");
        assert!(!r.drained().await, "never drained yet");

        assert_eq!(r.pass().await, RelayStats::default(), "nothing begun");
        assert_eq!(r.baselines().await, [leases("opt_in", "requested")]);
        assert_eq!(r.waiting(None).await, 0, "nothing written to the log");
        assert!(r.drained().await, "no rows left: the mark drains");
    })
    .await;
}

/// A transaction holds `leases` in `ROW EXCLUSIVE` mode, as a long app write
/// would, so the begin's `SHARE` lock times out. The pass still ships the
/// writer's change and does not fail: the baseline alone backs off, with one
/// attempt counted and its retry ahead. The pass settles the mark as it
/// would anyway, drained with no failure counted, and wakes it at the retry:
/// due once that passes. A next pass, with the retry an hour ahead, does not
/// try the begin again.
#[tokio::test]
async fn a_begin_behind_a_long_transaction_backs_off_its_baseline_not_the_writer() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_AB_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.app
            .write("insert into leases (id, name) values (3, 'C')")
            .await;
        let long = connect(&r.app.writer().await.dsn).await;
        long.batch_execute("BEGIN; LOCK TABLE leases IN ROW EXCLUSIVE MODE")
            .await
            .expect("hold leases in a long transaction");

        let first = r.pass().await;
        assert_eq!(first.shipped, 1, "the writer's change still ships");
        assert_eq!(first.begun, 0, "the begin could not take its lock");
        assert_eq!(r.backoff_of("leases").await, (1, true));
        assert_eq!(r.baselines().await, [leases("opt_in", "requested")]);
        let mark = r.mark().await;
        assert!(mark.drained, "settled as it would be anyway: {mark:?}");
        assert_eq!(mark.woken_later, Some(true), "woken at its retry");
        assert_eq!(mark.failures, 0, "no failure counted");
        r.db()
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE oltp_change_relays SET wake_at = now() - interval '1 second' \
                  WHERE org_id = $1 AND environment = $2",
                [r.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("move the wake back");
        assert!(r.is_due().await, "due once the wake passes");

        r.set_baseline("leases", "retry_at = now() + interval '1 hour'")
            .await;
        let second = r.pass().await;
        assert_eq!(second, RelayStats::default(), "nothing tried");
        assert_eq!(r.backoff_of("leases").await, (1, true), "not tried again");

        long.batch_execute("ROLLBACK")
            .await
            .expect("end the long transaction");
    })
    .await;
}

/// Two rows written before the install, and a first pass that begins, copies
/// and completes their baseline; then Airhouse refuses the next `refusals`
/// views. The installed sink.
async fn refused_views(r: &Relay, refusals: usize) -> Arc<RefusesViews> {
    r.app
        .promote("chg-1", true, &[LEASES_AB_SQL])
        .await
        .expect("the promote installs capture on leases");
    let copied = r.pass().await;
    assert_eq!((copied.begun, copied.completed), (1, 1));
    let sink = Arc::new(RefusesViews::new(r.duck.clone(), refusals));
    install_sink(sink.clone());
    sink
}

/// The pass that ships the baseline's `b`, `s` and `c` rows makes the view
/// after its write; Airhouse refuses it, and again when the pass tries the
/// pending view after its shipping. The pass does not fail, the batch ships
/// whole, and the view is recorded pending in the store.
#[tokio::test]
async fn a_refused_view_is_left_pending_and_the_batch_still_ships() {
    with_relay(|r| async move {
        let sink = refused_views(&r, 2).await;

        let shipped = r.pass().await;
        assert_eq!(shipped.shipped, 4, "b, two s and c ship");
        assert_eq!(r.waiting(None).await, 0);
        assert_eq!(sink.views(), 2, "the batch's, then the pending one's");
        assert_eq!(r.pending_views().await, 1, "recorded on the baseline");
        assert!(
            !r.tables().contains(&"leases".to_owned()),
            "no view made: {:?}",
            r.tables()
        );
        let markers = r.markers("leases");
        assert_eq!(markers.len(), 2, "the markers landed: {markers:?}");
    })
    .await;
}

/// A view refused at its `c` batch is made by the next pass, though its
/// table ships nothing then, and its flag clears; a pass after that, with
/// nothing pending, sends no view at all.
#[tokio::test]
async fn a_pending_view_is_made_again_by_the_next_pass() {
    with_relay(|r| async move {
        let sink = refused_views(&r, 2).await;
        r.pass().await;
        assert_eq!(r.pending_views().await, 1);

        let next = r.pass().await;
        assert_eq!(next.shipped, 0, "nothing to ship");
        assert_eq!(sink.views(), 3, "the pending view, made");
        assert_eq!(r.pending_views().await, 0, "the flag clears");
        assert_eq!(
            r.view("leases"),
            [[text("1"), text("A")], [text("2"), text("B")]]
        );

        r.pass().await;
        assert_eq!(sink.views(), 3, "nothing pending, no view sent");
    })
    .await;
}

/// The store's view bookkeeping fails (its `view_pending` column gone): the
/// batch that ships the baseline is still confirmed and deleted, the view is
/// made, and the pass does not fail.
#[tokio::test]
async fn a_view_whose_bookkeeping_fails_still_confirms_its_batch() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_AB_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.pass().await;
        r.app
            .owner()
            .await
            .batch_execute("ALTER TABLE oxy_changes.baselines DROP COLUMN view_pending")
            .await
            .expect("break the view bookkeeping");

        assert_eq!(r.pass().await.shipped, 4, "b, two s and c ship");
        assert_eq!(r.waiting(None).await, 0, "and are deleted");
        let (intent, shipped) = r.progress().await;
        assert_eq!(intent, shipped, "confirmed");
        assert_eq!(
            r.view("leases"),
            [[text("1"), text("A")], [text("2"), text("B")]]
        );
    })
    .await;
}

/// The pass that ships the baseline's `c` dies between its confirm and its
/// view. The batch is confirmed and deleted, and its view was flagged pending
/// with the confirm, so the next pass makes it and clears the flag.
#[tokio::test]
async fn a_view_the_pass_died_before_is_made_by_the_next_pass() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_AB_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.pass().await;
        install_sink(Arc::new(DiesAtFirstView::new(r.duck.clone())));

        let died = AssertUnwindSafe(r.try_pass(PRODUCTION, &Limits::default()))
            .catch_unwind()
            .await;
        assert!(died.is_err(), "the pass died at its view");
        assert_eq!(
            r.waiting(None).await,
            0,
            "the batch was confirmed and deleted"
        );
        assert_eq!(r.pending_views().await, 1, "flagged with the confirm");
        r.db()
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE oltp_change_relays SET lease_expires_at = now() - interval '1 second' \
                  WHERE org_id = $1 AND environment = $2",
                [r.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("let the dead pass's lease lapse");

        r.pass().await;
        assert_eq!(r.pending_views().await, 0, "made, and cleared");
        assert_eq!(
            r.view("leases"),
            [[text("1"), text("A")], [text("2"), text("B")]]
        );
    })
    .await;
}
