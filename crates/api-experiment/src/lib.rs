//! The experiments HTTP surface: pre-registered experiments, and the declared
//! worlds (simulations) a workspace's model is run against.
//!
//! One bounded context, one crate (`internal-docs/domain-boundaries.md` S1).
//! [`experiment`] is the host half of experiments — pricing, registration, the
//! tick that advances a run, read-back and guardrails, the sealed estimate, the
//! interim reading, write-back; [`simulation`] is the simulation runner's host
//! half (the throwaway world directory, the probe, the run store); [`api`] is
//! the thin handlers over both. `oxy-experiment` owns the file, its refusals and
//! the calendar; `oxy-simulation` owns the loop; airlayer's `engine::experiment`
//! owns the statistics. This crate is the seam between those and Postgres, the
//! warehouse and the task queue — which is why it is a surface over
//! `oxy-app` and not a module of `oxy-experiment`, which stays pure.
//!
//! Three seams into the `oxy-server` composition root:
//!
//! * [`workspace_routes`] + [`workspace_route_roles`] — `SurfaceSeams::workspace`,
//!   merged INSIDE the `/{workspace_id}` nest, so the handlers inherit the auth
//!   stack and `workspace_middleware` exactly as they did in `oxy-app`'s
//!   `build_workspace_routes`. Every route is FleetOk — Postgres rows, the
//!   compile boundary, or an enqueue — except write-back apply, which creates a
//!   git worktree and branch in the working copy and is declared IdeOnly.
//! * [`tasks`] — `SurfaceSeams::tasks`: the five `TaskSpec::Custom` kinds the
//!   worker fleet runs for this context (a simulation run, the price job, the
//!   read-back check, the estimate, the interim reading).
//! * [`global_ticks`] — `SurfaceSeams::global_ticks`: `tick_experiments`, which
//!   the singleton global driver runs each pass beside the schedule ticks;
//!   exactly-once across replicas by the CAS on `experiment_runs.next_check_at`.
//!
//! Reaches `oxy-app` through `oxy_app::surface` (platform runtime: the role
//! guards, the workspace context extractors, the project-context and
//! workspace-manager builders, the compile-boundary scan resolver) and, until
//! the metric-tree logic goes down into `oxy-metric-monitoring`, the `BACKLOG`
//! rows `crates/app/tests/routing/surface_contract.rs` lists for it
//! (`server::api::metric_tree`, `server::api::metric_tree_groups`,
//! `agentic_wiring::metric_tree_runner`). `oxy-app` never depends back on it.

// Laying out the handler futures (SeaORM 2.0 query types under the workspace
// middleware stack) takes more than rustc's default query depth of 128 at
// opt-level 3, like every other surface crate. Raised before it overflows there.
#![recursion_limit = "256"]

pub mod api;
pub mod experiment;
pub mod simulation;

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::handler::Handler;
use axum::response::Response;
use axum::routing::{get, post};
use oxy_app::surface::fleet::IdeState;
use oxy_app::surface::seams::{GlobalTickSeam, TaskSeam};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

// The handler modules under the names the route table names them by: the
// route-catalog walker (`crates/route-catalog/build_route_catalog.rs`) resolves
// `module::handler` through a file's imports, as it did for `oxy-app`'s router.
use api::{experiment as experiment_api, simulation as simulation_api};

/// The workspace-scoped routes, relative to `/{workspace_id}`: declared worlds
/// and their runs, then experiments. Moved out of `oxy-app`'s
/// `build_workspace_routes` with their paths unchanged.
///
/// The simulation reads come from the compile boundary (`simulation_definitions`)
/// and Postgres (`simulation_run*`); the POST only enqueues a `TaskSpec`. A run
/// DOES touch local disk — a per-run `TempDir` — but on the worker, never in a
/// handler, which is the whole reason it is queued work rather than a spawn.
/// The profit race is a `workspace_id`-scoped join of `simulation_runs` onto
/// `simulation_run_periods` and nothing else.
///
/// Experiments are records. Every route reads or writes Postgres (`experiments`,
/// `experiment_designs`, `experiment_proposals`, `experiment_runs`,
/// `experiment_results`, `experiment_interims`) or enqueues a `TaskSpec`; the
/// warehouse queries run on the worker, so the POSTs are FleetOk like the
/// simulation POST. The one exception is write-back apply — see
/// [`workspace_route_roles`].
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .route("/simulations", get(simulation_api::list_simulations))
        .route(
            "/simulations/validate",
            post(simulation_api::validate_simulation),
        )
        .route("/simulations/runs", get(simulation_api::list_runs))
        .route("/simulations/runs/{run_id}", get(simulation_api::get_run))
        .route(
            "/simulations/{name}/runs",
            post(simulation_api::enqueue_run),
        )
        .route(
            "/simulations/{name}/race",
            get(simulation_api::get_profit_race),
        )
        .route(
            "/experiments",
            get(experiment_api::list_experiments).post(experiment_api::create_experiment),
        )
        .route(
            "/experiments/{name}/design",
            get(experiment_api::get_design).put(experiment_api::put_design),
        )
        .route(
            "/experiments/{name}/duplicate",
            post(experiment_api::duplicate_experiment),
        )
        .route(
            "/experiments/{name}/proposals/{proposal_id}/use",
            post(experiment_api::use_proposal),
        )
        .route(
            "/experiments/{name}/register",
            post(experiment_api::post_register),
        )
        .route(
            "/experiments/{name}/retrospective",
            post(experiment_api::post_retrospective),
        )
        .route(
            "/experiments/{name}/price",
            post(experiment_api::post_price),
        )
        .route(
            "/experiments/{name}/proposals",
            get(experiment_api::list_proposals),
        )
        // The GET reads `experiment_runs` / `experiment_flips`; the abort records
        // one flag that the global driver's tick actuates.
        .route("/experiments/{name}/run", get(experiment_api::get_run))
        .route("/experiments/{name}/abort", post(experiment_api::abort_run))
        // The sealed report: two Postgres reads; the 409 is decided in the
        // handler, on the run's state.
        .route(
            "/experiments/{name}/result",
            get(experiment_api::get_result),
        )
        // The interim reading of a run in progress: a Postgres read, and a POST
        // that enqueues an `experiment_interim` TaskSpec.
        .route(
            "/experiments/{name}/interim",
            get(experiment_api::get_interim).post(experiment_api::post_interim),
        )
        // Creates a git worktree + branch in the working copy: IdeOnly, see
        // `apply_write_back_on_ide` and `workspace_route_roles`.
        .route(
            "/experiments/{name}/write-back/apply",
            post(apply_write_back_on_ide),
        )
}

/// Write-back apply takes `WorkspaceRootWorkingCopy`, which resolves only from
/// `oxy-app`'s `IdeState` — the type gate that keeps a handler needing the
/// working copy off a fleet route. A seam router is a `Router<AppState>`, so
/// this adapter runs the handler on the ide state explicitly, and
/// [`workspace_route_roles`] declares the route IdeOnly: the same two facts
/// `route_ide` stated when the route lived inside `oxy-app`. A fleet replica
/// never sees the request — `role_middleware` relays it to the ide — so the
/// extractor's "no working copy here" answer is for a misconfigured fleet, not
/// a normal path.
async fn apply_write_back_on_ide(State(app): State<AppState>, req: Request) -> Response {
    Handler::call(experiment_api::apply_write_back, req, IdeState(app)).await
}

/// The routes in [`workspace_routes`] that may not take the FleetOk default.
/// Relative to `/{workspace_id}`, like `oxy-api-tenancy`'s onboarding
/// declarations: `oxy-server` merges this crate inside the workspace nest.
///
/// One entry, and the rest is deliberate: every other route here is Postgres,
/// the compile boundary or an enqueue, so it stays FleetOk by default — reading
/// a sealed result has to survive a deploy. Write-back apply creates a git
/// worktree and branch, which only the ide singleton holds. `"*"` as
/// `route_ide` declared it inside `oxy-app`.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    &[RouteRoleDecl {
        method: "*",
        path: "/experiments/{name}/write-back/apply",
        role: RouteRole::IdeOnly,
    }]
}

/// The `TaskSpec::Custom` kinds this context queues, each with the executor
/// that runs it, for `SurfaceSeams::tasks`. The kind strings are the persisted
/// task-queue contract (`agentic_task_queue` rows), so they never change with
/// the code that registers them. A kind absent from the registry fails its
/// tasks as unknown; `oxy-server`'s composition test holds the list to the
/// enqueue sites.
pub fn tasks() -> Vec<TaskSeam> {
    vec![
        // One declared world, one policy, one replicate: minutes of warehouse
        // queries, so queued rather than spawned.
        TaskSeam {
            kind: simulation::SIMULATION_RUN_KIND,
            build: |deps| {
                Arc::new(simulation::SimulationTaskExecutor {
                    db: deps.db.clone(),
                })
            },
        },
        // The price job: one per candidate design, settling a `pending`
        // proposal row.
        TaskSeam {
            kind: experiment::EXPERIMENT_PRICE_KIND,
            build: |deps| {
                Arc::new(experiment::ExperimentPriceExecutor {
                    db: deps.db.clone(),
                })
            },
        },
        // Read-back and guardrails: the warehouse reads the tick must not make
        // itself. Context rebuilt per task from the run's workspace.
        TaskSeam {
            kind: experiment::jobs::CHECK_KIND,
            build: |deps| {
                Arc::new(experiment::check::executor::ExperimentCheckExecutor {
                    db: deps.db.clone(),
                })
            },
        },
        // The sealed report of a closed or aborted run: the tick enqueues one
        // per run.
        TaskSeam {
            kind: experiment::jobs::ESTIMATE_KIND,
            build: |deps| {
                Arc::new(experiment::estimate::ExperimentEstimateExecutor::warehouse(
                    deps.db.clone(),
                ))
            },
        },
        // The interim reading of a live or closing run: on demand, and once a
        // day by the tick.
        TaskSeam {
            kind: experiment::interim::store::INTERIM_KIND,
            build: |deps| {
                Arc::new(experiment::interim::ExperimentInterimExecutor::warehouse(
                    deps.db.clone(),
                ))
            },
        },
    ]
}

/// The singleton work the global driver runs for this context each pass, for
/// `SurfaceSeams::global_ticks`: the experiment tick, which advances every due
/// run this replica claims (registration is an explicit action, never the
/// tick's). Exactly-once across replicas by the CAS on
/// `experiment_runs.next_check_at`, so several eligible nodes running it is safe.
pub fn global_ticks() -> Vec<GlobalTickSeam> {
    vec![GlobalTickSeam {
        name: "experiment_tick",
        tick: |db, now| Box::pin(experiment::tick::tick_experiments(db, now)),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The persisted task-queue contract: these strings are in
    /// `agentic_task_queue` rows, so a change here strands queued work.
    #[test]
    fn the_task_kinds_are_the_five_the_fleet_knew() {
        let kinds: Vec<&str> = tasks().iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [
                "simulation_run",
                "experiment_price",
                "experiment_check",
                "experiment_estimate",
                "experiment_interim",
            ]
        );
    }

    /// The seam routes compose on their own: axum rejects a colliding path
    /// eagerly on `.route`, which would otherwise surface only as a boot panic.
    #[test]
    fn the_workspace_routes_compose() {
        let _: Router<()> = workspace_routes().with_state(oxy_app::surface::bare_app_state());
    }

    /// Every route is mounted — the half of "declared by a mount" that the
    /// classification test in `tests/integration/route_roles.rs` cannot see (an
    /// unmounted path classifies FleetOk by default). Driven through the router
    /// nested at `/{workspace_id}` as `oxy-server` nests it, with no middleware
    /// and no database: whatever each route answers — an extractor rejection, a
    /// connection error — it is never axum's unmatched-route fallback (an empty
    /// 404) or a 405, which is what a dropped or misspelled mount answers.
    #[tokio::test]
    async fn every_route_is_mounted() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let router: Router<()> = Router::new()
            .nest("/{workspace_id}", workspace_routes())
            .with_state(oxy_app::surface::bare_app_state());
        let ws = uuid::Uuid::nil();
        let id = uuid::Uuid::nil();
        for (method, path) in [
            ("GET", format!("/{ws}/simulations")),
            ("POST", format!("/{ws}/simulations/validate")),
            ("GET", format!("/{ws}/simulations/runs")),
            ("GET", format!("/{ws}/simulations/runs/{id}")),
            ("POST", format!("/{ws}/simulations/flat/runs")),
            ("GET", format!("/{ws}/simulations/flat/race")),
            ("GET", format!("/{ws}/experiments")),
            ("POST", format!("/{ws}/experiments")),
            ("GET", format!("/{ws}/experiments/promo_pilot/design")),
            ("PUT", format!("/{ws}/experiments/promo_pilot/design")),
            ("POST", format!("/{ws}/experiments/promo_pilot/duplicate")),
            (
                "POST",
                format!("/{ws}/experiments/promo_pilot/proposals/{id}/use"),
            ),
            ("POST", format!("/{ws}/experiments/promo_pilot/register")),
            (
                "POST",
                format!("/{ws}/experiments/promo_pilot/retrospective"),
            ),
            ("POST", format!("/{ws}/experiments/promo_pilot/price")),
            ("GET", format!("/{ws}/experiments/promo_pilot/proposals")),
            ("GET", format!("/{ws}/experiments/promo_pilot/run")),
            ("POST", format!("/{ws}/experiments/promo_pilot/abort")),
            ("GET", format!("/{ws}/experiments/promo_pilot/result")),
            ("GET", format!("/{ws}/experiments/promo_pilot/interim")),
            ("POST", format!("/{ws}/experiments/promo_pilot/interim")),
            (
                "POST",
                format!("/{ws}/experiments/promo_pilot/write-back/apply"),
            ),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(&path)
                .body(Body::empty())
                .unwrap();
            let response = router
                .clone()
                .oneshot(request)
                .await
                .expect("router responded");
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap_or_default();
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} is mounted for a different method"
            );
            assert!(
                !(status == StatusCode::NOT_FOUND && body.is_empty()),
                "{method} {path} is not mounted — the router fell through"
            );
        }
    }

    #[test]
    fn the_tick_is_handed_over_by_name() {
        assert_eq!(global_ticks().len(), 1);
        assert_eq!(global_ticks()[0].name, "experiment_tick");
    }
}
