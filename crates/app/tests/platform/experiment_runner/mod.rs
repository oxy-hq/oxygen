//! The experiment runner against a real Postgres (`fresh_db(Schema::All)`: the
//! lever jobs land in `agentic_runs` / `agentic_task_queue`, outside the
//! central chain). Clocks are injected everywhere; nothing here reads `now()`.

mod actuation;
mod batched;
mod check;
mod designs;
mod dispatch;
mod e2e;
mod failures;
pub(crate) mod fixture;
mod groups;
mod guardrails;
mod jobs;
mod observed;
mod periods;
mod registration;
mod resolution;
mod retrospective;
mod status;
mod switchback;
