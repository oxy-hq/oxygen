//! Resync (design §5.5): an operator re-baselines a captured app's tables.
//! The unshipped log rows of a held table are discarded (the baseline's
//! snapshot supersedes them), a baseline is requested for each target, and
//! the holds the resync resolves are released, so the relay ships the
//! snapshot.
//!
//! - **Targets** are the table asked for, or every installed table; a writer
//!   past the relay's ceiling (a `ceiling` hold) resyncs every installed
//!   table whatever was asked, discards all of its waiting rows, and has the
//!   hold released.
//! - **Discarded** rows are only those of a held table: an unheld table's
//!   unshipped rows still ship, and the snapshot lands beside them.

use oxy_oltp::changes::{
    BaselineReason, DISCARD_SQL, INSTALLED_SQL, STREAM_SQL, StreamState, pin_session_sql,
    request_named_baseline_sql,
};
use oxy_oltp::connect::pg_detail;
use oxy_oltp::schema::app_writer_name;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;
use uuid::Uuid;

use super::enabled_for;
use super::owner::{current_tenant, owner_client};
use super::relay::holds::{self, HoldKind, Holds, Release};
use super::relay::{BASELINE_STATEMENT_TIMEOUT, BEGIN_LOCK_TIMEOUT, PRODUCTION, marks};

/// The body of the resync route. Only production has a change stream.
#[derive(Debug, Default, Deserialize)]
pub struct ResyncRequest {
    /// One table; every captured table when absent.
    #[serde(default)]
    pub table: Option<String>,
    /// `production` when absent.
    #[serde(default)]
    pub environment: Option<String>,
}

/// The rows a resync removed from the log, of one table.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Discarded {
    pub table: String,
    pub rows: i64,
}

/// What a resync did.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ResyncReport {
    pub writer: String,
    /// The tables a baseline was requested of.
    pub requested: Vec<String>,
    /// The holds removed: a table's name, or `""` for the writer's ceiling hold.
    pub released: Vec<String>,
    pub discarded: Vec<Discarded>,
}

/// Why a resync was refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum ResyncError {
    #[error("the org's oltp_changes flag is off")]
    Off,
    #[error("only production has a change stream, not {0:?}")]
    NotProduction(String),
    #[error("the app has no change capture installed")]
    NoCapture,
    #[error("the table {0:?} is not captured for this app")]
    UnknownTable(String),
    #[error("a generation bump is re-stamping the log; resync after it finishes")]
    Restamping,
    #[error("a generation bump ran since the resync planned; retry")]
    Bumped,
    #[error("{cause} (after discarding {} rows)", discarded.iter().map(|d| d.rows).sum::<i64>())]
    AfterDiscard {
        discarded: Vec<Discarded>,
        cause: String,
    },
    #[error("{0}")]
    Infra(String),
}

/// Rows a chunk of a discard removes, each in a transaction of its own, so a
/// held backlog never meets the statement timeout.
pub const DISCARD_CHUNK_ROWS: i64 = 50_000;

/// `$1 generation, $2 writer, $3 tables or null, $4 limit`: [`DISCARD_SQL`],
/// for at most `$4` of the oldest rows.
const DISCARD_CHUNK_SQL: &str = "WITH gone AS (DELETE FROM oxy_changes.log \
     WHERE seq IN (SELECT seq FROM oxy_changes.log \
        WHERE generation = $1::bigint AND writer = $2::text \
          AND ($3::text[] IS NULL OR table_name = ANY($3::text[])) \
        ORDER BY seq LIMIT $4::bigint) \
     RETURNING table_name) \
     SELECT table_name, count(*)::bigint AS rows FROM gone GROUP BY table_name \
     ORDER BY table_name";

/// Which of the writer's waiting log rows a resync removes.
#[derive(Debug, PartialEq, Eq)]
enum Discard {
    Nothing,
    /// Every row of the writer.
    All,
    Tables(Vec<String>),
}

/// What a resync will do, from what is installed and held.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    ceiling: bool,
    targets: Vec<String>,
    discard: Discard,
}

impl Plan {
    fn of(
        writer: &str,
        table: Option<&str>,
        installed: Vec<String>,
        holds: &Holds,
    ) -> Result<Self, ResyncError> {
        if installed.is_empty() {
            return Err(ResyncError::NoCapture);
        }
        if let Some(t) = table.filter(|t| !installed.iter().any(|i| i == t)) {
            return Err(ResyncError::UnknownTable(t.to_owned()));
        }
        let ceiling = holds.writer_kind(writer) == Some(HoldKind::Ceiling);
        let targets = match table {
            Some(t) if !ceiling => vec![t.to_owned()],
            _ => installed,
        };
        let held = holds.tables(writer);
        let held: Vec<String> = targets
            .iter()
            .filter(|t| held.contains(t))
            .cloned()
            .collect();
        let discard = match (ceiling, held.is_empty()) {
            (true, _) => Discard::All,
            (false, true) => Discard::Nothing,
            (false, false) => Discard::Tables(held),
        };
        Ok(Self {
            ceiling,
            targets,
            discard,
        })
    }
}

fn infra(doing: &str, e: impl std::fmt::Display) -> ResyncError {
    ResyncError::Infra(format!("{doing}: {e}"))
}

fn pg(doing: &'static str) -> impl Fn(tokio_postgres::Error) -> ResyncError {
    move |e| infra(doing, pg_detail(&e))
}

/// Resync `app_slug`'s production capture: `table`, or every captured table.
pub async fn resync(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_slug: &str,
    table: Option<&str>,
) -> Result<ResyncReport, ResyncError> {
    resync_with(db, org_id, app_slug, table, DISCARD_CHUNK_ROWS).await
}

/// [`resync`], discarding `chunk` rows a transaction.
pub async fn resync_with(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_slug: &str,
    table: Option<&str>,
    chunk: i64,
) -> Result<ResyncReport, ResyncError> {
    if !enabled_for(org_id) {
        return Err(ResyncError::Off);
    }
    let writer = app_writer_name(app_slug).ok_or(ResyncError::NoCapture)?;
    let tenant = current_tenant(db, org_id, true)
        .await
        .map_err(ResyncError::Infra)?
        .ok_or(ResyncError::NoCapture)?;
    let mut owner = owner_client(&tenant, "oxy_changes resync")
        .await
        .map_err(ResyncError::Infra)?;
    let (generation, installed) = read_state(&owner, &writer).await?;
    let holds = holds::read(db, org_id, PRODUCTION)
        .await
        .map_err(|e| infra("read the holds", e))?;
    let plan = Plan::of(&writer, table, installed, &holds)?;

    let run = Run {
        writer: &writer,
        generation,
        plan: &plan,
        chunk,
    };
    let mut discarded = discard_all(&mut owner, org_id, run).await?;
    // Before the release and the nudge, so a failure of either keeps the counts.
    log(org_id, &writer, &plan, &discarded);
    let tail = async {
        let released = release(db, org_id, &writer, &plan).await?;
        marks::nudge(db, org_id, PRODUCTION)
            .await
            .map_err(|e| infra("nudge the relay", e))?;
        Ok::<_, ResyncError>(released)
    };
    let released = tail
        .await
        .map_err(|cause: ResyncError| ResyncError::AfterDiscard {
            discarded: std::mem::take(&mut discarded),
            cause: cause.to_string(),
        })?;
    Ok(ResyncReport {
        writer,
        requested: plan.targets,
        released,
        discarded,
    })
}

/// What one resync discards, and how.
#[derive(Clone, Copy)]
struct Run<'a> {
    writer: &'a str,
    generation: i64,
    plan: &'a Plan,
    /// Rows a chunk's transaction removes.
    chunk: i64,
}

/// The chunks, then the final transaction. A failure after a chunk
/// committed keeps what the chunks dropped: it is alarmed, and returned as
/// [`ResyncError::AfterDiscard`] for the audit row.
async fn discard_all(
    owner: &mut Client,
    org_id: Uuid,
    run: Run<'_>,
) -> Result<Vec<Discarded>, ResyncError> {
    let Run {
        writer,
        generation,
        plan,
        ..
    } = run;
    let mut total = Vec::new();
    let done = async {
        discard_chunks(owner, run, &mut total).await?;
        apply(owner, writer, generation, plan).await
    };
    match done.await {
        Ok(rest) => {
            merge(&mut total, rest);
            Ok(total)
        }
        Err(cause) if !total.is_empty() => {
            log(org_id, writer, plan, &total);
            Err(ResyncError::AfterDiscard {
                discarded: total,
                cause: cause.to_string(),
            })
        }
        Err(cause) => Err(cause),
    }
}

/// Add `more` to `into`, by table.
fn merge(into: &mut Vec<Discarded>, more: Vec<Discarded>) {
    for d in more {
        match into.iter_mut().find(|i| i.table == d.table) {
            Some(i) => i.rows += d.rows,
            None => into.push(d),
        }
    }
    into.sort_by(|a, b| a.table.cmp(&b.table));
}

/// The plan's discard as `(whether, tables or all)`.
fn scope(plan: &Plan) -> (bool, Option<&[String]>) {
    match &plan.discard {
        Discard::Nothing => (false, None),
        Discard::All => (true, None),
        Discard::Tables(t) => (true, Some(t.as_slice())),
    }
}

/// Discard in transactions of `chunk` rows, each pinned, until one removes
/// fewer: the hold keeps the relay off the writer meanwhile. The final
/// transaction ([`apply`]) takes what is left.
async fn discard_chunks(
    owner: &mut Client,
    run: Run<'_>,
    total: &mut Vec<Discarded>,
) -> Result<(), ResyncError> {
    let Run {
        writer,
        generation,
        plan,
        chunk,
    } = run;
    let doing = "discard the held rows in chunks";
    let (discards, only) = scope(plan);
    if !discards {
        return Ok(());
    }
    loop {
        let txn = owner.transaction().await.map_err(pg(doing))?;
        let pin = pin_session_sql(BEGIN_LOCK_TIMEOUT, BASELINE_STATEMENT_TIMEOUT);
        txn.batch_execute(&pin).await.map_err(pg(doing))?;
        let rows = txn
            .query(DISCARD_CHUNK_SQL, &[&generation, &writer, &only, &chunk])
            .await
            .map_err(pg(doing))?;
        txn.commit().await.map_err(pg(doing))?;
        let gone = read_discarded(&rows, doing)?;
        let count: i64 = gone.iter().map(|d| d.rows).sum();
        merge(total, gone);
        if count < chunk {
            break;
        }
    }
    Ok(())
}

fn read_discarded(
    rows: &[tokio_postgres::Row],
    doing: &'static str,
) -> Result<Vec<Discarded>, ResyncError> {
    rows.iter()
        .map(|row| {
            Ok(Discarded {
                table: row.try_get("table_name").map_err(pg(doing))?,
                rows: row.try_get("rows").map_err(pg(doing))?,
            })
        })
        .collect()
}

/// The stream's generation and the writer's installed tables, refusing a
/// stream that is re-stamping or has no capture.
async fn read_state(owner: &Client, writer: &str) -> Result<(i64, Vec<String>), ResyncError> {
    let doing = "read the change stream's state";
    let row = owner
        .query_opt(STREAM_SQL, &[])
        .await
        .map_err(pg(doing))?
        .ok_or(ResyncError::NoCapture)?;
    let stream = StreamState::from_row(&row).map_err(pg(doing))?;
    if stream.restamping.is_some() {
        return Err(ResyncError::Restamping);
    }
    let doing = "read the app's installed change capture";
    let rows = owner
        .query(INSTALLED_SQL, &[&writer])
        .await
        .map_err(pg(doing))?;
    let mut installed = Vec::new();
    for row in &rows {
        if row.try_get::<_, bool>("installed").map_err(pg(doing))? {
            installed.push(row.try_get("table_name").map_err(pg(doing))?);
        }
    }
    Ok((stream.generation, installed))
}

/// The discard and each target's request, in one pinned owner transaction.
async fn apply(
    owner: &mut Client,
    writer: &str,
    generation: i64,
    plan: &Plan,
) -> Result<Vec<Discarded>, ResyncError> {
    let doing = "discard the held rows and request the baselines";
    let txn = owner.transaction().await.map_err(pg(doing))?;
    let pin = pin_session_sql(BEGIN_LOCK_TIMEOUT, BASELINE_STATEMENT_TIMEOUT);
    txn.batch_execute(&pin).await.map_err(pg(doing))?;
    recheck(&txn, generation).await?;
    let (discards, only) = scope(plan);
    let mut discarded = Vec::new();
    if discards {
        let rows = txn
            .query(DISCARD_SQL, &[&generation, &writer, &only])
            .await
            .map_err(pg(doing))?;
        discarded = read_discarded(&rows, doing)?;
    }
    request_all(&txn, writer, &plan.targets).await?;
    txn.commit().await.map_err(pg(doing))?;
    Ok(discarded)
}

/// Refuse when a bump ran since planning: the generation moved, or a
/// re-stamp is running.
async fn recheck(
    txn: &tokio_postgres::Transaction<'_>,
    generation: i64,
) -> Result<(), ResyncError> {
    let doing = "re-read the change stream's state";
    let row = txn.query_opt(STREAM_SQL, &[]).await.map_err(pg(doing))?;
    let stream = row
        .map(|row| StreamState::from_row(&row))
        .transpose()
        .map_err(pg(doing))?;
    match stream {
        Some(s) if s.generation == generation && s.restamping.is_none() => Ok(()),
        _ => Err(ResyncError::Bumped),
    }
}

/// A Resync baseline request for each of `targets`.
async fn request_all(
    txn: &tokio_postgres::Transaction<'_>,
    writer: &str,
    targets: &[String],
) -> Result<(), ResyncError> {
    let doing = "request the baselines";
    for table in targets {
        let request = request_named_baseline_sql(writer, table, BaselineReason::Resync)
            .map_err(|e| infra(doing, e))?;
        for sql in &request {
            txn.batch_execute(sql).await.map_err(pg(doing))?;
        }
    }
    Ok(())
}

/// Remove the table holds of the targets, and the writer's ceiling hold when
/// the plan resolves it. Returns each removed hold's table name.
async fn release(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer: &str,
    plan: &Plan,
) -> Result<Vec<String>, ResyncError> {
    let doing = "release the holds";
    let tables = Release {
        org_id,
        environment: PRODUCTION,
        writer: Some(writer),
        tables: Some(plan.targets.as_slice()),
        kinds: &[HoldKind::Table],
    };
    let mut released = holds::release(db, tables)
        .await
        .map_err(|e| infra(doing, e))?;
    if plan.ceiling {
        let whole = [String::new()];
        let ceiling = Release {
            tables: Some(&whole),
            kinds: &[HoldKind::Ceiling],
            ..tables
        };
        let more = holds::release(db, ceiling)
            .await
            .map_err(|e| infra(doing, e))?;
        released.extend(more);
    }
    Ok(released.into_iter().map(|(_, table)| table).collect())
}

/// `oltp_changes_resynced`: an alarm when rows were discarded, so a resync
/// that dropped changes is never silent.
fn log(org_id: Uuid, writer: &str, plan: &Plan, discarded: &[Discarded]) {
    if discarded.is_empty() {
        tracing::info!(%org_id, writer, requested = ?plan.targets, "oltp_changes_resynced");
    } else {
        tracing::error!(%org_id, writer, requested = ?plan.targets, ?discarded,
            "oltp_changes_resynced");
    }
}
