//! The run event stream's pure rules, shared by every stream that serves a
//! run: the workspace stream (`agentic_http::routes::run::stream_events`) and
//! the custom-app stream (`oxy-api-custom-apps`' `agent_run_stream`).
//!
//! Two rules live here so they cannot fork between the two streams:
//!
//! - **Every stream ends on a terminal event** (`done` / `error` /
//!   `cancelled`). [`terminal_event_for_status`] derives it from the run row
//!   when nothing else persisted one.
//! - **The safety-net timer is sized against the connection pool.**
//!   [`poll_after`] and [`ROW_CHECK_EVERY_N_TIMED_WAKES`] set how often a
//!   parked stream reads the database.
//!
//! `std` and `serde_json` only: no database, no transport.

/// Synthesize a terminal SSE event from the run row's authoritative
/// `task_status`.
///
/// The SSE contract is that the stream closes only after a terminal event
/// (`done`/`error`/`cancelled`). That contract used to depend on *some*
/// code path remembering to persist a terminal event row — which it does
/// not when a run dies *before* the orchestrator loop starts (e.g. a
/// broken semantics file fails solver construction). The single source of
/// truth for run lifecycle is the `task_status` column, so when a stream
/// is about to close without having emitted a terminal event we derive
/// one from the run row instead of letting the client hang.
///
/// Returns `None` when the run is not (yet) terminal — the caller should
/// keep the stream open.
/// Pure mapping from a run row's terminal `task_status` to the SSE event
/// type + payload the frontend already knows how to render. Returns `None`
/// for non-terminal statuses (the stream should stay open). Split out from
/// the DB fetch so it can be unit-tested without a database.
pub fn terminal_event_for_status(
    task_status: Option<&str>,
    error_message: Option<&str>,
    answer: Option<&str>,
    run_id: &str,
) -> Option<(&'static str, serde_json::Value)> {
    match task_status? {
        "failed" | "timed_out" => Some((
            "error",
            serde_json::json!({
                "message": error_message
                    .unwrap_or("The run failed before it could report an error."),
                "trace_id": run_id,
            }),
        )),
        "cancelled" => Some(("cancelled", serde_json::json!({ "trace_id": run_id }))),
        "done" => Some((
            "done",
            serde_json::json!({
                "answer": answer.unwrap_or_default(),
                "trace_id": run_id,
            }),
        )),
        // running / awaiting_input / delegating — not terminal yet.
        _ => None,
    }
}

/// How long `agentic_http::routes::run::stream_events` parks before
/// re-reading `agentic_run_events` anyway, when the run's driver may be in
/// another process.
///
/// **Two wake sources, then the timer.** `RuntimeState.notifiers` is a
/// per-process `DashMap<run_id, Notify>` that only this process's driver
/// rings. A run driven elsewhere — an airway submit enqueued
/// `TaskScope::Global`, a queued chat claimed by another pod, a run whose
/// registering pod restarted — writes its rows to Postgres and rings a
/// notifier we cannot see. For those, every append to `agentic_run_events`
/// now also issues `NOTIFY oxy_run_events, '<run_id>'` in the write's own
/// transaction (`agentic_runtime::crud::events`), the task router's existing
/// LISTEN connection hears it, and it rings the
/// `RuntimeState::remote_wakes` entry this stream subscribes below. That is
/// the hot path: one indexed read per batch of events, however many pods.
///
/// The timer is what is left when the NOTIFY path is not delivering: the
/// listener connection is down (it reconnects with 200 ms–5 s backoff and a
/// 300 s keepalive, and rings every parked stream once on reconnect), the
/// fan-out is not yet bound at boot, or a notification was lost in the gap.
/// A stream never hangs on the listener's health; it is at most this much
/// slower for as long as the listener is down. So the value is sized by the
/// connection-pool budget, not by felt latency — see
/// [`ROW_CHECK_EVERY_N_TIMED_WAKES`] for the arithmetic.
///
/// Why not a dedicated LISTEN connection per stream, or per pod: a second
/// permanent LISTEN connection per pod is the horizontal-scale cap
/// `adr-postgres-as-worker-queue.md` §1/§3 warns of. This rides the one the
/// router already holds, as one more channel.
const REMOTE_DRIVER_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// The backed-off interval used once a stream has been quiet for
/// [`IDLE_TICKS_BEFORE_BACKOFF`].
///
/// A run that is `awaiting_input` is not terminal and deliberately never times
/// out (`SuspendedHuman`), so its stream can legitimately stay open for hours.
/// At the old 1 s fast interval that was two indexed queries a second, per
/// open browser, for the whole wait — a real cost on a handler shared by every
/// domain, not a hypothetical one — and even at 10 s it is a query the NOTIFY
/// path makes unnecessary.
///
/// Backing off is safe here precisely because it only engages after a long
/// quiet stretch: an active pipeline emits events continuously and resets to
/// the fast interval, and a run waiting to be claimed is picked up in
/// well under the idle threshold, so neither case ever reaches this value.
const IDLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Consecutive event-free polls before [`IDLE_POLL_INTERVAL`] takes over.
///
/// Sized to be far longer than any realistic wait for a driver to claim a
/// freshly-enqueued run, so the back-off can never be what makes a pipeline
/// look slow to start. A worker claims in ~300 ms; the ungated periodic
/// stranded tick is the slow path at ~30 s. 9 × 10 s = 90 s clears both —
/// the same wall-clock as before the fast tier went from 1 s to 10 s.
const IDLE_TICKS_BEFORE_BACKOFF: u32 = 9;

/// Ask the run row on one in every N **timed** wakes, not on all of them.
///
/// ## Why this is paced rather than free
///
/// `idle_timeout` cannot reap a connection any poller keeps touching — sqlx
/// holds idle connections in a FIFO `ArrayQueue` and `release()` restamps
/// `idle_since`, so a sequential caller round-robins the whole idle set and
/// resets the very clock the reaper measures. The threshold is
/// `N_idle / idle_timeout` acquires per second: **0.27/s** at the default
/// ceiling of 80, and **0.13/s** at the ceiling of 40 the incident review
/// recommends. That is the standing condition behind the 189-connection
/// plateau on oxy-prod (2026-09-01) — six hours of 30 s stalls and ~2,000
/// refused connections an hour — and it is still live, so anything that adds a
/// steady poll has to be sized against it rather than assumed cheap.
///
/// An idle SSE stream parked on a notifier issues **zero** acquires, and a
/// remote-driven one now parks on the `oxy_run_events` NOTIFY the same way.
/// What is left is the safety-net timer, and it is spent deliberately. **Both**
/// queries count: `get_events_after` runs on every wake, `get_run` on one
/// timed wake in `N`.
///
/// | stream | interval | acquires/sec |
/// | --- | --- | --- |
/// | local driver, notified wake | — | 0 |
/// | driver may be elsewhere, NOTIFY wake | — | one read per flush, as local |
/// | local driver, quiet | 30 s | 0.033 + 0.007 = **0.04** |
/// | driver may be elsewhere — safety net, inside the back-off | 10 s | 0.1 + 0.02 = **0.12** |
/// | either, quiet ≥ `IDLE_TICKS_BEFORE_BACKOFF` | 30 s | **0.04** |
///
/// 0.12/s is under the stricter reaper threshold of 0.13/s (ceiling 40,
/// `idle_timeout` 300 s) for a single stream, where the 1 s tier it replaces
/// sat at 1.2/s — ten times over, for every open remote-driven stream, for the
/// length of the run. The NOTIFY path carries the events; the 10 s read is
/// what a stream costs when nothing is ringing it, and it decays to 0.04/s
/// after 90 s quiet. The [`poll_after`] test pins the arithmetic.
///
/// The fast tier is keyed on *whether the driver may be elsewhere*, not on
/// `source_type` alone: a stream that found no notifier here was registered
/// by no local driver, whatever its kind. `notifier` is snapshotted once
/// before the loop and never re-read, so that state is permanent for the
/// stream's life — see the gate at the park.
///
/// An earlier version of this table read `driver local … ~0` and was wrong: it
/// counted only `get_run`, and `timed_wakes` not advancing on a notified wake
/// gates that query alone. `get_events_after` fires on every wake regardless,
/// so before `driver_may_be_remote` existed an analytics stream paid ~1.2/s
/// through every LLM call and warehouse query — gaps long enough to time out
/// but short enough to keep resetting `idle_polls`, so it never reached the
/// back-off. The same mistake is why the NOTIFY fan-out rings a registry of
/// its own (`RuntimeState::remote_wakes`) and not `notifiers`: every pod
/// hears its own appends too, and a second ring per flush on a locally-driven
/// stream is a second `get_events_after` that returns nothing.
///
/// Hence the gate: only a stream whose driver may be elsewhere subscribes to
/// the NOTIFY and pays the 10 s safety net. Everything else sits at 30 s,
/// where the notifier is doing the real work and the timer is a safety net.
pub const ROW_CHECK_EVERY_N_TIMED_WAKES: u32 = 5;

/// How long to park before the next timed read.
///
/// Pure, so the tiers can be asserted without a stream: a stream whose driver
/// may be elsewhere takes [`REMOTE_DRIVER_POLL_INTERVAL`] as its safety net
/// until it has been quiet for [`IDLE_TICKS_BEFORE_BACKOFF`] polls; every
/// other stream, and every stream once quiet, sits at [`IDLE_POLL_INTERVAL`].
/// Shared so the custom-app stream (`oxy-api-custom-apps`' `agent_run_stream`) spends the
/// same budget.
pub fn poll_after(driver_may_be_elsewhere: bool, idle_polls: u32) -> std::time::Duration {
    if driver_may_be_elsewhere && idle_polls < IDLE_TICKS_BEFORE_BACKOFF {
        REMOTE_DRIVER_POLL_INTERVAL
    } else {
        IDLE_POLL_INTERVAL
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        IDLE_POLL_INTERVAL, IDLE_TICKS_BEFORE_BACKOFF, REMOTE_DRIVER_POLL_INTERVAL,
        ROW_CHECK_EVERY_N_TIMED_WAKES, poll_after, terminal_event_for_status,
    };

    /// The timer is a safety net behind the NOTIFY path: fast only for a
    /// stream whose driver may be elsewhere, and only while it is not quiet.
    #[test]
    fn a_stream_whose_driver_may_be_elsewhere_polls_as_a_safety_net_then_backs_off() {
        assert_eq!(poll_after(true, 0), REMOTE_DRIVER_POLL_INTERVAL);
        assert_eq!(
            poll_after(true, IDLE_TICKS_BEFORE_BACKOFF - 1),
            REMOTE_DRIVER_POLL_INTERVAL
        );
        assert_eq!(
            poll_after(true, IDLE_TICKS_BEFORE_BACKOFF),
            IDLE_POLL_INTERVAL
        );
        assert_eq!(poll_after(true, u32::MAX), IDLE_POLL_INTERVAL);
        // A locally-driven stream's notifier is authoritative: never fast.
        assert_eq!(poll_after(false, 0), IDLE_POLL_INTERVAL);
        assert_eq!(
            poll_after(false, IDLE_TICKS_BEFORE_BACKOFF),
            IDLE_POLL_INTERVAL
        );
    }

    /// The tiers are sized against the 2026-09-01 connection-pool incident
    /// (`internal-docs/worker-fleet.md`, "Polling and the connection pool"):
    /// `idle_timeout` cannot reap a connection a poller keeps touching, and the
    /// threshold at the recommended ceiling of 40 is 0.13 acquires/s. Both
    /// queries in the loop count.
    #[test]
    fn the_safety_net_stays_under_the_pool_reaper_threshold() {
        let acquires_per_tick = 1.0 + 1.0 / f64::from(ROW_CHECK_EVERY_N_TIMED_WAKES);
        let fast = acquires_per_tick / REMOTE_DRIVER_POLL_INTERVAL.as_secs_f64();
        let idle = acquires_per_tick / IDLE_POLL_INTERVAL.as_secs_f64();
        assert!(
            fast < 0.13,
            "fast tier spends {fast:.3} acquires/s per stream"
        );
        assert!(
            idle < 0.05,
            "idle tier spends {idle:.3} acquires/s per stream"
        );
        // The events read alone is at most one per 10 s per idle stream.
        assert!(REMOTE_DRIVER_POLL_INTERVAL >= Duration::from_secs(10));
        // The back-off still engages at the same wall-clock the 1 s tier had,
        // so a slow claim never looks like a slow pipeline.
        assert_eq!(
            REMOTE_DRIVER_POLL_INTERVAL * IDLE_TICKS_BEFORE_BACKOFF,
            Duration::from_secs(90)
        );
    }

    #[test]
    fn failed_status_maps_to_error_event_with_message() {
        let (ty, payload) = terminal_event_for_status(
            Some("failed"),
            Some("semantic catalog error: bad yaml"),
            None,
            "r1",
        )
        .expect("failed is terminal");
        assert_eq!(ty, "error");
        assert_eq!(payload["message"], "semantic catalog error: bad yaml");
        assert_eq!(payload["trace_id"], "r1");
    }

    #[test]
    fn timed_out_maps_to_error_event() {
        let (ty, _) = terminal_event_for_status(Some("timed_out"), None, None, "r1")
            .expect("timed_out is terminal");
        assert_eq!(ty, "error");
    }

    #[test]
    fn failed_without_message_uses_fallback_text() {
        let (_, payload) = terminal_event_for_status(Some("failed"), None, None, "r1").unwrap();
        assert_eq!(
            payload["message"],
            "The run failed before it could report an error."
        );
    }

    #[test]
    fn cancelled_and_done_map_to_their_events() {
        assert_eq!(
            terminal_event_for_status(Some("cancelled"), None, None, "r1")
                .unwrap()
                .0,
            "cancelled"
        );
        let (ty, payload) =
            terminal_event_for_status(Some("done"), None, Some("42"), "r1").unwrap();
        assert_eq!(ty, "done");
        assert_eq!(payload["answer"], "42");
    }

    #[test]
    fn non_terminal_statuses_do_not_synthesize() {
        for s in ["running", "awaiting_input", "delegating"] {
            assert!(terminal_event_for_status(Some(s), None, None, "r1").is_none());
        }
        assert!(terminal_event_for_status(None, None, None, "r1").is_none());
    }
}
