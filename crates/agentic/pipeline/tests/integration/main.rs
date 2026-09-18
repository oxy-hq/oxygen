//! Integration tests for `agentic-pipeline`, in ONE binary.
//!
//! Cargo compiles every `tests/*.rs` into its own executable, each statically
//! linking the whole dependency graph — so a test *file* costs a link, not just
//! a compile. Grouping keeps that cost proportional to crates rather than to
//! files. Add a case as a `mod` below, not as a new `tests/*.rs`.
//!
//! See `internal-docs/testing.md` for the cost model.

mod airway_config_test;
mod airway_reset_in_place_resume_test;
mod airway_retry_count_test;
mod airway_run_history_scope_test;
mod airway_run_test;
mod automation_airway_admission_test;
mod automation_cache_resume_test;
mod automation_recovery_test;
mod commit_decision_test;
mod integration_tests;
mod scheduler_test;
