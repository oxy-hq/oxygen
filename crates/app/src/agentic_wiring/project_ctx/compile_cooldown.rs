//! Whether a request may ask for a self-heal compile *right now*.
//!
//! `OxyProjectContext::request_compile` hands a `NotInRevision` to the lazy
//! self-heal (`enqueue_lazy_compile`), which dedupes a compile already in
//! flight and backs off after failures — and nothing else. A self-heal compile
//! carries no `git_sha`, so every one that is taken mints and promotes a fresh
//! `local-<uuid>` revision. For a ref that is simply gone from `main` the
//! compile *succeeds* and still does not serve it, and the next request asks
//! again. Requests arrive at request rate, not person rate — a React Query
//! fetch retries a 503 three times, a stale tab refetches on focus, any client
//! honouring `Retry-After` comes back on schedule — so that was one promoted
//! revision per retry, for as long as the tab stayed open.
//!
//! The rule: a request may ask only when the latest `main` compile did not just
//! succeed. The window is the self-heal's own flat retry interval,
//! [`LAZY_COMPILE_BACKOFF_SECS`] — the shortest interval at which the self-heal
//! is willing to recompile the same working tree after a *failure*. A compile
//! that succeeded and still does not serve the ref is the same tree from the
//! requester's side (a tree change arrives through the post-pull trigger with a
//! `git_sha`, never through here), so it earns the same interval, not a shorter
//! one. Within it the host answers `false`, and the `503` body says the ref is
//! not served without claiming a compile was requested.
//!
//! The decision is a pure function over the row the lazy-compile backoff
//! already reads (`revisions`, `kind = 'main'`, newest `started_at` first), so
//! it is tested without a database; the read beside it is the same shape as
//! that backoff's.

use chrono::{DateTime, FixedOffset};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use crate::server::api::middlewares::workspace_context::LAZY_COMPILE_BACKOFF_SECS;

/// What the decision needs of the latest `main` revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LatestMainRevision {
    /// `compiling` | `ready` | `failed` | `superseded` — plain text in the row.
    pub status: String,
    pub finished_at: Option<DateTime<FixedOffset>>,
}

/// A compile that ran to completion. `ready` is the status the writer gives a
/// finished compile; `superseded` is one that finished and lost the promotion
/// race to a sibling — the tree was compiled either way, and that sibling is
/// what the boundary now serves. `failed` and `compiling` are not completions:
/// the first is the failure backoff's business, the second the in-flight
/// dedupe's.
fn compile_completed(status: &str) -> bool {
    matches!(status, "ready" | "superseded")
}

/// `true` when a compile of this workspace's `main` completed within the last
/// [`LAZY_COMPILE_BACKOFF_SECS`] of `now`, so a request must not ask for
/// another. `None` — no `main` revision at all — never cools down: a workspace
/// that has never compiled is exactly what the self-heal exists for.
pub(super) fn within_success_cooldown(
    latest: Option<&LatestMainRevision>,
    now: DateTime<FixedOffset>,
) -> bool {
    let Some(latest) = latest else {
        return false;
    };
    if !compile_completed(&latest.status) {
        return false;
    }
    let Some(finished_at) = latest.finished_at else {
        return false;
    };
    now - finished_at < chrono::Duration::seconds(LAZY_COMPILE_BACKOFF_SECS)
}

/// The latest `main` revision — the row the lazy-compile backoff reads, one
/// instead of eight. `None` on a read error too, said out loud: the enqueue
/// then proceeds, which is the behaviour before this guard existed.
pub(super) async fn latest_main_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Option<LatestMainRevision> {
    let row: Result<Option<(String, Option<DateTime<FixedOffset>>)>, _> =
        entity::revisions::Entity::find()
            .select_only()
            .column(entity::revisions::Column::Status)
            .column(entity::revisions::Column::FinishedAt)
            .filter(entity::revisions::Column::WorkspaceId.eq(workspace_id))
            .filter(entity::revisions::Column::Kind.eq("main"))
            .order_by_desc(entity::revisions::Column::StartedAt)
            .limit(1)
            .into_tuple()
            .one(db)
            .await;
    match row {
        Ok(row) => row.map(|(status, finished_at)| LatestMainRevision {
            status,
            finished_at,
        }),
        Err(e) => {
            tracing::warn!(?e, %workspace_id, "compile cooldown: reading the latest revision failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn at(secs_ago: i64) -> DateTime<FixedOffset> {
        (now() - chrono::Duration::seconds(secs_ago)).fixed_offset()
    }

    fn now() -> DateTime<FixedOffset> {
        Utc.with_ymd_and_hms(2026, 10, 3, 12, 0, 0)
            .unwrap()
            .fixed_offset()
    }

    fn revision(status: &str, finished_at: Option<DateTime<FixedOffset>>) -> LatestMainRevision {
        LatestMainRevision {
            status: status.into(),
            finished_at,
        }
    }

    /// THE case: a compile just succeeded and the ref is still not served. The
    /// next request — a React Query retry, a stale tab — must not buy another
    /// revision.
    #[test]
    fn a_compile_that_just_succeeded_holds_the_next_request_back() {
        let latest = revision("ready", Some(at(30)));
        assert!(within_success_cooldown(Some(&latest), now()));
    }

    /// The window is the self-heal's flat failure interval, exactly: one second
    /// inside holds, one second past lets go. Pinned to the constant rather
    /// than to 300 so the two cannot drift apart silently.
    #[test]
    fn the_window_is_the_lazy_compile_backoff() {
        let inside = revision("ready", Some(at(LAZY_COMPILE_BACKOFF_SECS - 1)));
        let past = revision("ready", Some(at(LAZY_COMPILE_BACKOFF_SECS + 1)));
        assert!(within_success_cooldown(Some(&inside), now()));
        assert!(!within_success_cooldown(Some(&past), now()));
    }

    /// A loser of the promotion race finished compiling the same tree; its
    /// winner is what the boundary serves. Another compile adds nothing.
    #[test]
    fn a_superseded_compile_counts_as_completed() {
        let latest = revision("superseded", Some(at(10)));
        assert!(within_success_cooldown(Some(&latest), now()));
    }

    /// A failure is the failure backoff's to pace, and an in-flight compile is
    /// the dedupe's: neither holds a request back here, or a workspace whose
    /// last compile broke would stop being self-healed by requests at all.
    #[test]
    fn a_failed_or_running_compile_does_not_cool_down() {
        let failed = revision("failed", Some(at(10)));
        let compiling = revision("compiling", None);
        assert!(!within_success_cooldown(Some(&failed), now()));
        assert!(!within_success_cooldown(Some(&compiling), now()));
    }

    /// A `ready` row with no `finished_at` cannot be placed in time; treat it
    /// as not recent rather than guess.
    #[test]
    fn a_completion_with_no_finish_time_does_not_cool_down() {
        let latest = revision("ready", None);
        assert!(!within_success_cooldown(Some(&latest), now()));
    }

    /// Never compiled is what the self-heal is for.
    #[test]
    fn no_main_revision_never_cools_down() {
        assert!(!within_success_cooldown(None, now()));
    }
}
