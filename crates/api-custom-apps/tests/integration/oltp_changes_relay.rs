//! The OLTP change stream relay end to end (design §5.3 and §5.4): a
//! production `ctx.oltp` write through the serve route, captured by the
//! capture a promote installed ([`crate::oltp_changes_fixture`]), shipped by
//! `relay_once_with` into typed change tables in an in-memory DuckDB, the
//! Airhouse stand-in ([`DuckSink`]), each change exactly once.
//!
//! The sink is process-global (`install_sink`). nextest runs each test in a
//! process of its own, but `cargo test` runs them as threads of one, so every
//! test here holds [`SERIAL`] for its whole run ([`with_relay`]).
//!
//! - `delivery`: a write reaches its change table signed and typed, once:
//!   after a second pass, after a pass that died between the Airhouse commit
//!   and its confirm (which backs the mark off until the next pass ships),
//!   and for a source transaction larger than a batch.
//! - `ceiling`: transactions past the relay's ceiling together ship one batch
//!   each; one past it alone holds its writer.
//! - `shapes`: an added column is added, null before it; a renamed one holds
//!   only its table.
//! - `guards`: an unsigned write ships `unverified`; the relay is inert
//!   outside production and with the flag off; a store where capture was
//!   never installed drains its mark.
//! - `bump`: a store rewound under Airhouse bumps its generation once, past
//!   Airhouse's, and ships and baselines in the new one; a re-stamp left set
//!   is finished; a writer held for a restore is checked again; a bump behind
//!   a long transaction fails the pass and backs it off.
//! - `drift`: a disabled capture trigger is put back and its gap baselined;
//!   a store without its guard is recorded once; a repair waits for an
//!   install holding the app's lock.
//! - `baselines`: an install's baseline copies the rows already there, and a
//!   reshape's carries a backfill; a copy paced by its backlog resumes at its
//!   cursor; writes during a copy win over its snapshots; markers land once
//!   after a pass that died; a held table's baseline waits.
//! - `resync`: a held table's unshipped rows are discarded and its baseline
//!   takes their place; an unheld table keeps its rows; a writer past the
//!   ceiling is baselined whole.
//! - `target`: a writer whose app's workspace has no Airhouse is held, and
//!   once the Airhouse exists the next pass lifts the hold and ships.
//! - `lease_desk`: the `customer-apps/examples/lease-desk` example's own
//!   manifest, migration and function, end to end: every version of a lease
//!   and each lease as it stands reach Airhouse, an excluded column never
//!   does, and beside a second app of the same org both ship to their own
//!   schemas, a restore re-baselines both, and a resync of one leaves the
//!   other's rows.
//!
//! Every install now requests a baseline, so a test that installs capture
//! calls [`Relay::settle`] right after, and the behaviour under test starts
//! from a stream with nothing open and nothing waiting.

use std::future::Future;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use oxy_api_custom_apps::oltp_changes::relay::render::{
    PROGRESS_TABLE, changes_table_name, schema_name,
};
use oxy_api_custom_apps::oltp_changes::relay::sink::install_sink;
use oxy_api_custom_apps::oltp_changes::relay::{Limits, RelayStats, relay_once_with};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use crate::oltp_changes_fixture::{App, with_app};

mod baselines;
mod bump;
mod ceiling;
mod delivery;
mod drift;
mod duck;
mod guards;
mod inspect;
mod lease_desk;
mod resync;
mod shapes;
mod target;

use duck::DuckSink;

/// The only environment the relay ships.
const PRODUCTION: &str = "production";

/// Held by every test of this file for its whole run: the sink each installs
/// is the process's.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A started [`App`], and the DuckDB its relay ships to.
struct Relay {
    app: Arc<App>,
    duck: Arc<DuckSink>,
}

/// Run `body` against a fresh, started [`App`] with a fresh [`DuckSink`]
/// installed, holding [`SERIAL`].
async fn with_relay<F, Fut>(body: F)
where
    F: FnOnce(Relay) -> Fut,
    Fut: Future<Output = ()>,
{
    let _serial = SERIAL.lock().await;
    let duck = Arc::new(DuckSink::new());
    install_sink(duck.clone());
    Box::pin(with_app(move |app| body(Relay { app, duck }))).await;
}

/// `Some(v)`, for comparing DuckDB's text.
fn text(v: &str) -> Option<String> {
    Some(v.to_owned())
}

impl Relay {
    fn db(&self) -> &DatabaseConnection {
        &self.app.t.db
    }

    /// The bare writer name: the log's `writer`, and `oltp_<writer>`.
    fn writer(&self) -> String {
        self.app.writer_name()
    }

    /// One production pass under the default [`Limits`], which must succeed.
    async fn pass(&self) -> RelayStats {
        self.pass_with(&Limits::default()).await
    }

    async fn pass_with(&self, limits: &Limits) -> RelayStats {
        self.try_pass(PRODUCTION, limits)
            .await
            .expect("a relay pass")
    }

    async fn try_pass(&self, environment: &str, limits: &Limits) -> Result<RelayStats, String> {
        relay_once_with(self.db(), self.app.org(), environment, limits).await
    }

    /// `"oltp_<writer>"."<table>"` in DuckDB.
    fn qualified(&self, table: &str) -> String {
        format!("\"{}\".\"{table}\"", schema_name(&self.writer()))
    }

    /// `columns` of every row of `table`'s change table, by `_seq`.
    fn changes(&self, table: &str, columns: &str) -> Vec<Vec<Option<String>>> {
        let from = self.qualified(&changes_table_name(table));
        self.duck
            .read(&format!("SELECT {columns} FROM {from} ORDER BY _seq"))
    }

    /// One aggregate over `table`'s change table, as text.
    fn aggregate(&self, table: &str, aggregate: &str) -> Option<String> {
        let from = self.qualified(&changes_table_name(table));
        let rows = self.duck.read(&format!("SELECT {aggregate} FROM {from}"));
        rows.into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .flatten()
    }

    /// The tables of the writer's schema in DuckDB, by name.
    fn tables(&self) -> Vec<String> {
        let schema = schema_name(&self.writer());
        let rows = self.duck.read(&format!(
            "SELECT table_name FROM information_schema.tables \
              WHERE table_schema = '{schema}' ORDER BY table_name"
        ));
        rows.into_iter().filter_map(|row| row[0].clone()).collect()
    }

    /// `(column_name, data_type)` of `table`'s change table, in order.
    fn columns(&self, table: &str) -> Vec<(String, String)> {
        let schema = schema_name(&self.writer());
        let table = changes_table_name(table);
        let rows = self.duck.read(&format!(
            "SELECT column_name, data_type FROM information_schema.columns \
              WHERE table_schema = '{schema}' AND table_name = '{table}' \
              ORDER BY ordinal_position"
        ));
        rows.into_iter()
            .map(|row| (row[0].clone().unwrap(), row[1].clone().unwrap()))
            .collect()
    }

    /// `_progress`'s one row, as `(generation, batch)`.
    fn shipped(&self) -> (i64, i64) {
        let from = self.qualified(PROGRESS_TABLE);
        let rows = self
            .duck
            .read(&format!("SELECT generation, batch FROM {from}"));
        assert_eq!(rows.len(), 1, "_progress keeps one row: {rows:?}");
        let number = |i: usize| -> i64 {
            let cell = rows[0][i].as_deref().expect("a number");
            cell.parse().expect("a number")
        };
        (number(0), number(1))
    }

    /// The writer's rows waiting in the store's `oxy_changes.log`, of `table`
    /// or of every table.
    async fn waiting(&self, table: Option<&str>) -> i64 {
        self.app
            .owner()
            .await
            .query_one(
                "SELECT count(*) FROM oxy_changes.log \
                  WHERE writer = $1 AND ($2::text IS NULL OR table_name = $2::text)",
                &[&self.writer(), &table],
            )
            .await
            .expect("count the writer's log rows")
            .get(0)
    }

    /// The writer's `(intent_batch, shipped_batch)` in the store, `(0, 0)`
    /// before its first batch.
    async fn progress(&self) -> (i64, i64) {
        let row = self
            .app
            .owner()
            .await
            .query_opt(
                "SELECT intent_batch, shipped_batch FROM oxy_changes.progress WHERE writer = $1",
                &[&self.writer()],
            )
            .await
            .expect("read the writer's progress");
        row.map_or((0, 0), |row| (row.get(0), row.get(1)))
    }

    /// The production mark's `nudged_at`; `None` with no mark.
    async fn nudged_at(&self) -> Option<DateTime<Utc>> {
        let row = self
            .db()
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT nudged_at FROM oltp_change_relays WHERE org_id = $1 AND environment = $2",
                [self.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("read the relay's mark")?;
        row.try_get("", "nudged_at").expect("nudged_at")
    }

    /// The production mark's backoff: its `failures`, and whether it has a
    /// `retry_after` and a `failed_at`.
    async fn backoff(&self) -> (i32, bool, bool) {
        let row = self
            .db()
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT failures, retry_after IS NOT NULL AS backed_off, \
                        failed_at IS NOT NULL AS failed \
                   FROM oltp_change_relays WHERE org_id = $1 AND environment = $2",
                [self.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("read the relay's mark")
            .expect("the mark");
        let column = |name: &str| row.try_get::<bool>("", name).expect(name);
        let failures = row.try_get("", "failures").expect("failures");
        (failures, column("backed_off"), column("failed"))
    }

    /// `environment`'s lease epoch: how many passes took its lease. `None`
    /// with no row at all.
    async fn lease_epoch(&self, environment: &str) -> Option<i64> {
        let row = self
            .db()
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT lease_epoch FROM oltp_change_relays WHERE org_id = $1 AND environment = $2",
                [self.app.org().into(), environment.into()],
            ))
            .await
            .expect("read the relay's lease")?;
        Some(row.try_get("", "lease_epoch").expect("lease_epoch"))
    }

    /// Every production hold of the org, as `(writer, table_name, reason)`.
    async fn holds(&self) -> Vec<(String, String, String)> {
        let rows = self
            .db()
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT writer, table_name, reason FROM oltp_change_holds \
                  WHERE org_id = $1 AND environment = $2 ORDER BY writer, table_name",
                [self.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("read the relay's holds");
        rows.iter()
            .map(|row| {
                let column = |name: &str| -> String { row.try_get("", name).expect(name) };
                (column("writer"), column("table_name"), column("reason"))
            })
            .collect()
    }
}
