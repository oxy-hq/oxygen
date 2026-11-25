//! The host nudges the change stream relay before a production `ctx.oltp`
//! transaction commits (OLTP change stream design §5.3).
//!
//! The relay drains `oxy_changes.log` on demand rather than on a poll, so a
//! transaction of an app that declares tables records a "nudge" for its
//! (org, environment) in the platform database first
//! (`relay::marks::nudge`). If the nudge cannot be recorded the caller drops
//! the transaction, which rolls it back: a change must never commit without
//! the relay having been asked to look.
//!
//! Debounced per (org, environment) for [`NUDGE_DEBOUNCE`], process-wide: the
//! first nudge already asks for everything logged so far, and the sweep
//! catches the rest.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::signs;
use super::*;
use crate::oltp_changes::relay::NUDGE_DEBOUNCE;
use crate::oltp_changes::relay::marks;

/// When each (org, environment) was last nudged by this process.
static LAST_NUDGE: LazyLock<Mutex<HashMap<(Uuid, String), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether a nudge is due: none was recorded, or the last one is at least
/// [`NUDGE_DEBOUNCE`] old.
fn due_to_nudge(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= NUDGE_DEBOUNCE)
}

/// Whether a commit that took `took` nudges again: one slower than
/// [`NUDGE_DEBOUNCE`] outlasted the nudge made before it.
fn renudges_after(took: Duration) -> bool {
    took > NUDGE_DEBOUNCE
}

fn last_nudge(key: &(Uuid, String)) -> Option<Instant> {
    LAST_NUDGE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(key)
        .copied()
}

fn remember_nudge(key: (Uuid, String)) {
    LAST_NUDGE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, Instant::now());
}

impl ProjectFunctionHost {
    /// Record the nudge for this run's (org, environment), just before a
    /// `ctx.oltp` transaction commits: a no-op unless the build declares
    /// tables, the run is production's and the org has the flag. An error is a
    /// platform error: the caller drops the transaction, which rolls it back.
    pub(in crate::custom_apps_functions::host) async fn nudge_oltp_changes(
        &self,
    ) -> Result<(), String> {
        if !signs(self.caps.oltp_changes, &self.policy)
            || !crate::oltp_changes::enabled_for(self.org_id)
        {
            return Ok(());
        }
        let key = (self.org_id, self.policy.environment().name().to_string());
        if !due_to_nudge(last_nudge(&key), Instant::now()) {
            return Ok(());
        }
        let (org_id, environment) = (key.0, key.1.clone());
        with_db_timeout("nudge", async {
            marks::nudge(&self.db, org_id, &environment)
                .await
                .map_err(|e| {
                    format!("could not record the change stream's nudge (platform-side): {e}")
                })
        })
        .await?;
        remember_nudge(key);
        Ok(())
    }

    /// After a `COMMIT` that took `took`: when slower than [`NUDGE_DEBOUNCE`],
    /// nudge again, bypassing the debounce, so the relay sees the commit that
    /// landed after the first nudge. Best effort: the data is committed, so a
    /// failure is logged, never raised to the app.
    pub(in crate::custom_apps_functions::host) async fn nudge_after_slow_commit(
        &self,
        took: Duration,
    ) {
        if !renudges_after(took)
            || !signs(self.caps.oltp_changes, &self.policy)
            || !crate::oltp_changes::enabled_for(self.org_id)
        {
            return;
        }
        let key = (self.org_id, self.policy.environment().name().to_string());
        let nudged = with_db_timeout("nudge", async {
            marks::nudge(&self.db, key.0, &key.1)
                .await
                .map_err(|e| e.to_string())
        })
        .await;
        match nudged {
            Ok(()) => remember_nudge(key),
            Err(e) => tracing::warn!(
                "could not re-nudge the change stream after a slow commit (platform-side): {e}"
            ),
        }
    }

    /// [`Self::nudge_oltp_changes`] for the commit of `ctx.oltp.tx` handle
    /// `id`, and only for an OLTP handle. A failed nudge forgets the handle,
    /// which the caller then drops. `true` for an OLTP handle.
    pub(in crate::custom_apps_functions::host) async fn nudge_before_tx_commit(
        &self,
        id: u64,
    ) -> Result<bool, String> {
        let plane = self.tx_writes.lock().await.get(&id).map(|a| a.plane);
        if plane != Some("oltp") {
            return Ok(false);
        }
        let nudged = self.nudge_oltp_changes().await;
        if nudged.is_err() {
            self.tx_writes.lock().await.remove(&id);
        }
        nudged.map(|()| true)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_nudge_is_debounced_for_five_seconds() {
        let now = Instant::now();
        assert!(due_to_nudge(None, now));
        assert!(!due_to_nudge(Some(now), now));
        assert!(!due_to_nudge(Some(now), now + Duration::from_millis(4_999)));
        assert!(due_to_nudge(Some(now), now + Duration::from_secs(5)));
        assert!(due_to_nudge(Some(now), now + Duration::from_secs(60)));
    }

    #[test]
    fn a_slow_commit_nudges_again_and_a_fast_one_does_not() {
        assert!(!renudges_after(Duration::from_millis(10)));
        assert!(!renudges_after(NUDGE_DEBOUNCE));
        assert!(renudges_after(NUDGE_DEBOUNCE + Duration::from_millis(1)));
    }
}
