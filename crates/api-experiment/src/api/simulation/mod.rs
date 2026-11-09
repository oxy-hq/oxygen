//! HTTP surface for declared worlds and their runs.
//!
//! Thin by construction: enqueue, list, read. Every route here reads or writes
//! **persisted rows only** — `simulation_definitions` through the compile
//! boundary, `simulation_run*` through Postgres — so all of them are `FleetOk`
//! and none belongs in `IDE_ONLY_PATTERNS`. The world's rows never touch node
//! local disk outside the runner's own `TempDir`, which lives on the worker.
//!
//! Split along the line the schema itself draws: [`worlds`] is *what happens*
//! (the declared `.simulation.yml` grid, resolved through the compile boundary
//! or the working copy), [`runs`] is *what someone does about it* (an arm and a
//! draw, queued and read back), and [`validate`] is neither — a check on a
//! spec nobody has written to a file yet. A world carries no policy, so a file
//! that listed both would be describing two different things.

pub mod runs;
pub mod validate;
pub mod worlds;

use axum::http::StatusCode;
use sea_orm::DatabaseConnection;

pub use runs::{
    ArmCoverage, ArmScore, EnqueuedRun, MAX_IN_FLIGHT_PER_WORKSPACE, MAX_RUNS_PER_REQUEST,
    PairedTestResult, ProfitRace, QueuedRuns, RaceComparison, RaceOptions, RaceQuery, RaceRunRow,
    ReplicateReach, RunDetail, RunPage, RunQuery, RunRequest, ScoredRun, SetAside,
    TERMINAL_STATUSES, enqueue_run, get_profit_race, get_run, list_runs, list_workspace_runs,
    list_workspace_runs_page, parse_policies, profit_race_report, read_run, start_run,
};
pub use validate::{ValidateResponse, validate_simulation};
pub use worlds::{SimulationSummary, list_simulations, list_worlds};

pub(crate) type ApiError = (StatusCode, String);

pub(crate) async fn connect() -> Result<DatabaseConnection, ApiError> {
    oxy::database::client::establish_connection()
        .await
        .map_err(internal("connect"))
}

pub(crate) fn internal<E: std::fmt::Display>(what: &'static str) -> impl Fn(E) -> ApiError {
    move |e| {
        tracing::error!(error = %e, "simulation: {what} failed");
        (StatusCode::INTERNAL_SERVER_ERROR, format!("{what} failed"))
    }
}
