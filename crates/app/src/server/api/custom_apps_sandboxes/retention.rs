//! Which of an app's builds a publish may prune, now that sandboxes publish
//! too (`custom_apps_publish::gc_builds`).
//!
//! An app keeps its newest builds so that production can be rolled back to
//! one. Were sandbox publishes counted in that one window, ten of them — an
//! afternoon of an agent iterating — would push out the build production
//! served last week, and the rollback would have no target. So builds that
//! **only a sandbox ever served** are kept in a window of their own, and
//! every other build in the window it always had.
//!
//! A build is *sandbox-only* when it has pointer events
//! (`app_environment_events`) and every one names a `dev-…` environment. A
//! build with no events at all — published before events were recorded — is
//! not, and stays in the app's own window.
//!
//! **Drafts do not spend a rollback slot either.** The window that was left
//! held two kinds of build: the ones production ran, and drafts that only
//! staging ever served. Ten drafts after a promote pushed out every build
//! production had run but was not pointing at — the rollback targets. So a
//! build **production has served** ([`production_served_builds`]: a pointer
//! event naming `production`, whatever moved it) is kept in a third window,
//! and no number of drafts or sandbox publishes can prune one. [`beyond`] is
//! the whole rule:
//!
//! | Window | A build is in it when | Kept |
//! | --- | --- | --- |
//! | production | some pointer event names `production` | newest `keep` |
//! | sandbox | it has events and every one names a `dev-…` environment | newest `keep` |
//! | drafts | anything else: staging only, or no event at all | newest `keep` |
//!
//! Nothing that was kept before is pruned now: a build stays in whichever of
//! the two old windows it was in, or moves to the new one, and each window
//! keeps as many as the one it came from. A build production served before
//! events were recorded has no event and stays a draft, as it was.

use std::collections::HashSet;

use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

/// The builds of `app_id` that only sandboxes ever pointed at. One grouped
/// read of the app's events.
pub(crate) async fn sandbox_only_builds<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
) -> Result<HashSet<Uuid>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT build_id FROM app_environment_events \
              WHERE app_id = $1 AND build_id IS NOT NULL \
              GROUP BY build_id \
             HAVING bool_and(environment LIKE 'dev-%')",
            [app_id.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|row| row.try_get::<Uuid>("", "build_id"))
        .collect()
}

/// The builds beyond retention, given every build of the app **newest
/// first**: all but the newest `keep` sandbox-only builds, and all but the
/// newest `keep` of the rest. What is returned may still be protected — a
/// build an environment points at is never deleted, whatever its age.
pub(crate) fn beyond_retention(
    newest_first: &[Uuid],
    sandbox_only: &HashSet<Uuid>,
    keep: usize,
) -> Vec<Uuid> {
    let (mut sandbox_seen, mut other_seen) = (0usize, 0usize);
    let mut beyond = Vec::new();
    for build in newest_first {
        let seen = if sandbox_only.contains(build) {
            &mut sandbox_seen
        } else {
            &mut other_seen
        };
        *seen += 1;
        if *seen > keep {
            beyond.push(*build);
        }
    }
    beyond
}

/// The builds of `app_id` production has served: the target of a promote, a
/// rollback, or the backfill of a pointer that predates events. One read of
/// the app's events.
pub(crate) async fn production_served_builds<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
) -> Result<HashSet<Uuid>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT build_id FROM app_environment_events \
              WHERE app_id = $1 AND build_id IS NOT NULL AND environment = 'production'",
            [app_id.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|row| row.try_get::<Uuid>("", "build_id"))
        .collect()
}

/// [`beyond_retention`] with the builds production served kept in a window of
/// their own: all but the newest `keep` of those, and what `beyond_retention`
/// answers for the rest. Newest first, as it was given.
pub(crate) fn beyond_windows(
    newest_first: &[Uuid],
    sandbox_only: &HashSet<Uuid>,
    production_served: &HashSet<Uuid>,
    keep: usize,
) -> Vec<Uuid> {
    let (served, rest): (Vec<Uuid>, Vec<Uuid>) = newest_first
        .iter()
        .copied()
        .partition(|build| production_served.contains(build));
    let mut beyond: HashSet<Uuid> = served.into_iter().skip(keep).collect();
    beyond.extend(beyond_retention(&rest, sandbox_only, keep));
    newest_first
        .iter()
        .copied()
        .filter(|build| beyond.contains(build))
        .collect()
}

/// The builds of `app_id` beyond retention, given every build of the app
/// **newest first**: the three windows of the module docs, read from the
/// app's pointer events. `Err` when the events cannot be read — the caller
/// skips the prune rather than guess which window a build is in.
pub(crate) async fn beyond<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    newest_first: &[Uuid],
    keep: usize,
) -> Result<Vec<Uuid>, DbErr> {
    let sandbox_only = sandbox_only_builds(db, app_id).await?;
    let production_served = production_served_builds(db, app_id).await?;
    Ok(beyond_windows(
        newest_first,
        &sandbox_only,
        &production_served,
        keep,
    ))
}

#[cfg(test)]
#[path = "retention_windows_tests.rs"]
mod windows_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// Two channel builds and ten sandbox builds after them: nothing is
    /// beyond retention with a window of ten each, where one shared window
    /// would have dropped both channel builds.
    #[test]
    fn sandbox_builds_do_not_push_channel_builds_out() {
        let channel = [id(1), id(2)];
        let sandbox: Vec<Uuid> = (10..20).map(id).collect();
        let newest_first: Vec<Uuid> = sandbox
            .iter()
            .rev()
            .chain(channel.iter().rev())
            .copied()
            .collect();
        let sandbox_only: HashSet<Uuid> = sandbox.iter().copied().collect();
        assert_eq!(newest_first.len(), 12);
        assert!(beyond_retention(&newest_first, &sandbox_only, 10).is_empty());
        // The shared window this replaces: the two oldest — the channel builds.
        assert_eq!(
            beyond_retention(&newest_first, &HashSet::new(), 10),
            vec![id(2), id(1)]
        );
    }

    /// Each window prunes its own oldest: the eleventh sandbox build goes, and
    /// the channel builds are untouched by it.
    #[test]
    fn each_window_keeps_its_newest_and_drops_its_own_oldest() {
        let sandbox: Vec<Uuid> = (10..21).map(id).collect(); // eleven
        let channel: Vec<Uuid> = (1..4).map(id).collect(); // three
        // Interleaved by age, newest first: sandbox 20..10, then channel 3..1.
        let newest_first: Vec<Uuid> = sandbox
            .iter()
            .rev()
            .chain(channel.iter().rev())
            .copied()
            .collect();
        let sandbox_only: HashSet<Uuid> = sandbox.iter().copied().collect();
        assert_eq!(
            beyond_retention(&newest_first, &sandbox_only, 10),
            vec![id(10)]
        );
        assert_eq!(
            beyond_retention(&newest_first, &sandbox_only, 2),
            // sandbox 18..10 (nine), then channel 1.
            (10..19).rev().map(id).chain([id(1)]).collect::<Vec<_>>()
        );
        assert!(beyond_retention(&[], &sandbox_only, 10).is_empty());
    }
}
