//! Pre-registered experiments — the design artifact (`ExperimentConfig`), stored
//! as insert-only versions by the host.
//!
//! Pure: no database, no HTTP, no `agentic-*` import, no `airlayer`. The host's
//! design store uses [`validate_static`] to refuse a version before it is
//! written; this crate also owns the run state machine, compliance and the
//! report. See `internal-docs/experiments.md`.
//!
//! A **draft** and a **registrable** version are the same artifact at two stages:
//! a draft lacks the assignment (`design.waves`, or `design.switchback.periods`),
//! `design.seed` and `outcome.mde_at_proposal` — pricing's output, with
//! `assign_by`, `design.strata` and `outcome.prior_coefficient` — and is stored
//! cleanly; [`is_registrable`] is what refuses it.

pub mod compliance;
pub mod config;
pub mod engine_in;
pub mod eval;
pub mod expr;
pub mod grain;
pub mod interim;
pub mod moment;
pub mod patch;
pub mod report;
pub mod run;
pub mod timeline;
pub mod validate;

mod suggest;

#[cfg(test)]
mod eval_tests;
#[cfg(test)]
mod interim_tests;
#[cfg(test)]
mod test_fixtures;

pub use config::{
    Assignment, DesignConfig, Direction, ExperimentConfig, FilterOp, Guardrail, GuardrailAction,
    GuardrailCriteria, LeverConfig, LeverPer, LeverState, MeasureFilter, OnClose, OutcomeConfig,
    PeriodConfig, PriorCoefficient, PriorSource, ReadBack, SubstitutionCheck, SwitchbackConfig,
    ViaRef, WaveConfig, parse,
};
pub use grain::Grain;
pub use moment::Moment;
pub use validate::{
    ConfigError, is_registrable, is_retrospective_ready, validate_against_units, validate_static,
};
