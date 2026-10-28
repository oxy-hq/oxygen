//! A per-process throttle in front of the lazy self-heal.
//!
//! `enqueue_lazy_compile` runs on every serve request that misses the compile
//! boundary, and one page sends many at once (a custom app's dashboard fires
//! about twenty queries). Before the advisory lock and the dedupe in
//! `enqueue_compile_deduped_as`, it decides what to queue: `heal` reads the
//! workspace row and its promoted revision, and `request_check` writes and
//! reads a check request. Those answer the same a few seconds later, and the
//! dedupe behind them drops all but the first enqueue anyway, so a workspace
//! this process asked about within [`WINDOW`] is passed over before any read.
//!
//! Per process, not shared: each replica still asks once per window, and the
//! advisory lock and the dedupe already make those agree.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use uuid::Uuid;

/// How long a workspace's self-heal decision stands in this process. Far
/// under the self-heal's own failure backoff, so it only folds a burst.
const WINDOW: Duration = Duration::from_secs(10);

static LAST_ASKED: LazyLock<DashMap<Uuid, Instant>> = LazyLock::new(DashMap::new);

/// `true`, and recorded, when this process has not asked for a self-heal of
/// `workspace_id` within [`WINDOW`]; `false` for every other call in it.
pub(crate) fn first_in_window(workspace_id: Uuid) -> bool {
    first_in_window_at(&LAST_ASKED, workspace_id, Instant::now())
}

fn first_in_window_at(asked: &DashMap<Uuid, Instant>, workspace_id: Uuid, now: Instant) -> bool {
    match asked.entry(workspace_id) {
        Entry::Occupied(mut last) => {
            if now.saturating_duration_since(*last.get()) < WINDOW {
                return false;
            }
            last.insert(now);
            true
        }
        Entry::Vacant(slot) => {
            slot.insert(now);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_for_one_workspace_asks_once() {
        let asked = DashMap::new();
        let ws = Uuid::new_v4();
        let t0 = Instant::now();
        assert!(first_in_window_at(&asked, ws, t0));
        assert!(!first_in_window_at(
            &asked,
            ws,
            t0 + Duration::from_millis(5)
        ));
        assert!(!first_in_window_at(
            &asked,
            ws,
            t0 + WINDOW - Duration::from_millis(1)
        ));
    }

    #[test]
    fn it_asks_again_once_the_window_has_passed() {
        let asked = DashMap::new();
        let ws = Uuid::new_v4();
        let t0 = Instant::now();
        assert!(first_in_window_at(&asked, ws, t0));
        assert!(first_in_window_at(&asked, ws, t0 + WINDOW));
        assert!(!first_in_window_at(
            &asked,
            ws,
            t0 + WINDOW + Duration::from_secs(1)
        ));
    }

    #[test]
    fn workspaces_are_throttled_apart() {
        let asked = DashMap::new();
        let t0 = Instant::now();
        assert!(first_in_window_at(&asked, Uuid::new_v4(), t0));
        assert!(first_in_window_at(&asked, Uuid::new_v4(), t0));
    }
}
