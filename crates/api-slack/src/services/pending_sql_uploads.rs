//! In-memory cache for SQL artifacts awaiting a "View N queries" click.
//!
//! When `run_for_slack` posts the prose message with a defer-upload button,
//! the captured SQL artifacts are stashed here keyed by a synthetic upload
//! id (UUID) which is also embedded in the button's `value`. The
//! interactivity handler then `take`s the entries and runs the actual
//! `files.uploadV2` calls.
//!
//! ## Trade-offs
//!
//! - **Restart safety:** entries are lost on process restart. A user who
//!   clicks the button after a deploy gets a "session expired — view in
//!   Oxygen →" ephemeral fallback.
//! - **Single-instance:** the cache is process-local. If Slack ever scales
//!   to multiple Oxy pods, replace with a Postgres-backed table or the
//!   `slack_threads` row's metadata.
//! - **TTL:** entries older than `ENTRY_TTL` are evicted opportunistically
//!   on each `insert`. No background sweeper — cheap and good enough at
//!   the volumes we serve.
//!
//! See `webhooks/handlers/view_sql_artifacts.rs` for the consumer.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::render::CapturedSqlArtifact;

/// Stale entries beyond this age are evicted on the next `insert`. One hour
/// is comfortably longer than a user would reasonably leave a Slack thread
/// open before clicking the button, and short enough that abandoned entries
/// don't accumulate indefinitely.
const ENTRY_TTL: Duration = Duration::from_secs(60 * 60);

struct Entry {
    artifacts: Vec<CapturedSqlArtifact>,
    inserted_at: Instant,
}

fn cache() -> &'static Mutex<HashMap<Uuid, Entry>> {
    static CACHE: OnceLock<Mutex<HashMap<Uuid, Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Insert a fresh upload-id → artifacts mapping. Generates and returns the
/// id; the caller embeds it in the Slack button's `value`. Also evicts any
/// entries older than [`ENTRY_TTL`] in the same critical section so the
/// cache stays bounded.
pub async fn insert(artifacts: Vec<CapturedSqlArtifact>) -> Uuid {
    let upload_id = Uuid::new_v4();
    let mut guard = cache().lock().await;
    let now = Instant::now();
    guard.retain(|_, entry| now.duration_since(entry.inserted_at) < ENTRY_TTL);
    guard.insert(
        upload_id,
        Entry {
            artifacts,
            inserted_at: now,
        },
    );
    upload_id
}

/// Atomically remove and return the entry for `upload_id`. Returns `None`
/// if the entry doesn't exist (already consumed, evicted by TTL, or lost
/// to a process restart). Callers should treat `None` as "session expired"
/// and surface that to the user.
pub async fn take(upload_id: Uuid) -> Option<Vec<CapturedSqlArtifact>> {
    cache().lock().await.remove(&upload_id).map(|e| e.artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(title: &str) -> CapturedSqlArtifact {
        CapturedSqlArtifact {
            title: title.to_string(),
            sql: format!("SELECT '{title}'"),
            database: "duckdb".to_string(),
            is_verified: false,
        }
    }

    #[tokio::test]
    async fn round_trips_inserted_entries() {
        let id = insert(vec![fixture("a"), fixture("b")]).await;
        let taken = take(id).await.expect("entry present");
        let titles: Vec<&str> = taken.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn take_consumes_so_second_call_misses() {
        let id = insert(vec![fixture("once")]).await;
        assert!(take(id).await.is_some());
        assert!(take(id).await.is_none());
    }

    #[tokio::test]
    async fn unknown_id_returns_none() {
        assert!(take(Uuid::new_v4()).await.is_none());
    }
}
