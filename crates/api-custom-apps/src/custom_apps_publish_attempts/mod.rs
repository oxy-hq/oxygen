//! Every custom-app publish, the ones that failed as well as the ones that
//! landed, with how long each stage of it took.
//!
//! `app_builds` records a publish that succeeded; one that failed part way is
//! undone and left nothing. The pipeline (`custom_apps_publish::pipeline`) now
//! clocks each of its stages with a [`StageTimer`] and writes one
//! `app_publish_attempts` row per publish through [`Attempt`]: the outcome,
//! the stage a failure happened at, the refusal, and `{stage: ms}`. The
//! console reads them back through `GET /api/customer-apps/{id}/publishes`
//! ([`recent`]), and `oxyc publish` prints the timings a publish answers.

mod list;
mod record;
mod stages;

pub use list::{DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT, clamp_limit, recent};
pub use record::{
    Attempt, AttemptFacts, KEEP_ATTEMPTS, MAX_BUILD_ID_CHARS, MAX_ERROR_CHARS, MAX_LABEL_CHARS,
};
pub use stages::{Stage, StageTimer, StageTiming, timings_from_json};

/// A refused publish, with the stage it was refused at — `None` when it was
/// refused before the pipeline's first stage.
#[derive(Debug)]
pub struct Failed<E> {
    pub error: E,
    pub stage: Option<Stage>,
}

impl<E> Failed<E> {
    /// A refusal outside any stage.
    pub fn unstaged(error: E) -> Self {
        Self { error, stage: None }
    }

    /// The same refusal at the same stage, told differently.
    pub fn map<F>(self, f: impl FnOnce(E) -> F) -> Failed<F> {
        Failed {
            error: f(self.error),
            stage: self.stage,
        }
    }
}
