//! What `POST /simulations/{name}/runs` answers with.
//!
//! The fan-out queues one run per (arm, draw) in its own transaction, so a
//! failure on arm `k` leaves arms `0..k` queued *and executing*. The response
//! has to say so: a bare 500 would hide runs the fleet is already spending
//! minutes on, and a caller retrying would double them.

use std::ops::Deref;

use oxy_simulation::PolicyKind;
use serde::Serialize;
use uuid::Uuid;

use super::super::ApiError;

#[derive(Debug, Serialize)]
pub struct EnqueuedRun {
    pub run_id: Uuid,
    pub simulation: String,
    /// The arm, spelled the way the run row stores it.
    pub policy: String,
    /// Which draw of the world this is; `0` is the world's declared seed.
    pub replicate: u32,
    /// The seed this run actually got, so a caller can reproduce one draw
    /// without re-deriving it.
    pub seed: u64,
}

/// The runs a request queued, and whether that was all of them.
///
/// `partial_failure` is `Some` exactly when the fan-out stopped early: the
/// runs listed are real and running, the ones after the named arm never
/// existed. A request that queued *nothing* is an error, not an empty one of
/// these — see [`QueuedRuns::absorb_failure`].
#[derive(Debug, Default, Serialize)]
pub struct QueuedRuns {
    pub runs: Vec<EnqueuedRun>,
    /// Always serialised, `null` when the fan-out completed — the shape
    /// `web-app/src/types/simulation.ts` mirrors as `string | null`. No
    /// `skip_serializing_if`: a reader checking for the note should find the
    /// key and see it is null rather than have to distinguish "absent" from
    /// "nothing went wrong".
    pub partial_failure: Option<String>,
}

/// The arm the fan-out died on, for the note.
#[derive(Debug, Clone, Copy)]
pub struct FailedArm {
    pub policy: PolicyKind,
    pub replicate: u32,
    /// How many runs the request asked for in total.
    pub total: usize,
}

impl QueuedRuns {
    /// What the fan-out does when one arm fails.
    ///
    /// Nothing queued yet → the error, unchanged: the caller has nothing to
    /// keep and the status code still means what it says. Something queued →
    /// those runs, plus a note naming what did not happen. The status is 200
    /// in that case because the body is a list of runs the fleet is executing,
    /// and a caller who reads only the code would otherwise retry them.
    pub fn absorb_failure(mut self, failed: FailedArm, err: ApiError) -> Result<Self, ApiError> {
        if self.runs.is_empty() {
            return Err(err);
        }
        let (status, message) = err;
        self.partial_failure = Some(format!(
            "queued {} of {} runs; {} #{} failed ({}: {}) and the arms after it were not \
             queued. The runs listed are executing.",
            self.runs.len(),
            failed.total,
            failed.policy.as_str(),
            failed.replicate,
            status.as_u16(),
            message,
        ));
        Ok(self)
    }
}

/// A `QueuedRuns` reads as its list of runs.
///
/// Every reader of the fan-out that predates the note — `queued.len()`,
/// `queued[0]`, `for run in &queued` — keeps working, and the note is there
/// for the one that asks.
impl Deref for QueuedRuns {
    type Target = [EnqueuedRun];

    fn deref(&self) -> &Self::Target {
        &self.runs
    }
}

impl<'a> IntoIterator for &'a QueuedRuns {
    type Item = &'a EnqueuedRun;
    type IntoIter = std::slice::Iter<'a, EnqueuedRun>;

    fn into_iter(self) -> Self::IntoIter {
        self.runs.iter()
    }
}
