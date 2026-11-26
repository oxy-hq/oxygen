//! How a step's failures are classified, and when a chunk is tried again.

use tokio_postgres::error::SqlState;

use super::{CHUNK_ATTEMPTS, Halt, Step, halt_for, read_halt_for, retry, settled};
use crate::oltp_changes::relay::pass::Stop;

/// A serialization failure (40001) is a conflict only for a chunk's read,
/// and so is a deadlock (40P01) a lock timeout; any statement's lock timeout
/// (55P03) is one; nothing else is classified.
#[test]
fn only_a_chunks_read_can_conflict() {
    let conflict = Some(&SqlState::T_R_SERIALIZATION_FAILURE);
    let lock = Some(&SqlState::LOCK_NOT_AVAILABLE);
    let deadlock = Some(&SqlState::T_R_DEADLOCK_DETECTED);
    assert_eq!(read_halt_for(conflict), Some(Halt::Conflict));
    assert_eq!(read_halt_for(lock), Some(Halt::LockTimeout));
    assert_eq!(read_halt_for(deadlock), Some(Halt::LockTimeout));
    assert_eq!(halt_for(deadlock), None, "only the chunk's read");
    assert_eq!(
        halt_for(conflict),
        None,
        "any other statement's 40001 is an error"
    );
    assert_eq!(halt_for(lock), Some(Halt::LockTimeout));
    for neither in [
        Some(&SqlState::QUERY_CANCELED),
        Some(&SqlState::UNIQUE_VIOLATION),
        None,
    ] {
        assert_eq!(halt_for(neither), None, "{neither:?}");
        assert_eq!(read_halt_for(neither), None, "{neither:?}");
    }
}

/// A conflict is retried while attempts remain, and on the last attempt is
/// a conflict still, which backs the baseline off as its own outcome, never
/// as a lock timeout or an error without the table. A lock timeout is never
/// retried, and neither is any other error, which fails the writer naming
/// the table.
#[test]
fn a_conflicting_chunk_is_retried_then_backs_off_as_a_conflict() {
    for attempt in 1..CHUNK_ATTEMPTS {
        assert_eq!(retry(Halt::Conflict, attempt), Ok(()), "attempt {attempt}");
    }
    let spent = retry(Halt::Conflict, CHUNK_ATTEMPTS);
    assert_eq!(spent, Err(Halt::Conflict));
    assert_eq!(retry(Halt::LockTimeout, 1), Err(Halt::LockTimeout));
    let error = Halt::Error("write the chunk: boom".to_owned());
    assert_eq!(retry(error.clone(), 1), Err(error));

    assert!(matches!(
        settled("leases", Err(Halt::Conflict)),
        Ok(Step::Conflicted)
    ));
    assert!(matches!(
        settled("leases", Err(Halt::LockTimeout)),
        Ok(Step::LockTimeout)
    ));
    let failed = settled("leases", Err(Halt::Error("boom".to_owned())));
    assert!(
        matches!(&failed, Err(Stop::Failed(e)) if e == "baseline of leases: boom"),
        "{failed:?}"
    );
}
