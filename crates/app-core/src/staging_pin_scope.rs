//! The staging pin's task-local scope: run a future under a pinned semantic
//! revision, and read the pin back from code that is not handed it.
//!
//! Lives here (below `oxy-app`) because the compile-boundary readers
//! ([`crate::preagg_context`]) must honour the pin; the publish-time and
//! pin logic is in `oxy_custom_apps::staging_pin`. Callers in `oxy-app` and
//! `oxy-api-custom-apps` import the functions below from here.

use uuid::Uuid;

tokio::task_local! {
    /// The staging revision this task is pinned to — set only by
    /// [`with_staging_pin`], never by the ordinary promoted-revision pin. Lets
    /// code that is not handed the pin still hold the staging rules: the rollup
    /// short-circuit is skipped (`preagg_context`) and result caches are
    /// partitioned ([`cache_partition`]).
    static STAGING_PIN: Uuid;
}

/// Run `fut` with every compile-boundary read pinned to `pin` — or unchanged
/// when `pin` is `None`.
///
/// Use this rather than calling `compiled_reader::with_pinned_revision`
/// directly: there, `Some(None)` scoped means "this request reads the
/// FILESYSTEM everywhere", which on a replica is a 503. "No pin" has to mean
/// "don't scope at all", and this is the one place that says so.
///
/// Two callers: a custom-app draft build's data-plane reads, and a staff
/// workspace preview (`server::previews`), which pins a staging revision for a
/// whole `/api/{workspace_id}/…` request. Both get the same rules inside the
/// scope — see [`current_staging_pin`].
pub async fn with_staging_pin<F, T>(pin: Option<Uuid>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match pin {
        Some(revision_id) => {
            STAGING_PIN
                .scope(
                    revision_id,
                    crate::compiled_reader::with_pinned_revision(Some(revision_id), fut),
                )
                .await
        }
        None => fut.await,
    }
}

/// The staging revision the current task is pinned to, if any. `None` outside
/// [`with_staging_pin`] — including in a task spawned from inside it, which is
/// why background work can never see one.
pub fn current_staging_pin() -> Option<Uuid> {
    STAGING_PIN.try_with(|pin| *pin).ok()
}

/// The result-cache partition for a request carrying `staging_pin`: `""` on
/// live, `staging-pin:<uuid>` on a pinned staging request, so a branch's answer
/// never serves live users and live's never answers the staging preview.
///
/// The one statement of the rule. Every result-cache key goes through it —
/// [`cache_partition`] for code that reads the pin from the task, and
/// `oxy_workspace_runtime::result_cache::scope_for` (a re-export) for the
/// data-plane routes and the semantic boundary, which hold the gate's
/// `staging_pin`. A second copy that drifted would be a live/staging data
/// leak, not a build error.
pub fn scope_for(staging_pin: Option<Uuid>) -> String {
    staging_pin
        .map(|pin| format!("staging-pin:{pin}"))
        .unwrap_or_default()
}

/// The result-cache partition for the current task: [`scope_for`] of the pin
/// this task runs under.
pub fn cache_partition() -> String {
    scope_for(current_staging_pin())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The staging rules hold for anything running inside the pin — the
    /// rollup short-circuit declines and result caches get their own partition —
    /// and for nothing outside it, including a task spawned from inside it.
    #[tokio::test]
    async fn inside_a_staging_pin_rollups_are_skipped_and_caches_partitioned() {
        let pin = Uuid::new_v4();
        /// Would this task get the rollup short-circuit, given a warm cache?
        fn rollups() -> bool {
            crate::preagg_context::preagg_context(
                Uuid::nil(),
                Some(std::sync::Arc::new(std::sync::RwLock::new(
                    agentic_semantic::refresh_key_cache::RefreshKeyCache::new(),
                ))),
                None,
                crate::preagg_context::RollupFreshness::ServeStale,
            )
            .is_some()
        }

        assert!(rollups(), "unpinned, the rollup tier is available");
        assert_eq!(current_staging_pin(), None);
        assert_eq!(cache_partition(), "");

        let (inside, spawned) = with_staging_pin(Some(pin), async move {
            let inside = (current_staging_pin(), cache_partition(), rollups());
            let spawned = tokio::spawn(async move { (current_staging_pin(), rollups()) })
                .await
                .unwrap();
            (inside, spawned)
        })
        .await;
        assert_eq!(
            inside,
            (Some(pin), format!("staging-pin:{pin}"), false),
            "pinned: the pin is visible, caches partition on it, rollups decline"
        );
        assert_eq!(
            spawned,
            (None, true),
            "a spawned task is background work: it never inherits the pin"
        );

        let untouched = with_staging_pin(None, async { (current_staging_pin(), rollups()) }).await;
        assert_eq!(untouched, (None, true), "no pin means no scope at all");
    }

    /// The result-cache partition rule, pinned: live is `""` and a pinned
    /// staging request is `staging-pin:<uuid>`. Every result-cache key — the
    /// data-plane routes, the semantic boundary and [`cache_partition`] — is
    /// built from this one function, so a change here is a change to all of
    /// them at once.
    #[test]
    fn scope_for_is_empty_on_live_and_names_the_pin_on_staging() {
        let pin = Uuid::new_v4();
        assert_eq!(scope_for(None), "");
        assert_eq!(scope_for(Some(pin)), format!("staging-pin:{pin}"));
        assert_ne!(scope_for(Some(pin)), scope_for(None));
        assert_ne!(scope_for(Some(pin)), scope_for(Some(Uuid::new_v4())));
    }
}
