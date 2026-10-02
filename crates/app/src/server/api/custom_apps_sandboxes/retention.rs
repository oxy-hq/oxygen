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
