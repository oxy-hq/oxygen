//! The clone's bound and heartbeat, on a paused clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::bounded;

const EVERY: Duration = Duration::from_secs(5 * 60);
const MAX: Duration = Duration::from_secs(2 * 60 * 60);

fn counter() -> (Arc<AtomicUsize>, impl FnMut() -> std::future::Ready<()>) {
    let beats = Arc::new(AtomicUsize::new(0));
    let seen = beats.clone();
    (beats, move || {
        seen.fetch_add(1, Ordering::SeqCst);
        std::future::ready(())
    })
}

/// A clone that never finishes (a half-open connection) is abandoned at the
/// bound, so its row can settle Failed instead of beating forever.
#[tokio::test(start_paused = true)]
async fn a_hung_clone_is_abandoned_at_the_bound() {
    let (beats, beat) = counter();
    let outcome = bounded(std::future::pending::<()>(), MAX, EVERY, beat).await;
    assert!(outcome.is_none());
    // It beat the whole time it ran, then stopped.
    let n = beats.load(Ordering::SeqCst);
    assert!((23..=24).contains(&n), "beats: {n}");
}

/// A clone that finishes returns its result, having beaten while it ran.
#[tokio::test(start_paused = true)]
async fn a_slow_clone_returns_its_result_and_beat_meanwhile() {
    let (beats, beat) = counter();
    let clone = async {
        tokio::time::sleep(Duration::from_secs(16 * 60)).await;
        "cloned"
    };
    assert_eq!(bounded(clone, MAX, EVERY, beat).await, Some("cloned"));
    assert_eq!(beats.load(Ordering::SeqCst), 3);
}
