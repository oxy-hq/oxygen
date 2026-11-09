//! PR 5 — the sealed report, the estimate job and write-back, against a real database.
//!
//! Every function under test takes an explicit `&DatabaseConnection`, so nothing
//! here reaches the shared `OXY_DATABASE_URL` (`authz::shared_db_registry`).

mod estimate;
mod estimate_abort;
mod estimate_carryover;
mod estimate_designs;
mod estimate_fully_staggered;
mod estimate_retrospective;
mod estimate_retry;
mod estimate_switchback;
mod estimate_tick;
mod fixtures;
mod harness;
mod interim;
mod sealed;
mod table;
mod write_back;
