//! Integration tests for `oxy (core)`, in ONE binary.
//!
//! Cargo compiles every `tests/*.rs` into its own executable, each statically
//! linking the whole dependency graph — so a test *file* costs a link, not just
//! a compile. Grouping keeps that cost proportional to crates rather than to
//! files. Add a case as a `mod` below, not as a new `tests/*.rs`.
//!
//! See `internal-docs/testing.md` for the cost model.

mod config_parsing;
mod omni_query_workflow;
// `semantic_variables` spawned `oxy run` to show an automation parses; with
// `oxyc run` (2026-11-24) it is `agentic-automation`'s `fixture_parse_tests`,
// which parses the fixture with the type the server runs it as.
mod serve;
mod strict_validation;
mod sync;
mod validation;
