//! What an install's baseline leaves, and passes until it is done: the
//! [`Relay`] helpers every test that installs capture needs
//! ([`Relay::settle`]), and the reads of a baseline's store rows, its
//! markers and its current-state view.

use oxy_api_custom_apps::oltp_changes::relay::Limits;
use oxy_api_custom_apps::oltp_changes::relay::render::BASELINES_TABLE;

use super::Relay;

/// The most passes [`Relay::settle`] takes.
const SETTLE_PASSES: usize = 6;

impl Relay {
    /// Passes under the default [`Limits`] until no baseline is open and no
    /// row of the writer waits in the log: what an install leaves behind,
    /// shipped.
    pub(super) async fn settle(&self) {
        self.settle_with(&Limits::default()).await;
    }

    /// [`Relay::settle`] under `limits`, in at most [`SETTLE_PASSES`] passes.
    pub(super) async fn settle_with(&self, limits: &Limits) {
        for _ in 0..SETTLE_PASSES {
            if self.open_baselines().await == 0 && self.waiting(None).await == 0 {
                return;
            }
            self.pass_with(limits).await;
        }
        let left = (self.open_baselines().await, self.waiting(None).await);
        assert_eq!(left, (0, 0), "settled within {SETTLE_PASSES} passes");
    }

    /// `table`'s current-state view, every row by its first column.
    pub(super) fn view(&self, table: &str) -> Vec<Vec<Option<String>>> {
        let from = self.qualified(table);
        self.duck.read(&format!("SELECT * FROM {from} ORDER BY 1"))
    }

    /// `table`'s rows of `_baselines`, as `(marker, begin_seq)` by `seq`.
    pub(super) fn markers(&self, table: &str) -> Vec<(String, i64)> {
        let from = self.qualified(BASELINES_TABLE);
        let rows = self.duck.read(&format!(
            "SELECT marker, begin_seq FROM {from} WHERE table_name = '{table}' ORDER BY seq"
        ));
        rows.into_iter()
            .map(|row| {
                let begin_seq = row[1].as_deref().expect("a begin_seq");
                (
                    row[0].clone().expect("a marker"),
                    begin_seq.parse().expect("a number"),
                )
            })
            .collect()
    }

    /// The writer's baselines in the store that are requested or copying.
    pub(super) async fn open_baselines(&self) -> i64 {
        self.app
            .owner()
            .await
            .query_one(
                "SELECT count(*) FROM oxy_changes.baselines \
                  WHERE writer = $1 AND state IN ('requested', 'copying')",
                &[&self.writer()],
            )
            .await
            .expect("count the writer's open baselines")
            .get(0)
    }

    /// The writer's baselines in the store, as `(table, reason, state)` by
    /// `id`.
    pub(super) async fn baselines(&self) -> Vec<(String, String, String)> {
        let rows = self
            .app
            .owner()
            .await
            .query(
                "SELECT table_name, reason, state FROM oxy_changes.baselines \
                  WHERE writer = $1 ORDER BY id",
                &[&self.writer()],
            )
            .await
            .expect("read the writer's baselines");
        rows.iter()
            .map(|row| (row.get(0), row.get(1), row.get(2)))
            .collect()
    }
}
