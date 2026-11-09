//! The two caps on queueing, and the page a listing reads.
//!
//! Pure functions over numbers, so the arithmetic that decides a 400 or a 429
//! is assertable without a database — the handler only supplies the counts.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use super::super::ApiError;

/// Runs are cheap to queue and expensive to execute — a 40-period run is
/// minutes of warehouse queries — so one request cannot silently commit the
/// fleet to hours of work. Five arms over ten replicates is already 50.
pub const MAX_RUNS_PER_REQUEST: usize = 64;

/// How many runs one workspace may have queued or running at once.
///
/// [`MAX_RUNS_PER_REQUEST`] bounds one click; this bounds the member who
/// clicks a dozen times. A **cap**, deliberately, not a lease: Airway takes a
/// per-pipeline lease because concurrent runs share a cursor and would fold
/// overlapping snapshots into one table. Simulation runs are independent —
/// each reads its own spec snapshot and writes its own rows — so nothing is
/// wrong with two of them running at once. The only concern is fleet
/// capacity, and a count answers that.
///
/// This once read the count outside any transaction and argued that "a cap
/// that is briefly off by one request is the right cost for not locking a
/// table nothing else contends on". That reasoning priced the race at one
/// request because it only ever imagined two. The cost is not bounded by the
/// overshoot of a single racer: every concurrent request reads the same stale
/// count, so the real bound was `MAX_IN_FLIGHT_PER_WORKSPACE × concurrency`,
/// and concurrency is the caller's to choose. Eight simultaneous requests of
/// sixteen queued 128 runs against this cap of 64 — measured, not reasoned
/// about, in `the_in_flight_cap_holds_when_requests_arrive_together`. Since
/// the route is reachable by any workspace member and each run is minutes of
/// blocking DuckDB and fitter CPU, that is a member-triggered denial of the
/// worker fleet, not an off-by-one.
///
/// So the count is now read under [`advisory_lock_key`] inside the same
/// transaction that inserts the run — see `runs::queue_one`. The lock is held
/// for one run's three writes, contends only with other queueing in the same
/// workspace, and leaves the "independent runs, no lease" design intact: it
/// serialises the *decision*, not the runs.
pub const MAX_IN_FLIGHT_PER_WORKSPACE: u64 = 64;

/// Stable 64-bit key for the `pg_advisory_xact_lock` that serialises queueing
/// within one workspace.
///
/// Namespaced by a fixed discriminant so it cannot collide with another
/// subsystem's advisory lock on the same database — `secret_manager` hashes a
/// `(project, name)` pair into the same 64-bit space, and a shared key there
/// would make two unrelated features queue behind each other. Deterministic
/// across processes (a fixed-key hasher), so it serialises across the fleet
/// and not merely within one replica. A collision between two workspaces
/// costs an unnecessary wait, never a wrong answer: the count inside the lock
/// is still filtered by `workspace_id`.
pub fn advisory_lock_key(workspace_id: uuid::Uuid) -> i64 {
    use std::hash::{Hash, Hasher};
    /// Distinguishes this lock from every other advisory lock in the process.
    const NAMESPACE: &str = "oxy::simulation::in_flight";
    let mut h = std::collections::hash_map::DefaultHasher::new();
    NAMESPACE.hash(&mut h);
    workspace_id.hash(&mut h);
    h.finish() as i64
}

/// How long a run may sit at `queued` or `running` before it stops counting
/// against [`MAX_IN_FLIGHT_PER_WORKSPACE`].
///
/// The cap counts rows, and a row only leaves `queued`/`running` when a worker
/// writes a terminal status. That write is best-effort — the executor says so
/// out loud (`simulation::run_simulation_task`): if it fails, the run
/// stays `running` with nothing left alive to move it. Without an age bound,
/// 64 such rows would lock the workspace out of queueing forever, with no
/// recourse from the API at all.
///
/// Six hours rather than something tighter: a 40-period run is minutes, so
/// this is an order of magnitude past the longest honest run and cannot
/// discount one that is genuinely still going. Measured from `queued_at`, not
/// `started_at`, because a run abandoned *before* a worker claimed it never
/// restamps `started_at`.
pub const IN_FLIGHT_MAX_AGE_HOURS: i64 = 6;

/// The oldest `queued_at` that still counts against the cap.
///
/// Takes `now` rather than reading the clock so the boundary is assertable.
pub fn in_flight_cutoff(now: DateTime<Utc>) -> DateTime<Utc> {
    now - Duration::hours(IN_FLIGHT_MAX_AGE_HOURS)
}

/// How many runs `arms × replicates` is, or a 400 if that is over the
/// per-request limit.
pub fn check_request_size(arms: usize, replicates: u32) -> Result<usize, ApiError> {
    let total = arms * replicates as usize;
    if total > MAX_RUNS_PER_REQUEST {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "{arms} arms × {replicates} replicates is {total} runs, over the limit of \
                 {MAX_RUNS_PER_REQUEST}. Each one is minutes of warehouse queries."
            ),
        ));
    }
    Ok(total)
}

/// Refuse a request that would push the workspace over
/// [`MAX_IN_FLIGHT_PER_WORKSPACE`], naming both numbers.
///
/// 429 rather than 400: the request is well-formed, and the same request
/// succeeds once the runs ahead of it finish.
///
/// The message names only actions the API actually offers. There is no cancel
/// route, so it points at the two things that are true: runs finish, and a run
/// that never will ages out of the count after
/// [`IN_FLIGHT_MAX_AGE_HOURS`].
pub fn check_in_flight(in_flight: u64, requested: usize) -> Result<(), ApiError> {
    let after = in_flight.saturating_add(requested as u64);
    if after > MAX_IN_FLIGHT_PER_WORKSPACE {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "{in_flight} runs are already queued or running in this workspace; {requested} \
                 more would be {after}, over the limit of {MAX_IN_FLIGHT_PER_WORKSPACE}. Wait \
                 for some to finish; a run that never does stops counting \
                 {IN_FLIGHT_MAX_AGE_HOURS}h after it was queued."
            ),
        ));
    }
    Ok(())
}

/// `?limit=&offset=` on `GET /simulations/runs`.
///
/// Doubles as the query extractor: both fields are optional on the wire, and
/// the accessors apply the defaults and the ceiling so a handler never reads
/// the raw options.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct RunPage {
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    offset: Option<u64>,
}

impl RunPage {
    pub const DEFAULT_LIMIT: u64 = 100;
    /// Clamped rather than rejected: a caller asking for more than this gets
    /// this, which is what they would have paged to anyway.
    pub const MAX_LIMIT: u64 = 1000;

    pub fn new(limit: Option<u64>, offset: Option<u64>) -> Self {
        Self { limit, offset }
    }

    pub fn limit(self) -> u64 {
        self.limit
            .unwrap_or(Self::DEFAULT_LIMIT)
            .clamp(1, Self::MAX_LIMIT)
    }

    pub fn offset(self) -> u64 {
        self.offset.unwrap_or(0)
    }
}
