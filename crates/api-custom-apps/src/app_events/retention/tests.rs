use super::*;
use crate::app_events::deliveries::{AttemptOutcome, CRASHED};

/// The dead sync and the expiry close an attempt left open with `begin`'s
/// own words for a crash: its outcome and its error.
#[test]
fn the_dead_sync_and_the_expiry_close_open_attempts_as_begin_does() {
    for sql in [DEAD_SQL, EXPIRE_SQL] {
        assert!(
            sql.contains("UPDATE app_event_delivery_attempts a"),
            "{sql}"
        );
        assert!(
            sql.contains("a.finished_at IS NULL"),
            "only an open attempt: {sql}"
        );
        assert!(
            sql.contains(&format!("outcome = '{}'", AttemptOutcome::Crash.as_str())),
            "{sql}"
        );
        assert!(sql.contains(&format!("error = '{CRASHED}'")), "{sql}");
    }
}
