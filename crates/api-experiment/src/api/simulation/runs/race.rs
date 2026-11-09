//! The paired profit race, read out of Postgres in one pass.
//!
//! [`oxy_simulation::race`] settles which arm won and whether the margin is a
//! finding. It is horizon-agnostic and keyed: hand it one profit number per
//! world per arm and it does the rest. Nothing could hand it those numbers in
//! bulk — `GET /simulations/runs` carries `policy` and `replicate` but no
//! profit, and `cumulative_profit` is a column on `simulation_run_periods`
//! reachable only one run at a time. A race was therefore a listing plus N
//! reads, paired in whatever the caller wrote. This module is the missing read,
//! and it pairs on the server so the pairing is one implementation rather than
//! one per caller.
//!
//! # The SPEC is the world, and it is what pairs
//!
//! `race`'s module docs make the paired test conditional on one fact: replicate
//! *k* of every arm saw the same world, because `replicate_seed(base, k) =
//! base + k` takes no policy argument and [`World`](oxy_simulation::World) draws
//! its exogenous streams from the spec seed alone. That fact holds only while
//! **the whole spec** is unchanged — the seed is what makes one world's draws
//! differ from another's, but every other field is what makes it that world at
//! all.
//!
//! Two edits break the pairing, and only one of them moves the seed:
//!
//! * Edit `seed:` between queueing arm A and arm B and replicate 0 of each is a
//!   different world, so a *replicate*-keyed pairing fabricates a comparison.
//!   With the base moved 7 → 8, `(base 7, replicate 1)` and
//!   `(base 8, replicate 0)` are both seed 8, ARE the same world, and pair
//!   despite differing labels.
//! * Edit anything else — `entities.count` 100 → 500, `noise_ratio`,
//!   `lag_days`, `period_days`, `scale_sigma` — and the seeds still match while
//!   the worlds do not. A *seed*-keyed pairing then reports the edit as a
//!   policy effect: `machine` carrying ~5× the profit of `hold` for reasons
//!   nothing about the policy explains, with `dropped_unpaired: 0` and a
//!   confident p-value. That is the worse of the two failures, because nothing
//!   in the response looks wrong.
//!
//! So the pairing key is the whole world: the **seed** each run stored for
//! itself, plus a fingerprint of [`simulation_runs::Column::Spec`], the spec
//! snapshot `queue_one` wrote for that run. The seed half is redundant — the
//! snapshot is seed-substituted, so the fingerprint already subsumes it — and
//! is kept only because it orders the worlds the way a reader reads them.
//!
//! The fingerprint is over a *canonical* rendering of the spec (object keys
//! sorted, recursively), because the column is `jsonb` and Postgres does not
//! preserve key order: hashing the bytes as they come back would make one world
//! two on a round trip. See [`spec_fingerprint`].
//!
//! `replicate` stays on the wire as the label a reader recognises, and
//! [`ReplicateReach::seed`] and [`ReplicateReach::world`] ride beside it,
//! because once a base seed has moved the replicate number no longer identifies
//! a draw — and once a spec has been edited, neither does the seed. When two
//! arms share no world at all the comparison is withheld as `disjoint_worlds` —
//! a named outcome, distinct from the `no_pairs` an unrun arm produces, which a
//! reader glosses as "no difference".
//!
//! # Only a run that stopped is evidence
//!
//! Runs are loaded with a **terminal** status only — `done`, `failed`,
//! `cancelled`, the three of the five in
//! `crates/api-experiment/src/simulation/store.rs` that nothing will move again.
//! Two reasons, and the first is the blocker:
//!
//! * a re-queued run is the newest row for its world and has no period rows at
//!   all, so admitting it would evict the completed run it repeats and blank a
//!   race that was already there;
//! * a `running` run's curve grows between two identical requests, so scoring
//!   it would move the horizon — and every margin under it — with nothing in
//!   the response saying why. `latest_run_per_draw`'s tie-break already refuses
//!   that ("a race whose numbers move between two identical requests is worse
//!   than either answer"), and a partial curve is the same defect on a longer
//!   clock.
//!
//! A `failed` run is kept: its curve is frozen, the periods it did record are
//! real, and the horizon rule below was written for exactly that raggedness.
//! In-flight runs are counted into [`ProfitRace::in_flight_runs`] rather than
//! dropped silently, the way `superseded_runs` already is.
//!
//! # The horizon is a choice, and it is reported
//!
//! Runs of one world end at different `periods_done` — an arm that failed, an
//! arm retried, an arm whose worker died. **Scoring each run at its own last period
//! row would race arm A at period 40 against arm B at period 12**, which is the
//! same class of error as misaligning replicates: the number that comes out is
//! not a comparison, and nothing in it says so. `ArmProfits` cannot catch it,
//! because by the time the profits reach it they are just numbers.
//!
//! So every arm is scored at ONE period index, and [`ProfitRace::horizon`] says
//! which. By default that is the **deepest period every recorded replicate
//! reached** — `min` over each run's last period — which is the only choice
//! that needs no argument about which runs deserve to be dropped, and the one
//! that maximises the paired sample. Its cost is real and worth stating: one
//! run that died at period 2 drags a five-arm race to period 2, throwing away
//! 38 periods of everyone else's evidence.
//!
//! That cost is why the horizon is a *parameter* and not just a rule.
//! [`ProfitRace::arms`] reports every replicate's `reach`, so a caller who sees
//! `horizon: 2` next to four arms reaching 40 can see exactly which draw held
//! it back, and `?horizon=40` re-runs the race at the depth they meant — with
//! the short replicate **dropped and counted** in `short`/`dropped_unpaired`,
//! never truncated to its own last row.
//!
//! Two drops exist and both are counted rather than swallowed:
//!
//! * a draw with no `cumulative_profit` row at the horizon — it never got that
//!   far, or it recorded nothing at all — is absent from that arm's
//!   [`ArmProfits`] and shows up in [`ArmCoverage::short`];
//! * a world one arm drew and the other did not is `dropped_unpaired`, counted
//!   by `race::compare` itself.
//!
//! # Which arm is the baseline
//!
//! `?baseline=` names it. Unset, it is the first arm present in
//! [`PolicyKind::ALL`] order — the null (`hold`), else what a customer does
//! today (`legacy`), and so on. That order is already "the reference arm
//! first", so the default reference is the most-null arm actually run rather
//! than whichever one sorted first alphabetically.
//!
//! # Multiplicity
//!
//! Every p-value here is **per-comparison and uncorrected** — `race`'s module
//! docs are emphatic about it, and this is the surface that decides the family.
//! [`ProfitRace::family_size`] is how many comparisons were run, so a reader can
//! see that "one arm cleared 0.05" out of four tries is a much weaker statement
//! than it looks. The correction is deliberately not applied here: which arms
//! belong to one family is the *caller's* question, and a surface that silently
//! Bonferroni'd four arms would be as wrong for a caller who only cares about
//! one of them.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::{simulation_run_periods, simulation_runs};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Deserialize;
use uuid::Uuid;

use oxy_simulation::PolicyKind;

use super::super::{ApiError, connect, internal};
use oxy_app::surface::WorkspaceManagerReadOnly;

/// `?baseline=&horizon=` on `GET /simulations/{name}/race`.
#[derive(Debug, Deserialize, Default)]
pub struct RaceQuery {
    /// The arm every challenger is compared against. Absent means the first arm
    /// present in [`PolicyKind::ALL`] order.
    #[serde(default)]
    pub baseline: Option<String>,
    /// Score every arm at this period instead of the common one.
    #[serde(default)]
    pub horizon: Option<i32>,
}

/// The two choices a race takes, parsed.
///
/// Separate from [`RaceQuery`] so the read path is callable — and testable —
/// without building an HTTP request, and so a bad `?baseline=` is a 400 raised
/// once at the edge rather than a `PolicyKind` parse buried in the join.
#[derive(Debug, Default, Clone, Copy)]
pub struct RaceOptions {
    pub baseline: Option<PolicyKind>,
    pub horizon: Option<i32>,
}

impl RaceQuery {
    pub fn parse(self) -> Result<RaceOptions, ApiError> {
        let baseline = self
            .baseline
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<PolicyKind>()
                    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
            })
            .transpose()?;
        // Periods are 1-based: period 0 is "before the loop ran", which no row
        // records. A `?horizon=0` is a caller who meant "the start" and would
        // otherwise get an empty race with no explanation.
        if let Some(h) = self.horizon
            && h < 1
        {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("horizon must be a period index of 1 or more, got {h}"),
            ));
        }
        Ok(RaceOptions {
            baseline,
            horizon: self.horizon,
        })
    }
}

/// `GET /simulations/{name}/race` — every arm of one world against a baseline,
/// paired by replicate at one horizon.
pub async fn get_profit_race(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    // The tuple is not optional: this router is mounted under
    // `/{workspace_id}`, so a bare `Path<String>` sees two segments and axum
    // rejects the request before the handler body runs.
    Path((_workspace_id, name)): Path<(Uuid, String)>,
    Query(query): Query<RaceQuery>,
) -> Result<Json<ProfitRace>, ApiError> {
    let options = query.parse()?;
    // From the extractor, never from the path segment — scoping a tenant read
    // to a number the caller chose is not scoping it at all.
    profit_race_report(workspace_manager.workspace_id, &name, options)
        .await
        .map(Json)
}

/// The read path, split from the handler so it is reachable from a test and the
/// transport layer stays extract-call-serialize.
pub async fn profit_race_report(
    workspace_id: Uuid,
    simulation: &str,
    options: RaceOptions,
) -> Result<ProfitRace, ApiError> {
    let db = connect().await?;
    let loaded = load_curves(&db, workspace_id, simulation).await?;
    assemble(simulation, loaded, options)
}

// ── loading ──────────────────────────────────────────────────────────────────

/// The identity of the world a run ran, as a short hex digest of its spec.
///
/// **Canonical, not literal.** `simulation_runs.spec` is a `jsonb` column and
/// Postgres does not preserve object key order, so the same spec can come back
/// with its keys in a different order than it went in — and two orderings of
/// one world must not be two worlds. Every object is re-rendered with its keys
/// sorted, recursively, before it is hashed. Arrays keep their order, which is
/// meaningful.
///
/// **Never persisted, and never on the wire as an IDENTIFIER.** It does go out,
/// on [`ReplicateReach::world`], but as an opaque token a reader compares within
/// one response — not as a handle anything may store or look up. It is compared
/// only against other fingerprints computed in the same request, so the digest
/// is free to change: a shorter prefix, a different hash, a canonicalisation
/// that also normalises `1.0` and `1` would each be a behaviour change to the
/// race and to nothing else. The prefix is 16 hex characters — 64 bits, which for
/// the handful of worlds in one workspace's listing is collision-free with room
/// to spare, and short enough to read in a response.
fn spec_fingerprint(spec: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};

    fn canonical(value: &serde_json::Value, out: &mut String) {
        use std::fmt::Write;
        match value {
            serde_json::Value::Object(map) => {
                // `serde_json::Map` is insertion-ordered unless the
                // `preserve_order` feature is off, so sort explicitly rather
                // than relying on which it is.
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort_unstable();
                out.push('{');
                for (i, key) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    let _ = write!(out, "{key:?}:");
                    canonical(&map[key], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    canonical(item, out);
                }
                out.push(']');
            }
            other => {
                let _ = write!(out, "{other}");
            }
        }
    }

    let mut rendered = String::new();
    canonical(spec, &mut rendered);
    let digest = Sha256::digest(rendered.as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// One run reduced to what a race needs: which arm, which draw of the world,
/// and the cumulative-profit curve it recorded.
#[derive(Debug, Clone)]
pub struct RunCurve {
    pub policy: PolicyKind,
    /// Half the pairing key, and the half a reader recognises. Stored
    /// `big_integer` because Postgres has no unsigned type; a bit-cast of the
    /// `u64` `replicate_seed` produced, which is fine as a key since the cast
    /// is injective.
    pub seed: i64,
    /// The other half, and the authoritative one: [`spec_fingerprint`] of the
    /// spec snapshot this run stored. Two runs at one seed whose specs differ
    /// are two worlds — see this module's docs.
    pub spec_fingerprint: String,
    /// Which draw of the world this run was labelled as. **A label, not a
    /// key** — see this module's docs.
    pub replicate: i32,
    /// `period -> cumulative_profit`. Empty for a run that died before its
    /// first period — a case the horizon rule has to survive, not divide by.
    pub by_period: BTreeMap<i32, f64>,
}

/// What one workspace's runs of one world reduce to, plus what was set aside.
#[derive(Debug, Default)]
pub struct LoadedCurves {
    pub curves: Vec<RunCurve>,
    /// Older runs of an `(arm, seed)` that a newer terminal run replaced. Two
    /// rows for one world are one world — `ArmProfits::observe` says so — so
    /// only the newest is scored, and the count says how many were passed over.
    pub superseded_runs: usize,
    /// Runs of this world still `queued` or `running`. Not scored, and not
    /// counted as superseded: a run that has not stopped has replaced nothing.
    pub in_flight_runs: usize,
}

/// One `simulation_runs` row, as much of it as a race reads.
///
/// A struct rather than the five-tuple `into_tuple` hands back, because the two
/// fields added here — `seed` and `status` — are the two the race is keyed and
/// filtered on, and a positional `(Uuid, String, i32, i64, String)` at every
/// call site is exactly where a `seed`/`replicate` transposition would hide.
#[derive(Debug, Clone)]
pub struct RaceRunRow {
    pub run_id: Uuid,
    pub policy: String,
    pub replicate: i32,
    pub seed: i64,
    /// [`spec_fingerprint`] of this run's own spec snapshot, computed at load.
    /// Carried rather than the spec itself: the race only ever compares it, and
    /// a full `serde_json::Value` per row on a five-arm × N-replicate listing
    /// is a lot of JSON to hold for an equality test.
    pub spec_fingerprint: String,
    pub status: String,
}

/// A run that survived the status filter and deduplication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoredRun {
    pub run_id: Uuid,
    pub policy: PolicyKind,
    pub replicate: i32,
    pub seed: i64,
    pub spec_fingerprint: String,
}

/// What deduplication set aside, by reason. Two numbers rather than one,
/// because "an older run of this world" and "a run that has not finished" are
/// different facts about a race and a reader acts on them differently: the
/// first is history, the second is "come back in a minute".
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SetAside {
    pub superseded_runs: usize,
    pub in_flight_runs: usize,
}

/// The statuses a run stops at — enumerated from the writes in
/// `crates/api-experiment/src/simulation/store.rs`, which sets exactly five:
/// `queued`, `running`, `done`, `failed`, `cancelled`. The first two are the
/// in-flight pair `limits.rs` counts against the workspace cap; these three are
/// the rest, and nothing moves a run out of them.
///
/// `failed` and `cancelled` are terminal, not excluded: whatever periods such a
/// run recorded are real and frozen, and the horizon rule was written for that
/// raggedness. What is excluded is a run that is still *changing*.
pub const TERMINAL_STATUSES: [&str; 3] = ["done", "failed", "cancelled"];

/// Both queries, in that order: the runs of this world in this workspace, then
/// the period rows of the runs that survived the status filter and
/// deduplication.
///
/// The `workspace_id` filter is a correctness invariant, not an optimisation:
/// `simulation_runs` is keyed by `run_id` alone, so nothing else stops a race
/// from pulling another tenant's profits into its own mean. The periods query
/// is keyed only by `run_id` — safe **because** the id set it is given came
/// from the scoped query above and from nowhere else.
///
/// Status is selected rather than filtered in SQL so the in-flight runs can be
/// *counted* on the way past. One pass either way, and the alternative is a
/// second round trip to learn a number the first query already had in hand.
async fn load_curves<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    simulation: &str,
) -> Result<LoadedCurves, ApiError> {
    let rows: Vec<(Uuid, String, i32, i64, serde_json::Value, String)> =
        simulation_runs::Entity::find()
            .filter(simulation_runs::Column::WorkspaceId.eq(workspace_id))
            .filter(simulation_runs::Column::SimulationName.eq(simulation))
            .select_only()
            .column(simulation_runs::Column::RunId)
            .column(simulation_runs::Column::Policy)
            .column(simulation_runs::Column::Replicate)
            .column(simulation_runs::Column::Seed)
            // The world's identity, not decoration: the seed alone cannot tell a
            // re-run of one world from a run of a world that was edited under it.
            // `NOT NULL` on every row since the table was created, so there is no
            // missing-spec case to fall back for.
            .column(simulation_runs::Column::Spec)
            .column(simulation_runs::Column::Status)
            // Newest first, so deduplication keeps the newest by taking the first
            // it sees. `run_id` breaks a tie rather than leaving one: two runs
            // queued in the same microsecond would otherwise pick a winner by
            // whatever order the scan happened to return, and a race whose numbers
            // move between two identical requests is worse than either answer.
            .order_by_desc(simulation_runs::Column::QueuedAt)
            .order_by_desc(simulation_runs::Column::RunId)
            .into_tuple()
            .all(db)
            .await
            .map_err(internal("load race runs"))?;

    let rows: Vec<RaceRunRow> = rows
        .into_iter()
        .map(
            |(run_id, policy, replicate, seed, spec, status)| RaceRunRow {
                run_id,
                policy,
                replicate,
                seed,
                spec_fingerprint: spec_fingerprint(&spec),
                status,
            },
        )
        .collect();

    let (kept, set_aside) = latest_run_per_draw(rows);
    if kept.is_empty() {
        return Ok(LoadedCurves {
            curves: Vec::new(),
            superseded_runs: set_aside.superseded_runs,
            in_flight_runs: set_aside.in_flight_runs,
        });
    }

    let ids: Vec<Uuid> = kept.iter().map(|run| run.run_id).collect();
    let periods: Vec<(Uuid, i32, f64)> = simulation_run_periods::Entity::find()
        .filter(simulation_run_periods::Column::RunId.is_in(ids))
        .select_only()
        .column(simulation_run_periods::Column::RunId)
        .column(simulation_run_periods::Column::Period)
        .column(simulation_run_periods::Column::CumulativeProfit)
        .into_tuple()
        .all(db)
        .await
        .map_err(internal("load race periods"))?;

    let mut by_run: BTreeMap<Uuid, BTreeMap<i32, f64>> = BTreeMap::new();
    for (run_id, period, cumulative_profit) in periods {
        by_run
            .entry(run_id)
            .or_default()
            .insert(period, cumulative_profit);
    }

    Ok(LoadedCurves {
        curves: kept
            .into_iter()
            .map(|run| RunCurve {
                policy: run.policy,
                seed: run.seed,
                spec_fingerprint: run.spec_fingerprint,
                replicate: run.replicate,
                by_period: by_run.remove(&run.run_id).unwrap_or_default(),
            })
            .collect(),
        superseded_runs: set_aside.superseded_runs,
        in_flight_runs: set_aside.in_flight_runs,
    })
}

/// Keep the newest **terminal** run of each `(arm, seed)`, given rows already
/// ordered newest first.
///
/// A world re-run keeps both rows — `simulation_runs` has no unique key on
/// `(simulation, policy, seed)` and should not: a run is evidence and the old
/// one is still readable. But a race over both would let a January draw and a
/// March draw of the same world fight for one slot in `ArmProfits`, where
/// whichever landed last would win silently. The newest is the one a race
/// means.
///
/// **Keyed on the world, not the replicate.** The replicate is a label whose
/// meaning depends on the base seed the run was fanned out from; the seed is
/// which draw of a world, and the spec is which world. Both halves are in the
/// key, so a re-run of one world supersedes and a run of a world that was
/// *edited* under the same seed does not — the latter is a second world, and
/// evicting one with the other would silently answer a race over both with
/// whichever was queued last. See this module's docs.
///
/// An in-flight run is skipped **before** the key is even considered, so it can
/// neither win a slot nor be recorded as having superseded anything. That
/// ordering is the fix for the eviction: a re-queued run is by construction the
/// newest row for its world and has no period rows, so any rule that let it
/// reach the key would blank the finished race behind it.
///
/// A policy string that does not parse is skipped with a warning rather than
/// failing the race: the column is written from this same enum, so an
/// unrecognised value means the enum moved, and refusing to race the four arms
/// that *are* readable helps nobody. An unrecognised *status* is treated as
/// in-flight — the conservative read, since the alternative is scoring a curve
/// that may still be growing.
pub fn latest_run_per_draw(rows: Vec<RaceRunRow>) -> (Vec<ScoredRun>, SetAside) {
    // A `Vec` rather than a set: `PolicyKind` is not `Hash`, and a race has at
    // most five arms times the draws of one world.
    let mut seen: Vec<(PolicyKind, i64, String)> = Vec::new();
    let mut kept = Vec::new();
    let mut set_aside = SetAside::default();
    for row in rows {
        if !TERMINAL_STATUSES.contains(&row.status.as_str()) {
            tracing::debug!(
                run_id = %row.run_id, status = %row.status,
                "simulation race: a run that has not stopped is not evidence yet"
            );
            set_aside.in_flight_runs += 1;
            continue;
        }
        let Ok(policy) = row.policy.parse::<PolicyKind>() else {
            tracing::warn!(
                run_id = %row.run_id, policy = %row.policy,
                "simulation race: skipping a run whose arm does not parse"
            );
            continue;
        };
        let key = (policy, row.seed, row.spec_fingerprint.clone());
        if seen.contains(&key) {
            set_aside.superseded_runs += 1;
        } else {
            seen.push(key);
            kept.push(ScoredRun {
                run_id: row.run_id,
                policy,
                replicate: row.replicate,
                seed: row.seed,
                spec_fingerprint: row.spec_fingerprint,
            });
        }
    }
    (kept, set_aside)
}

mod report;

pub use report::{
    ArmCoverage, ArmScore, PairedTestResult, ProfitRace, RaceComparison, ReplicateReach, assemble,
};

#[cfg(test)]
mod tests;
