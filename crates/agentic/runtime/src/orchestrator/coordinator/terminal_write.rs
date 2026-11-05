//! Terminal writes on the timeout and cancel paths: retried, then reported.
//!
//! `check_suspend_timeouts` and `handle_cancelled` used to end these writes in
//! `.ok()`. By the time they run, the coordinator has already marked the node
//! terminal in memory and is about to tell the parent (or end the run), so a
//! dropped write left the row `running` / `delegating` with nobody driving it,
//! and nothing said so.
//!
//! **Retry, not propagate.** Both callers are event handlers of the
//! coordinator's loop; there is no caller above them that could do anything
//! with an `Err` except log it, and failing the loop would strand every other
//! task the coordinator holds. The writes are idempotent (a status UPDATE, or
//! that UPDATE plus an upserted outcome row, in one transaction), so repeating
//! one is safe. Each failed attempt is a `warn!` naming the task and run; one
//! that fails on every attempt is an `error!` with `rule = "terminal-write-lost"`,
//! and the run is left to the backstops that find non-terminal runs nothing
//! drives (`worker-fleet.md`).

use std::future::Future;
use std::time::Duration;

use sea_orm::DbErr;

/// Pause before each retry. Attempts = `RETRY_BACKOFF.len() + 1`. Short: the
/// coordinator's loop waits on it, and a database down for longer than this is
/// a case for the backstops, not for this loop.
const RETRY_BACKOFF: [Duration; 2] = [Duration::from_millis(100), Duration::from_millis(400)];

/// Which terminal write, for the logs.
#[derive(Clone, Copy)]
pub(super) struct TerminalWriteTarget<'a> {
    pub what: &'static str,
    pub task_id: &'a str,
    pub run_id: &'a str,
}

/// Run `write` until it succeeds or the attempts run out. `true` if it landed.
pub(super) async fn write_terminal<F, Fut>(target: TerminalWriteTarget<'_>, write: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), DbErr>>,
{
    write_with_backoff(target, &RETRY_BACKOFF, write).await
}

async fn write_with_backoff<F, Fut>(
    target: TerminalWriteTarget<'_>,
    backoff: &[Duration],
    mut write: F,
) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), DbErr>>,
{
    let TerminalWriteTarget {
        what,
        task_id,
        run_id,
    } = target;
    for attempt in 1..=backoff.len() + 1 {
        let error = match write().await {
            Ok(()) => return true,
            Err(e) => e,
        };
        let Some(pause) = backoff.get(attempt - 1) else {
            tracing::error!(
                target: "coordinator",
                rule = "terminal-write-lost",
                what,
                task_id,
                run_id,
                attempt,
                error = %error,
                "terminal write failed on every attempt; the run row is left non-terminal"
            );
            return false;
        };
        tracing::warn!(
            target: "coordinator",
            what,
            task_id,
            run_id,
            attempt,
            error = %error,
            "terminal write failed; retrying"
        );
        tokio::time::sleep(*pause).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const NO_PAUSE: [Duration; 2] = [Duration::ZERO; 2];

    fn target() -> TerminalWriteTarget<'static> {
        TerminalWriteTarget {
            what: "test",
            task_id: "t",
            run_id: "r",
        }
    }

    /// A write that fails its first `failures` calls.
    async fn run(failures: usize) -> (bool, usize) {
        let calls = AtomicUsize::new(0);
        let landed = write_with_backoff(target(), &NO_PAUSE, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < failures {
                    Err(DbErr::Custom("injected".into()))
                } else {
                    Ok(())
                }
            }
        })
        .await;
        (landed, calls.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn a_write_that_lands_is_made_once() {
        assert_eq!(run(0).await, (true, 1));
    }

    #[tokio::test]
    async fn a_write_that_fails_is_retried_until_it_lands() {
        assert_eq!(run(2).await, (true, 3));
    }

    #[tokio::test]
    async fn a_write_that_never_lands_stops_after_its_attempts_and_says_so() {
        assert_eq!(run(usize::MAX).await, (false, NO_PAUSE.len() + 1));
    }
}
