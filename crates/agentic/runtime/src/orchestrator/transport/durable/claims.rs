//! Per-claim bookkeeping for [`super::DurableTransport`]: the cancel tokens and
//! heartbeat tickers of every execution this process is running.
//!
//! `worker_id` is process-stable, so a task id alone cannot tell an execution
//! that lost its claim from this same process's later re-claim of the row.
//! Everything here is therefore keyed by `(task_id, claim_epoch)`, and an old
//! claim's entry can only ever be touched through its own epoch.

use std::collections::HashMap;

use agentic_core::delegation::{ClaimEpoch, NO_CLAIM};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use tokio_util::sync::CancellationToken;

/// The epoch to put in an owned queue predicate: `None` (unfenced) for work
/// with no claim identity, `Some(epoch)` otherwise.
pub(super) fn fence(claim: ClaimEpoch) -> Option<i64> {
    (claim != NO_CLAIM).then_some(claim)
}

/// Everything cancellable under one task id.
#[derive(Default)]
struct Slot {
    /// Fired by a Stop, whichever claim is (or will be) executing. Also the
    /// token of work with no claim ([`NO_CLAIM`]). A fired `stop` makes every
    /// later claim's token start cancelled, so a Stop whose queue write failed
    /// is still honoured by a re-claim.
    stop: CancellationToken,
    /// One token per claim that has asked for one. A heartbeat that loses its
    /// claim cancels only its own entry, so a re-claim never starts cancelled.
    claims: HashMap<ClaimEpoch, CancellationToken>,
}

/// Cancel tokens, keyed by task id and then by claim.
#[derive(Default)]
pub(super) struct ClaimTokens(DashMap<String, Slot>);

impl ClaimTokens {
    /// A task was (re-)assigned: start from a fresh, un-fired slot.
    pub(super) fn arm(&self, task_id: &str) {
        self.0.insert(task_id.to_string(), Slot::default());
    }

    /// The token for `claim` of `task_id`, created on first ask.
    pub(super) fn token(&self, task_id: &str, claim: ClaimEpoch) -> CancellationToken {
        let mut slot = self.0.entry(task_id.to_string()).or_default();
        if claim == NO_CLAIM {
            return slot.stop.clone();
        }
        let stopped = slot.stop.is_cancelled();
        slot.claims
            .entry(claim)
            .or_insert_with(|| {
                let token = CancellationToken::new();
                if stopped {
                    token.cancel();
                }
                token
            })
            .clone()
    }

    /// A Stop: fire the stop token and every claim's token of `task_id`.
    pub(super) fn stop(&self, task_id: &str) {
        if let Some(slot) = self.0.get(task_id) {
            Self::fire_slot(&slot);
        }
    }

    /// A Stop for `root_task_id` and every `root.<n>…` descendant.
    pub(super) fn stop_subtree(&self, root_task_id: &str) {
        let prefix = format!("{root_task_id}.");
        for entry in self.0.iter() {
            if entry.key() == root_task_id || entry.key().starts_with(&prefix) {
                Self::fire_slot(entry.value());
            }
        }
    }

    fn fire_slot(slot: &Slot) {
        slot.stop.cancel();
        slot.claims.values().for_each(CancellationToken::cancel);
    }

    /// Cancel one claim's execution, leaving the others (and `stop`) alone.
    pub(super) fn cancel_claim(&self, task_id: &str, claim: ClaimEpoch) {
        if let Some(slot) = self.0.get(task_id)
            && let Some(token) = slot.claims.get(&claim)
        {
            token.cancel();
        }
    }

    /// Forget a claim that ended, bounding the map to live claims.
    pub(super) fn release(&self, task_id: &str, claim: ClaimEpoch) {
        if let Some(mut slot) = self.0.get_mut(task_id) {
            slot.claims.remove(&claim);
        }
    }
}

/// The live heartbeat ticker of each claimed task: at most one per task id,
/// and always the newest claim's.
#[derive(Default)]
pub(super) struct Heartbeats(DashMap<String, (ClaimEpoch, CancellationToken)>);

impl Heartbeats {
    /// Register `ticker` as the heartbeat of `claim`. An older claim's ticker is
    /// retired and its epoch returned: that execution is stale by definition,
    /// and the caller cancels it (a ticker retired silently would leave the old
    /// copy running on). A *newer* claim's ticker wins over a late registration,
    /// which is retired instead. An unfenced ([`NO_CLAIM`]) registration — a
    /// virtual worker driving a root — replaces whatever is there, as the
    /// task_id-keyed registry always did.
    pub(super) fn register(
        &self,
        task_id: &str,
        claim: ClaimEpoch,
        ticker: &CancellationToken,
    ) -> Option<ClaimEpoch> {
        match self.0.entry(task_id.to_string()) {
            Entry::Vacant(v) => {
                v.insert((claim, ticker.clone()));
                None
            }
            Entry::Occupied(mut o) => {
                let (held, held_ticker) = o.get().clone();
                if claim == NO_CLAIM {
                    held_ticker.cancel();
                    o.insert((claim, ticker.clone()));
                    return None;
                }
                if held > claim {
                    ticker.cancel();
                    return None;
                }
                held_ticker.cancel();
                o.insert((claim, ticker.clone()));
                (held < claim).then_some(held)
            }
        }
    }

    /// The claim this process is beating for `task_id`, if any.
    pub(super) fn held(&self, task_id: &str) -> Option<ClaimEpoch> {
        self.0.get(task_id).map(|entry| entry.0)
    }

    /// Stop the ticker of exactly this claim, if it is still the registered one.
    /// An unfenced ([`NO_CLAIM`]) caller owns whatever ticker is registered.
    pub(super) fn retire(&self, task_id: &str, claim: ClaimEpoch, why: &'static str) {
        let removed = self
            .0
            .remove_if(task_id, |_, (held, _)| claim == NO_CLAIM || *held == claim);
        if let Some((_, (_, ticker))) = removed {
            tracing::debug!(task_id, claim, why, "retiring heartbeat ticker");
            ticker.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_claim_is_unfenced_and_a_claim_is_fenced() {
        assert_eq!(fence(NO_CLAIM), None);
        assert_eq!(fence(5), Some(5));
    }

    #[test]
    fn cancelling_one_claim_leaves_the_next_untouched() {
        let tokens = ClaimTokens::default();
        let old = tokens.token("t", 1);
        tokens.cancel_claim("t", 1);
        assert!(old.is_cancelled());
        assert!(!tokens.token("t", 2).is_cancelled());
    }

    #[test]
    fn a_stop_fires_every_claim_and_taints_later_ones() {
        let tokens = ClaimTokens::default();
        let running = tokens.token("t", 1);
        tokens.stop("t");
        assert!(running.is_cancelled());
        assert!(tokens.token("t", NO_CLAIM).is_cancelled());
        assert!(
            tokens.token("t", 2).is_cancelled(),
            "a re-claim honours the Stop"
        );
    }

    #[test]
    fn arming_a_task_starts_from_a_clean_slot() {
        let tokens = ClaimTokens::default();
        tokens.stop("t");
        tokens.arm("t");
        assert!(!tokens.token("t", 3).is_cancelled());
    }

    #[test]
    fn a_subtree_stop_skips_siblings_that_share_a_prefix() {
        let tokens = ClaimTokens::default();
        let child = tokens.token("r.1", 1);
        let sibling = tokens.token("r0.1", 1);
        tokens.stop_subtree("r");
        assert!(child.is_cancelled());
        assert!(!sibling.is_cancelled());
    }

    #[test]
    fn a_released_claim_gets_a_fresh_token() {
        let tokens = ClaimTokens::default();
        let first = tokens.token("t", 1);
        first.cancel();
        tokens.release("t", 1);
        assert!(!tokens.token("t", 1).is_cancelled());
    }

    #[test]
    fn registering_a_newer_claim_reports_the_older_one() {
        let beats = Heartbeats::default();
        let (a, b) = (CancellationToken::new(), CancellationToken::new());
        assert_eq!(beats.register("t", 1, &a), None);
        assert_eq!(beats.register("t", 2, &b), Some(1));
        assert!(a.is_cancelled() && !b.is_cancelled());
    }

    #[test]
    fn an_unfenced_caller_owns_whatever_ticker_is_registered() {
        let beats = Heartbeats::default();
        let adopted = CancellationToken::new();
        beats.register("t", 4, &adopted);
        let virtual_worker = CancellationToken::new();
        assert_eq!(beats.register("t", NO_CLAIM, &virtual_worker), None);
        assert!(adopted.is_cancelled() && !virtual_worker.is_cancelled());
        beats.retire("t", NO_CLAIM, "done");
        assert!(virtual_worker.is_cancelled());
    }

    #[test]
    fn retiring_a_stale_claim_does_not_touch_the_current_ticker() {
        let beats = Heartbeats::default();
        let current = CancellationToken::new();
        beats.register("t", 2, &current);
        beats.retire("t", 1, "stale");
        assert!(!current.is_cancelled());
        beats.retire("t", 2, "done");
        assert!(current.is_cancelled());
    }
}
