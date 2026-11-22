//! `oxyc evals run`'s server half: an eval run queued for the worker fleet.
//!
//! - `POST /api/{workspace_id}/evals/runs` resolves the `.test.yml` files from
//!   the served revision's compile boundary (`eval_test_definitions`, through
//!   the config manager's `eval_test`), inserts an `agentic_runs` row and enqueues
//!   one `TaskSpec::Custom { kind: "eval_run" }` (long, and it calls an LLM, so
//!   never a handler spawn). Answers `202 { run_id, test_files }`.
//! - `GET /api/{workspace_id}/evals/runs/{run_id}` reads that row back: its
//!   status, and once done the results document, byte for byte what
//!   `oxy test --output-json` wrote ([`crate::reporters::results_json`]).
//!
//! The executor ([`executor::EvalRunExecutor`], registered through
//! `SurfaceSeams::tasks`) builds the workspace's context over the PROMOTED
//! revision (`workspace_runtime::build_workspace_ctx`, as the health eval
//! does), carrying the caller's identity so an `airhouse_managed` query runs
//! as them, and runs the same engine `oxy test` runs.
//!
//! Both routes are FleetOk: Postgres reads and one enqueue, no disk. The guard
//! is the one starting an agent run has (`POST …/analytics/runs`): the
//! workspace middleware's membership check plus an authenticated user, no
//! narrower role.

pub mod executor;
pub(crate) mod handlers;
pub mod request;
