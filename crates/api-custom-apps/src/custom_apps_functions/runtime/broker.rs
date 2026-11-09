//! The caller's side of a run: admission, then the loop that services the
//! isolate's host calls and enforces cancel, timeout and grace.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{mpsc, oneshot};

use super::super::LogLine;
use super::engine::{ActiveEngine, IsolateEngine, TerminateHandle};
use super::host_call::HostCall;
use super::isolate_limits::TIMEOUT_GRACE;
use super::isolate_thread::IsolateRun;
use super::warm_pool::{self, Job};
use super::{FnRequest, FnResponse, FunctionHost, InvocationCtx, RuntimeError, host_call_task};
use crate::custom_apps_telemetry::InvocationMeters;

/// [`run`] on an explicit warm pool, so a test can pick the pool's size.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_in(
    pool: &Arc<warm_pool::WarmPool>,
    artifact_js: String,
    // Whose invocation this is, for the per-org concurrency ceiling. Not part
    // of `InvocationCtx` on purpose: that struct is serialized into the isolate
    // as `ctx`, and the org id is a platform fact the tenant has no business
    // reading.
    org_id: uuid::Uuid,
    ctx: InvocationCtx,
    req: FnRequest,
    host: std::sync::Arc<dyn FunctionHost>,
    mut cancel: oneshot::Receiver<()>,
    timeout: std::time::Duration,
    // Shared with the caller: the isolate appends `console.*`/`ctx.log` here and
    // the caller drains it after (surfaced back to the app, not just tracing).
    logs: Arc<std::sync::Mutex<Vec<LogLine>>>,
    // Shared with the caller for the same reason as `logs`: the isolate thread
    // can outlive this call, so what it measures cannot be a return value.
    meters: InvocationMeters,
    // The span the isolate thread runs inside. Must be PARENTLESS — see the
    // comment at the spawn below for why a clone of the caller's span is the
    // wrong thing here.
    isolate_span: tracing::Span,
) -> Result<FnResponse, RuntimeError> {
    // Admission first, before anything is allocated. The cap exists to stop the
    // OS thread and the V8 heap from being created at all, so it has to gate
    // the spawn rather than the work after it — a permit taken later would
    // limit concurrency while still paying for every thread.
    let queued_at = std::time::Instant::now();
    let org_label = org_id.to_string();
    let _admission = match super::super::limits::admit(org_id, &mut cancel).await {
        Ok(permit) => {
            oxy_telemetry::metrics::record::custom_app_admission_wait(
                &org_label,
                super::super::limits::elapsed_since(queued_at),
            );
            permit
        }
        // The caller left while queued. Nothing was refused and nobody is
        // waiting for an answer, so this is a cancellation like any other —
        // counting it as a shed would make the shed rate track how impatient
        // users are rather than how loaded the fleet is.
        Err(reason) if !reason.is_shed() => {
            tracing::debug!(
                target: "oxy.custom_app.admission",
                queued_ms = queued_at.elapsed().as_millis() as u64,
                "caller went away while queued for a concurrency permit"
            );
            return Err(RuntimeError::Cancelled);
        }
        Err(reason) => {
            oxy_telemetry::metrics::record::custom_app_admission_shed(&org_label, reason.as_str());
            tracing::warn!(
                target: "oxy.custom_app.admission",
                reason = reason.as_str(),
                queued_ms = queued_at.elapsed().as_millis() as u64,
                "shed a function invocation: no concurrency permit came free"
            );
            return Err(RuntimeError::Overloaded);
        }
    };

    let (call_tx, mut call_rx) = mpsc::unbounded_channel::<HostCall>();
    let (done_tx, done_rx) = oneshot::channel::<Result<FnResponse, RuntimeError>>();
    let (handle_tx, handle_rx) = oneshot::channel::<<ActiveEngine as IsolateEngine>::Terminate>();
    let cancelled = Arc::new(AtomicBool::new(false));

    // Sentry hubs are per thread: the job carries this invocation's hub onto
    // the isolate thread (see `spawn_cold`). The thread comes from the warm
    // pool when one is ready, and is built here otherwise.
    warm_pool::start_isolate(
        pool,
        Job {
            run: IsolateRun {
                artifact_js,
                ctx,
                req,
                call_tx,
                cancelled: cancelled.clone(),
                logs,
                meters: meters.clone(),
            },
            handle_tx,
            done_tx,
            hub: sentry::Hub::current(),
            span: isolate_span,
        },
    )?;

    // Grab the isolate handle so cancellation can terminate execution even
    // when the isolate is stuck in a compute-only loop (design doc §11.4).
    // Held in a guard: if this future is dropped (a route caller hung up and
    // axum dropped the handler), nothing below runs, so the isolate would
    // otherwise run on to completion with its admission permit already gone.
    let mut isolate_handle = TerminateOnDrop {
        handle: handle_rx.await.ok(),
        cancelled: cancelled.clone(),
        finished: false,
    };

    tokio::pin!(done_rx);
    let sleep = tokio::time::sleep(timeout);
    tokio::pin!(sleep);
    // Grace timer, armed only after the wall-clock timeout fires. Starts
    // already-elapsed but is gated off by `if timed_out` until then.
    let grace = tokio::time::sleep(std::time::Duration::ZERO);
    tokio::pin!(grace);
    let mut timed_out = false;
    loop {
        tokio::select! {
            // Cancellation: terminate the isolate; the thread's done_tx will
            // then deliver an error/partial which we map to Cancelled.
            _ = &mut cancel => {
                isolate_handle.terminate();
                return Err(RuntimeError::Cancelled);
            }
            // Wall-clock timeout: terminate the isolate the same way as
            // cancel, then keep looping until the isolate thread actually
            // exits (`done_rx` below) so the OS thread + V8 isolate don't
            // outlive this call — but only up to `TIMEOUT_GRACE` (armed here),
            // after which we give up waiting (see the grace branch).
            _ = &mut sleep, if !timed_out => {
                timed_out = true;
                isolate_handle.terminate();
                grace
                    .as_mut()
                    .reset(tokio::time::Instant::now() + TIMEOUT_GRACE);
            }
            // Grace expired: the isolate did not exit within `TIMEOUT_GRACE`
            // of termination. This only happens if it's parked in a host call
            // that hasn't returned (terminate_execution can't interrupt a
            // pending host await — there's no running JS to throw into). Host
            // ops are individually bounded (ctx.fetch carries connect+total
            // timeouts; connectors carry their own), so rather than block this
            // request indefinitely we return Timeout and let the detached
            // thread unwind on its own once the host op completes — its
            // `done_tx`/`call_tx` sends then no-op against our dropped ends.
            _ = &mut grace, if timed_out => {
                // Deliberate, and worth counting: see `abandoned_isolates`.
                let abandoned_total = oxy_telemetry::metrics::sources::isolate_abandoned();
                tracing::warn!(
                    abandoned_total,
                    grace_secs = TIMEOUT_GRACE.as_secs(),
                    "isolate thread did not exit after termination; detaching it"
                );
                return Err(RuntimeError::Timeout);
            }
            // Service a host call from the isolate on a task of its own, so
            // concurrent ctx.* awaits inside one function don't serialize
            // (`host_call_task`: its span, its failure note, its trace record).
            maybe_call = call_rx.recv() => {
                match maybe_call {
                    Some(call) => host_call_task::spawn_host_call(call, host.clone(), &meters),
                    None => { /* sender dropped; isolate is finishing */ }
                }
            }
            done = &mut done_rx => {
                isolate_handle.finished = true;
                // `select!` picks among ready branches at random, so a call
                // the isolate sent just before it finished can still be
                // queued here. Count it now, or an un-awaited call goes
                // missing from the trace instead of reading as unfinished.
                while let Ok(call) = call_rx.try_recv() {
                    host_call_task::spawn_host_call(call, host.clone(), &meters);
                }
                if timed_out {
                    return Err(RuntimeError::Timeout);
                }
                return done.unwrap_or_else(|_| {
                    Err(RuntimeError::Internal("isolate thread vanished".into()))
                });
            }
        }
    }
}

/// The isolate's terminate handle, fired on drop unless the isolate reported
/// back. Isolates are single-use (`warm_pool`), so a late terminate can only
/// ever reach the isolate this run started.
struct TerminateOnDrop {
    handle: Option<<ActiveEngine as IsolateEngine>::Terminate>,
    cancelled: Arc<AtomicBool>,
    finished: bool,
}

impl TerminateOnDrop {
    fn terminate(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(h) = &self.handle {
            h.terminate();
        }
    }
}

impl Drop for TerminateOnDrop {
    fn drop(&mut self) {
        if !self.finished {
            self.terminate();
        }
    }
}
