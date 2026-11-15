//! Procedure dry runs: staff run a procedure of a previewed branch; it runs on
//! the worker fleet against the branch's staging revision, reads and agent
//! steps run for real, managed-Airhouse writes land in the preview's own
//! schemas (phase 2b), and every other write is held and reported as "would
//! have written". A change check queues the same kind of run for each
//! pure-Airhouse transform the branch changed (`transform_build`), which the
//! same queue starts; its compare with live is `previews::compare`. An Airway
//! sample (`airway_sample`, phase 2b S11) is a bounded window of a branch's
//! pipeline into the preview's own schemas, queued on the same queue and run
//! by `previews::sample`.
//!
//! Only rows are written here — nothing runs in a handler:
//!
//! 1. [`submit`] records a `workspace_preview_runs` row (`queued`) and calls
//!    [`advance`].
//! 2. [`advance`] starts the oldest queued run of a workspace when none is
//!    running, and seeds its `agentic_runs` row and root task
//!    (`agentic_pipeline::automation_run::seed_preview_automation_run`, or
//!    `previews::sample::seed` for a sample) in the same transaction. Preview work serialises per workspace; the
//!    `one_running` partial unique index settles a race.
//! 3. The global-run driver claims the task and drives it with the preview
//!    platform (`previews::runtime::PreviewRunResolver`).
//! 4. [`sweep`], on the driver's periodic tick, retires runs past the
//!    wall-clock ceiling ([`retire_overdue`], `OXY_PREVIEW_RUN_MAX_MINUTES`),
//!    finishes rows whose run is terminal ([`mark_finished`]), settles the poll
//!    row of a custom app's automation run that ran as a preview
//!    (`settle_custom_app_runs`) and advances each workspace's queue.
//!
//! Gated by `OXY_PREVIEW_RUNS`: every runs route answers `404
//! preview_runs_disabled` without it, and the sweep starts nothing. Enable it
//! only once every serve, ide and worker replica runs this release.

mod notes;
mod report;
mod submit;
#[cfg(test)]
mod tests;

// The queue step and the run lifecycle are the half that stays below when the
// rest of previews leaves `oxy-app`: the compile worker, the custom-app
// automation run's preview mode and its settle call them (`oxy_app::surface::previews::queue`).
pub use oxy_app::surface::previews::queue::lifecycle::{
    TERMINAL_RUN_STATUSES, cancel_queued, cancel_queued_at,
};
pub use oxy_app::surface::previews::queue::run::{RUNS_FLAG_ENV, advance, runs_enabled};
// The per-pass sweep (and the ceiling it applies) is rows only, so it stays
// below too: the global driver calls it inline (`queue::sweep`).
pub use oxy_app::surface::previews::queue::ceiling::{
    MAX_MINUTES_ENV, max_minutes, retire_overdue,
};
pub use oxy_app::surface::previews::queue::sweep::{mark_finished, sweep};

pub(crate) use report::outcome_of;
pub use report::{
    CopyNote, HeldNote, Redirect, RedirectNote, RunDetail, RunStep, RunSummary, get, list,
};
pub use submit::{SubmitRun, Submitted, submit};

use axum::http::StatusCode;
use sea_orm::DbErr;

use agentic_airway::preview::SampleRefusal;

/// Why a runs request was refused, in the contract's codes.
#[derive(Debug, thiserror::Error)]
pub enum RunRequestError {
    #[error("workspace preview runs are not enabled on this deployment")]
    Disabled,
    #[error("there is no preview of branch {0}")]
    PreviewNotFound(String),
    #[error("the preview of {0} has no ready staging revision yet")]
    NotReady(String),
    #[error("{0} is not an automation in the previewed revision")]
    RefNotInRevision(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("there is no preview run {0}")]
    RunNotFound(String),
    /// An Airway sample the rules refuse (`agentic_airway::preview`).
    #[error("{0}")]
    Sample(SampleRefusal),
    #[error("{0}")]
    Internal(String),
}

impl RunRequestError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Disabled => "preview_runs_disabled",
            Self::PreviewNotFound(_) => "preview_not_found",
            Self::NotReady(_) => "preview_not_ready",
            Self::RefNotInRevision(_) => "ref_not_in_revision",
            Self::BadRequest(_) => "bad_request",
            Self::RunNotFound(_) => "run_not_found",
            Self::Sample(refusal) => refusal.code(),
            Self::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Disabled
            | Self::PreviewNotFound(_)
            | Self::RefNotInRevision(_)
            | Self::RunNotFound(_) => StatusCode::NOT_FOUND,
            Self::NotReady(_) => StatusCode::CONFLICT,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Sample(refusal) => match refusal.code() {
                "sample_refused" | "sample_unsupported" => StatusCode::UNPROCESSABLE_ENTITY,
                "sandbox_required" => StatusCode::CONFLICT,
                _ => StatusCode::BAD_REQUEST,
            },
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<DbErr> for RunRequestError {
    fn from(e: DbErr) -> Self {
        Self::Internal(format!("database error: {e}"))
    }
}
