//! Queueing runs of a declared world, and reading them back.
//!
//! A run is an *arm* and a *draw* of a world: `?policies=hold,machine` queues
//! one run per arm on the world's own seed, and a world that declares
//! `replicates:` fans each arm onto that many seeds. Both live here rather than
//! in the `.simulation.yml` for the same reason — a world is what happens, a
//! policy is what someone does about it, and a profit race is only attributable
//! if every arm saw the same world.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::simulation_runs;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, Statement, TransactionTrait,
};
use serde::Deserialize;
use uuid::Uuid;

use oxy_simulation::{PolicyKind, SimulationSpec};

use super::worlds::resolve_world;
use super::{ApiError, connect, internal};
use crate::simulation::store::{self, NewRun};
use crate::simulation::{SIMULATION_RUN_KIND, SimulationRunPayload};
use oxy_app::surface::WorkspaceManagerReadOnly;

mod limits;
mod queued;
mod race;
mod read;
#[cfg(test)]
mod tests;

pub use limits::{MAX_IN_FLIGHT_PER_WORKSPACE, MAX_RUNS_PER_REQUEST, RunPage};
pub use queued::{EnqueuedRun, FailedArm, QueuedRuns};
pub use race::{
    ArmCoverage, ArmScore, LoadedCurves, PairedTestResult, ProfitRace, RaceComparison, RaceOptions,
    RaceQuery, RaceRunRow, ReplicateReach, RunCurve, ScoredRun, SetAside, TERMINAL_STATUSES,
    get_profit_race, profit_race_report,
};
pub use read::{
    RunDetail, get_run, list_runs, list_workspace_runs, list_workspace_runs_page, read_run,
};

/// What a caller chooses about a run, as query parameters.
///
/// Everything here is a property of the *run*, not of the world — which is the
/// whole reason it is here rather than in the `.simulation.yml`.
#[derive(Debug, Deserialize, Default)]
pub struct RunQuery {
    /// `?policies=hold,machine` — the arms to race. Absent means `machine`: a
    /// run nobody parameterised is a run of the thing we ship.
    #[serde(default)]
    pub policies: Option<String>,
    /// Overrides the world's declared `replicates:`. For a one-off look at a
    /// world that normally runs five seeds.
    #[serde(default)]
    pub replicates: Option<u32>,
}

/// A resolved run request. Grouped rather than passed as four positionals: the
/// name says *which world*, the rest say *which experiment*.
///
/// No branch: which revision the world is read from is the manager's, pinned
/// once by `workspace_middleware` — see [`super::worlds`].
pub struct RunRequest<'a> {
    pub name: &'a str,
    /// Empty means the default arm.
    pub policies: Vec<PolicyKind>,
    pub replicates: Option<u32>,
}

impl<'a> RunRequest<'a> {
    /// The single-arm, single-draw request — what a caller who names only a
    /// world gets.
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            policies: Vec::new(),
            replicates: None,
        }
    }
}

/// `POST /simulations/{name}/runs` — queue one or more runs of one declared
/// world.
///
/// Plural because the arms of a profit race are runs of the *same* world:
/// `?policies=hold,machine` queues both against one seed, which is what makes
/// the gap between their profit curves attributable rather than suggestive.
/// Replicates fan out the same way — a marginal world needs several draws
/// before its cell of the outcome map means anything.
///
/// Returns as soon as the tasks are queued. A 40-period run is minutes of
/// warehouse queries, so the handler's job is to enqueue and get out of the way
/// — the work happens on the worker fleet, and progress is read back through
/// [`get_run`].
///
/// The body is a [`QueuedRuns`], not a bare list: each run is queued in its
/// own transaction, so a failure part-way leaves earlier arms executing, and
/// the response names them plus what did not happen rather than answering 500
/// over runs the fleet is already spending minutes on.
pub async fn enqueue_run(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    // The tuple is not optional: this router is mounted under
    // `/{workspace_id}`, so a bare `Path<String>` sees two segments and axum
    // rejects the request before the handler body ever runs.
    Path((_workspace_id, name)): Path<(Uuid, String)>,
    Query(q): Query<RunQuery>,
) -> Result<Json<QueuedRuns>, ApiError> {
    let policies = parse_policies(q.policies.as_deref())?;
    start_run(
        workspace_manager.workspace_id,
        &workspace_manager.config_manager,
        RunRequest {
            name: &name,
            policies,
            replicates: q.replicates,
        },
    )
    .await
    .map(Json)
}

/// `hold,machine` → two arms. A misspelled arm is a 400 naming the five, not a
/// silent fallback to `machine`: queueing the product when someone asked for
/// the null would put a wrong number on a chart with nothing to say it is
/// wrong.
pub fn parse_policies(raw: Option<&str>) -> Result<Vec<PolicyKind>, ApiError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let parsed: Vec<PolicyKind> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<PolicyKind>()
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
        })
        .collect::<Result<_, _>>()?;

    // Dedupe rather than reject: `lever_conflicts`
    // (crates/semantic/src/metric_tree.rs) treats a repeated id as one, not an
    // error, and the same read applies here — `?policies=machine,machine` is a
    // caller who typo-repeated an arm, not one who meant two runs of it. First
    // occurrence wins so the order stays what the caller asked for; that order
    // is user-visible in the queued runs, so this must not become a sort.
    let mut deduped: Vec<PolicyKind> = Vec::with_capacity(parsed.len());
    for policy in parsed {
        if !deduped.contains(&policy) {
            deduped.push(policy);
        }
    }
    Ok(deduped)
}

/// Resolve a world through the manager and queue a run of it.
///
/// `workspace_id` is still its own argument: it keys the run rows, which is a
/// different question from where the world was read.
pub async fn start_run<S: oxy::config::DiskSlot>(
    workspace_id: Uuid,
    config_manager: &oxy::config::ConfigManager<S>,
    req: RunRequest<'_>,
) -> Result<QueuedRuns, ApiError> {
    let db = connect().await?;
    let name = req.name;
    let definition = resolve_world(config_manager, name).await?;

    // Parsed here only for the two numbers the fan-out needs — the base seed and
    // the declared replicate count. *Validation* deliberately stays on the
    // worker: an incoherent world (an unreachable optimum, too little history)
    // is a run failure with a diagnosable message, not a 400 that leaves no
    // record anyone can read. A body that is not a world at all cannot be
    // queued, because there would be nothing to fan out over.
    let spec: SimulationSpec = serde_json::from_value(definition.clone()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("'{name}' is not a declared world: {e}"),
        )
    })?;

    let policies = if req.policies.is_empty() {
        vec![PolicyKind::default()]
    } else {
        req.policies
    };
    let replicates = req.replicates.unwrap_or(spec.replicates).max(1);
    let total = limits::check_request_size(policies.len(), replicates)?;
    // Before the fan-out, against the whole request: a request that would
    // cross the cap on its last arm should not queue its first.
    //
    // An affordance, not the enforcement. Read outside any transaction, it
    // answers the common case — one caller, nothing in flight, a request that
    // plainly does not fit — with a clean 429 and no rows written. It cannot
    // be what holds the cap, because concurrent requests all read it before
    // any of them commits; `queue_one` re-checks under a lock, and that is
    // what actually bounds the workspace. See [`limits::advisory_lock_key`].
    let in_flight = count_in_flight(&db, workspace_id).await?;
    limits::check_in_flight(in_flight, total)?;

    let world = DeclaredWorld {
        definition: &definition,
        spec: &spec,
    };
    let mut queued = QueuedRuns::default();
    'arms: for policy in policies {
        for replicate in 0..replicates {
            match queue_one(&db, workspace_id, world, policy, replicate).await {
                Ok(run) => queued.runs.push(run),
                Err(err) => {
                    // Stop here rather than skipping the arm: the runs behind
                    // this one would be a race missing one of its arms, and
                    // whatever failed once is likely to fail again.
                    let failed = FailedArm {
                        policy,
                        replicate,
                        total,
                    };
                    queued = queued.absorb_failure(failed, err)?;
                    break 'arms;
                }
            }
        }
    }
    Ok(queued)
}

/// How many of this workspace's runs are queued or running right now — what
/// [`limits::check_in_flight`] counts a request against.
///
/// Bounded by [`limits::in_flight_cutoff`], not just by status: a terminal
/// write is best-effort, so a run whose worker died between its last period
/// and its status update stays `running` with nothing left to move it. Counting
/// those forever would let 64 of them lock a workspace out of queueing
/// permanently, and the API offers no cancel to dig it back out.
///
/// Generic over the connection so the *enforcing* read in [`queue_one`] can run
/// inside that run's transaction, under the advisory lock, rather than on a
/// pooled connection of its own — a count taken outside the transaction that
/// inserts is exactly the race this used to lose.
async fn count_in_flight<C: ConnectionTrait>(db: &C, workspace_id: Uuid) -> Result<u64, ApiError> {
    let cutoff = limits::in_flight_cutoff(chrono::Utc::now());
    simulation_runs::Entity::find()
        .filter(simulation_runs::Column::WorkspaceId.eq(workspace_id))
        .filter(simulation_runs::Column::Status.is_in(["queued", "running"]))
        .filter(simulation_runs::Column::QueuedAt.gte(cutoff.fixed_offset()))
        .count(db)
        .await
        .map_err(internal("count in-flight runs"))
}

/// A world as both of the things queueing needs it to be: the opaque body that
/// travels to the worker, and the parsed spec the fan-out reads its seed and
/// replicate count from.
#[derive(Clone, Copy)]
struct DeclaredWorld<'a> {
    definition: &'a serde_json::Value,
    spec: &'a SimulationSpec,
}

/// Queue one (world, arm, draw), atomically.
///
/// Split out because the fan-out reads as a loop over the experiment, and the
/// per-run plumbing — derive the seed, snapshot the spec, register the run,
/// enqueue the task — is the same three calls every time.
///
/// One transaction per run, around all three writes. Without it, a failure
/// between the run row and its task left a row at `queued` with nothing to
/// ever move it — invisible to the fleet, visible to every listing, forever.
/// Per run rather than per request so a failure on arm `k` keeps arms `0..k`
/// rather than rolling back runs the caller can see and reuse; the response
/// says which — see [`QueuedRuns`].
async fn queue_one(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    world: DeclaredWorld<'_>,
    policy: PolicyKind,
    replicate: u32,
) -> Result<EnqueuedRun, ApiError> {
    let name = world.spec.name.as_str();
    let seed = replicate_seed(world.spec.seed, replicate);

    // The seed is substituted into the snapshot rather than carried beside it,
    // so the spec a run stores IS the world it ran. A reader who opens replicate
    // 3 sees its seed, not the file's plus arithmetic they have to redo.
    let mut snapshot = world.definition.clone();
    if let Some(obj) = snapshot.as_object_mut() {
        obj.insert("seed".to_string(), serde_json::json!(seed));
    }

    let run_id = Uuid::new_v4();
    // The spec travels by value. A run is evidence, and one that re-read its
    // world at execution time would silently run a different world from the one
    // that was requested if the file changed in between.
    let payload = SimulationRunPayload {
        run_id,
        workspace_id,
        revision_id: None,
        spec: snapshot,
        policy,
        replicate,
    };

    // Written before the task is even queued: `GET .../runs/{run_id}` has to
    // find a row the instant this handler returns, or a caller that polls
    // ahead of the worker fleet claiming the task sees "no such run" instead
    // of a run that simply hasn't started — see `store::queue_run`.
    let txn = db.begin().await.map_err(internal("begin queue"))?;

    // The cap, enforced. Taken before the count and released when this
    // transaction ends, so every concurrent queueing attempt in this workspace
    // reads a count that already includes the rows the others committed.
    //
    // Per run rather than once around the fan-out, because the fan-out
    // deliberately commits per run (see this function's doc): one transaction
    // spanning every arm would hold the lock for the whole request and would
    // roll arms `0..k` back on a failure at `k`, which is the behaviour the
    // per-run transaction exists to avoid. The cost is that a request racing
    // the cap now stops mid-fan-out instead of being refused whole — which is
    // the partial-failure path `QueuedRuns::absorb_failure` already reports,
    // naming the arm that did not queue.
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1)",
        [limits::advisory_lock_key(workspace_id).into()],
    ))
    .await
    .map_err(internal("lock the workspace's run queue"))?;
    let in_flight = count_in_flight(&txn, workspace_id).await?;
    limits::check_in_flight(in_flight, 1)?;

    let mut queued_spec = world.spec.clone();
    queued_spec.seed = seed;
    store::queue_run(
        &txn,
        NewRun {
            run_id,
            workspace_id,
            revision_id: None,
            policy,
            replicate,
        },
        &queued_spec,
    )
    .await
    .map_err(internal("queue simulation run"))?;

    register_and_enqueue(
        &txn,
        &payload,
        &format!("simulation {name} · {} #{replicate}", policy.as_str()),
    )
    .await?;
    // The queue's wake-up is a `pg_notify` fired by a trigger on
    // `agentic_task_queue` (`AddTaskQueueNotifyTrigger`), and Postgres holds
    // a NOTIFY until its transaction commits — so a worker is woken only for
    // a task it can actually claim, and never for one that rolled back.
    txn.commit().await.map_err(internal("commit queue"))?;

    tracing::info!(
        %workspace_id, %run_id, simulation = %name, policy = %policy.as_str(), replicate,
        "simulation run enqueued"
    );
    Ok(EnqueuedRun {
        run_id,
        simulation: name.to_string(),
        policy: policy.as_str().to_string(),
        replicate,
        seed,
    })
}

/// Write the run row, then queue its task.
///
/// In that order and never the other way: `agentic_task_queue.run_id` has an FK
/// to `agentic_runs.id`. Same UUID for both — one task per run, no fan-out
/// below this level. Generic over the connection so it runs inside
/// [`queue_one`]'s transaction.
async fn register_and_enqueue(
    db: &impl ConnectionTrait,
    payload: &SimulationRunPayload,
    label: &str,
) -> Result<(), ApiError> {
    let task_id = payload.run_id.to_string();
    agentic_runtime::crud::insert_run(
        db,
        &task_id,
        label,
        None,
        SIMULATION_RUN_KIND,
        Some(serde_json::json!({
            "workspace_id": payload.workspace_id,
            "simulation": payload.spec["name"],
            "policy": payload.policy.as_str(),
            "replicate": payload.replicate,
        })),
        payload.workspace_id,
    )
    .await
    .map_err(internal("register simulation run"))?;

    let task_spec = agentic_core::delegation::TaskSpec::Custom {
        kind: SIMULATION_RUN_KIND.to_string(),
        payload: serde_json::to_value(payload).map_err(internal("encode payload"))?,
    };
    agentic_runtime::crud::enqueue_task(
        db,
        &task_id,
        &task_id,
        None,
        &task_spec,
        None,
        agentic_runtime::orchestrator::crud::queue::TaskScope::Global,
    )
    .await
    .map_err(internal("enqueue simulation run"))?;
    Ok(())
}

/// The seed for one draw of a world.
///
/// Replicate 0 is the declared seed, so a single-replicate world reproduces
/// exactly what it did before replicates existed — and a recorded run stays
/// reproducible from its file. Later draws walk the seed by one: `Rng::stream`
/// XORs it with an FNV-hashed label and runs it through SplitMix64, which is a
/// mixer, so consecutive seeds are independent worlds rather than shifted ones.
fn replicate_seed(base: u64, replicate: u32) -> u64 {
    base.wrapping_add(replicate as u64)
}
