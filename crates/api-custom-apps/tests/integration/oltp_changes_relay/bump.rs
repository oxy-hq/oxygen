//! A detected restore bumps the store's generation (design §5.6): a store
//! rewound under Airhouse bumps once, past Airhouse's generation, and ships
//! and baselines again in the new one; a re-stamp an earlier pass left set is
//! finished; a row an older generation left after a bump is re-stamped and
//! ships; a writer PR 2 held for a restore is checked again; and a bump
//! behind a long transaction backs off, failing only its writer, and tries
//! no bump until its retry.

use oxy_api_custom_apps::oltp_changes::relay::Limits;
use oxy_api_custom_apps::oltp_changes::relay::holds::{self, HoldKey, HoldKind};
use oxy_api_custom_apps::oltp_changes::relay::render::{BASELINES_TABLE, progress_write_sql};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::{PRODUCTION, Relay, text, with_relay};
use crate::oltp_changes_fixture::{LEASES_SQL, connect};

impl Relay {
    /// The stream's `(generation, restamping)`.
    async fn stream(&self) -> (i64, Option<i64>) {
        let row = self
            .app
            .owner()
            .await
            .query_one("SELECT generation, restamping FROM oxy_changes.stream", &[])
            .await
            .expect("read the stream");
        (row.get(0), row.get(1))
    }

    /// Capture installed on `leases` and settled, row 1 written and shipped:
    /// the store's state before a restore.
    async fn shipped_one(&self) {
        self.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        self.settle().await;
        self.app
            .write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        assert_eq!(self.pass().await.shipped, 1);
    }

    /// [`Relay::shipped_one`], then Airhouse's `_progress` set past the
    /// store's intent, as when the store is restored to before a batch
    /// Airhouse holds (in `generation`, or the one it shipped), and row 2
    /// written. The stream's generation before the restore is found.
    async fn rewound(&self, generation: Option<i64>) -> i64 {
        self.shipped_one().await;
        let (intent, _) = self.progress().await;
        let generation = generation.unwrap_or(self.shipped().0);
        self.duck.exec(&progress_write_sql(
            &self.writer(),
            generation,
            intent + 100,
        ));
        self.app
            .write("insert into leases (id, name) values (2, 'Flat 2')")
            .await;
        self.stream().await.0
    }

    /// `table`'s markers of `generation` in `_baselines`, by `seq`.
    fn markers_of(&self, table: &str, generation: i64) -> Vec<String> {
        let from = self.qualified(BASELINES_TABLE);
        let rows = self.duck.read(&format!(
            "SELECT marker FROM {from} \
              WHERE table_name = '{table}' AND generation = {generation} ORDER BY seq"
        ));
        rows.into_iter()
            .map(|row| row[0].clone().expect("a marker"))
            .collect()
    }

    /// The control plane's generation row: the last generation handed out,
    /// the bump's attempts, and whether its retry is still ahead.
    async fn bump_backoff(&self) -> (i64, i32, bool) {
        let row = self
            .db()
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT generation, bump_attempts, \
                        coalesce(bump_retry_at > now(), false) AS ahead \
                   FROM oltp_change_generations WHERE org_id = $1 AND environment = $2",
                [self.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("read the generation row")
            .expect("the generation row");
        (
            row.try_get("", "generation").expect("generation"),
            row.try_get("", "bump_attempts").expect("bump_attempts"),
            row.try_get("", "ahead").expect("ahead"),
        )
    }

    /// Run `sql` against the org's production rows, `$1` the org and `$2`
    /// the environment.
    async fn control(&self, sql: &str) {
        self.db()
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                [self.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    /// Whether the production mark has a wake set.
    async fn woken(&self) -> bool {
        let row = self
            .db()
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT wake_at IS NOT NULL AS woken FROM oltp_change_relays \
                  WHERE org_id = $1 AND environment = $2",
                [self.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("read the relay's mark")
            .expect("the mark");
        row.try_get("", "woken").expect("woken")
    }

    /// How many changes of `leases` row `id`, not snapshots, shipped under
    /// `generation`.
    fn changes_of(&self, id: i64, generation: i64) -> Option<String> {
        let changes = format!(
            "count(*) FILTER (WHERE id = {id} AND _op <> 'snapshot' AND _generation = {generation})"
        );
        self.aggregate("leases", &changes)
    }
}

/// Airhouse's `_progress` set past the store's intent: the next pass bumps
/// the generation past both, holds nothing, re-stamps the waiting row, and
/// restarts the batches. Settled, row 2 ships under the new generation, a
/// `bump` baseline completes in it, and the view shows both rows. The pass
/// after does not bump again.
#[tokio::test]
async fn a_store_rewound_under_airhouse_bumps_once_and_ships() {
    with_relay(|r| async move {
        let old = r.rewound(None).await;
        let airhouse = r.shipped().0;

        let stats = r.pass().await;
        assert!(stats.bumped, "{stats:?}");
        let holds = r.holds().await;
        assert!(holds.is_empty(), "no hold made: {holds:?}");
        let (new, restamping) = r.stream().await;
        assert!(new > old && new > airhouse, "{old}, {airhouse} -> {new}");
        assert_eq!(restamping, None, "re-stamped within the pass");
        assert_eq!(r.progress().await, (0, 0), "batches start again");

        r.settle().await;
        assert_eq!(r.changes_of(2, new), text("1"), "row 2, in the new one");
        let markers = r.markers_of("leases", new);
        assert!(markers.contains(&"complete".to_owned()), "{markers:?}");
        assert_eq!(
            r.view("leases"),
            [[text("1"), text("Flat 1")], [text("2"), text("Flat 2")]]
        );

        assert!(!r.pass().await.bumped, "bumped once");
    })
    .await;
}

/// Airhouse's `_progress` in generation 50, past the store's: `Rewound`, and
/// the bump goes past it.
#[tokio::test]
async fn generations_outrun_airhouse() {
    with_relay(|r| async move {
        r.rewound(Some(50)).await;

        assert!(r.pass().await.bumped);
        let (generation, _) = r.stream().await;
        assert!(generation >= 51, "{generation}");
    })
    .await;
}

/// A bump that committed (the stream moved, the batches restarted) and left
/// its re-stamp unfinished, with a row waiting in the old generation: the next pass finishes the re-stamp, then
/// ships the row under the new generation.
#[tokio::test]
async fn a_pass_that_finds_restamping_set_finishes_it() {
    with_relay(|r| async move {
        r.shipped_one().await;
        r.app
            .write("insert into leases (id, name) values (2, 'Flat 2')")
            .await;
        r.app
            .owner()
            .await
            .batch_execute(
                "UPDATE oxy_changes.stream SET restamping = generation, generation = generation + 1; \
                 UPDATE oxy_changes.progress SET intent_batch = 0, shipped_batch = 0",
            )
            .await
            .expect("leave a bump re-stamping");
        let (new, restamping) = r.stream().await;
        assert_eq!(restamping, Some(new - 1));

        r.pass().await;
        assert_eq!(r.stream().await, (new, None), "the re-stamp finished");
        assert_eq!(r.changes_of(2, new), text("1"), "shipped in the new one");
    })
    .await;
}

/// A writer held for a restore as PR 2 held it, with Airhouse still ahead:
/// the pass checks it again rather than skipping it, bumps, and lifts the
/// hold.
#[tokio::test]
async fn a_writer_held_for_a_restore_by_pr2_is_checked_again() {
    with_relay(|r| async move {
        r.rewound(None).await;
        let writer = r.writer();
        let key = HoldKey {
            org_id: r.app.org(),
            environment: PRODUCTION,
            writer: &writer,
            table: None,
            kind: HoldKind::Restore,
        };
        holds::hold(
            r.db(),
            key,
            "restore detected (Rewound): a generation bump is needed (PR 3)",
        )
        .await
        .expect("hold the writer as PR 2 did");

        assert!(r.pass().await.bumped, "checked again, not skipped");
        let holds = r.holds().await;
        assert!(holds.is_empty(), "the hold is gone: {holds:?}");
    })
    .await;
}

/// A completed bump, then a row left in an older generation, as a
/// transaction that spanned the bump commits it: the next pass re-stamps it
/// into the stream's generation and ships it.
#[tokio::test]
async fn a_row_of_an_older_generation_is_restamped_and_ships() {
    with_relay(|r| async move {
        r.rewound(None).await;
        assert!(r.pass().await.bumped);
        r.settle().await;
        let (new, _) = r.stream().await;
        r.app
            .write("insert into leases (id, name) values (3, 'Flat 3')")
            .await;
        r.app
            .owner()
            .await
            .execute(
                "UPDATE oxy_changes.log SET generation = $1 WHERE writer = $2",
                &[&(new - 1), &r.writer()],
            )
            .await
            .expect("leave the row in an older generation");

        r.pass().await;
        assert_eq!(r.waiting(None).await, 0, "re-stamped, then shipped");
        assert_eq!(r.changes_of(3, new), text("1"), "in the stream's one");
    })
    .await;
}

/// A writer's session holds open a transaction that captured a row, with
/// Airhouse ahead: the bump's lock times out, which fails only the writer
/// (the pass goes on, and names it) and backs the bump off, the stream
/// unchanged and the mark woken for the retry. The next pass, at once,
/// tries no bump: no generation handed out, no attempt counted. With the
/// transaction committed and the retry due, a pass bumps and ends the
/// backoff.
#[tokio::test]
async fn a_bump_behind_a_long_transaction_backs_off_and_fails_only_its_writer() {
    with_relay(|r| async move {
        r.rewound(None).await;
        let long = connect(&r.app.writer().await.dsn).await;
        long.batch_execute("BEGIN; INSERT INTO leases (id, name) VALUES (3, 'Flat 3')")
            .await
            .expect("capture a row in a long transaction");
        let before = r.stream().await;

        let failed = r.try_pass(PRODUCTION, &Limits::default()).await;
        let error = failed.expect_err("the bump cannot take the stream's lock");
        let timed_out = format!("failed for {}: the generation bump timed out", r.writer());
        assert!(
            error.contains(&timed_out),
            "only the writer failed: {error}"
        );
        assert_eq!(r.backoff().await, (1, true, true));
        assert_eq!(r.stream().await, before, "the stream is unchanged");
        let backed_off = r.bump_backoff().await;
        assert_eq!((backed_off.1, backed_off.2), (1, true), "{backed_off:?}");
        assert!(r.woken().await, "the mark wakes for the retry");

        let again = r.try_pass(PRODUCTION, &Limits::default()).await;
        let error = again.expect_err("the writer fails while the bump backs off");
        assert!(error.contains("backs off"), "{error}");
        assert_eq!(r.bump_backoff().await, backed_off, "no bump tried");

        long.batch_execute("COMMIT")
            .await
            .expect("commit the long transaction");
        r.control(
            "UPDATE oltp_change_generations SET bump_retry_at = now() - interval '1 second' \
              WHERE org_id = $1 AND environment = $2",
        )
        .await;
        assert!(r.pass().await.bumped, "the retry bumps");
        let (_, attempts, ahead) = r.bump_backoff().await;
        assert_eq!((attempts, ahead), (0, false), "the backoff is over");
    })
    .await;
}
