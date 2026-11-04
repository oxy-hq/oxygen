//! The isolate thread's side of a run: build (or receive) an isolate, bind
//! the invocation to it, evaluate the tenant module and call its export.
//! Engine-neutral: every engine call goes through [`IsolateEngine`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{mpsc, oneshot};

use super::super::LogLine;
use super::engine::{ActiveEngine, IsolateConfig, IsolateEngine};
use super::host_call::HostCall;
use super::isolate_limits::heap_limit_bytes;
use super::ops::{FunctionLogs, OpContext};
use super::warm_pool::Job;
use super::{BOOTSTRAP_JS, FnRequest, FnResponse, InvocationCtx, RuntimeError};
use crate::custom_apps_telemetry::InvocationMeters;

/// Build the isolate thread on the request path — the thread, its Tokio
/// runtime and the isolate — and run `job` on it. What every invocation did
/// before the warm pool, and what one still does when the pool has nothing
/// ready or is off (`OXY_FUNCTION_WARM_ISOLATES=0`).
pub(super) fn spawn_cold(job: Job) -> std::io::Result<()> {
    let Job {
        run,
        handle_tx,
        done_tx,
        hub: sentry_hub,
        span: isolate_span,
    } = job;
    // Isolate runs on its own thread with a current-thread runtime.
    //
    // `tracing`'s notion of "the current span" is THREAD-LOCAL, so a span
    // opened by the caller does not follow execution across this `spawn`.
    // Every `ctx.log()` / `console.*` line is emitted from the isolate thread,
    // so without a span entered *on that thread* they all fire with no current
    // span — and the observability layer drops an event that has no span.
    //
    // **`isolate_span` is deliberately PARENTLESS, not a clone of the caller's.**
    // A clone bumps the registry refcount and `on_close` fires only at zero, and
    // this thread outlives `run` on two paths: `Cancelled`, and the grace-expiry
    // branch that lets a detached thread unwind on its own. A clone would
    // therefore pin the caller's invocation span open — exporting it late, or
    // never if the thread parks in a host op forever, and inflating its
    // `duration_ns` (measured at close) to the leaked thread's whole lifetime.
    // The invocations that time out are exactly the ones worth having.
    //
    // The cost is that logs land on a sibling span rather than a child: they
    // are correlated by the `invocation_id` field both spans carry, not by the
    // trace tree. That is the right trade — a late-but-correct sibling beats a
    // parent whose duration is a lie.
    // Sentry hubs are per thread: carry this invocation's hub onto the isolate
    // thread, so a custom-app surface tag (`middlewares::sentry_surface`) still
    // covers what is captured there, panics included.
    std::thread::Builder::new()
        .name("oxy-function".into())
        .spawn(move || {
            // Counted from inside the closure, so a thread that failed to
            // spawn is never counted as live. The guard covers every exit
            // path below — the early `return` when the runtime fails to
            // build, a panic in tenant code, and the normal end — because a
            // missed decrement drifts this gauge upward forever.
            let _isolate_guard = oxy_telemetry::metrics::sources::IsolateGuard::enter();
            sentry::Hub::run(sentry_hub, || {
                // Sync closure, so holding the guard for the whole thread body
                // is correct (the "never hold across an await" rule is about
                // async fns; everything below runs inside `block_on`).
                let _span_guard = isolate_span.enter();
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = done_tx.send(Err(RuntimeError::Internal(format!(
                            "failed to build isolate runtime: {e}"
                        ))));
                        return;
                    }
                };
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    let result = execute_isolate::<ActiveEngine>(run, handle_tx).await;
                    let _ = done_tx.send(result);
                });
            })
        })
        .map(|_| ())
}

/// What one isolate needs from [`run`], moved onto the isolate thread.
pub(super) struct IsolateRun {
    pub(super) artifact_js: String,
    pub(super) ctx: InvocationCtx,
    pub(super) req: FnRequest,
    pub(super) call_tx: mpsc::UnboundedSender<HostCall>,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) logs: Arc<std::sync::Mutex<Vec<LogLine>>>,
    pub(super) meters: InvocationMeters,
}

/// Body that runs on the isolate thread.
///
/// Wraps [`execute_isolate_inner`] only to translate a heap-limit kill. A
/// terminated isolate surfaces as a generic "execution terminated" JS error,
/// byte-identical to the one a cancel or a timeout produces — the flag the
/// near-limit callback sets is the only thing that knows which it was.
async fn execute_isolate<E: IsolateEngine>(
    run: IsolateRun,
    handle_tx: oneshot::Sender<E::Terminate>,
) -> Result<FnResponse, RuntimeError> {
    let engine = E::create(IsolateConfig {
        heap_limit: heap_limit_bytes(),
        bootstrap: BOOTSTRAP_JS,
    })
    .map_err(|e| RuntimeError::Internal(format!("bootstrap failed: {e}")))?;
    execute_on_engine(engine, run, handle_tx).await
}

/// [`execute_isolate`] from the point an isolate exists — whether it was
/// created just now or built ahead of demand by the warm pool.
pub(super) async fn execute_on_engine<E: IsolateEngine>(
    mut engine: E,
    run: IsolateRun,
    handle_tx: oneshot::Sender<E::Terminate>,
) -> Result<FnResponse, RuntimeError> {
    let oom = engine.oom_flag();
    let _ = handle_tx.send(engine.terminate_handle());
    let result = execute_isolate_inner(&mut engine, run).await;

    // Checked on the error path only: a function that allocated hard, tripped
    // the callback, and *still* returned a response has not failed, and
    // rewriting its success into an error would be a lie about what the app saw.
    if result.is_err() && oom.load(Ordering::Relaxed) {
        return Err(RuntimeError::OutOfMemory);
    }
    result
}

/// The module every function artifact is loaded as; the invoke script imports
/// it by this name.
const FUNCTION_MODULE: &str = "oxy:function";

/// Loads + evaluates the module, calls the default export, and returns the
/// parsed `FnResponse`.
async fn execute_isolate_inner<E: IsolateEngine>(
    engine: &mut E,
    run: IsolateRun,
) -> Result<FnResponse, RuntimeError> {
    let IsolateRun {
        artifact_js,
        ctx,
        req,
        call_tx,
        cancelled,
        logs,
        meters,
    } = run;
    // The ops read this at call time, not at creation: the isolate does not
    // know whose invocation it serves until here.
    engine.bind_invocation(OpContext {
        call_tx,
        cancelled,
        logs: FunctionLogs(logs),
    });

    let ctx_json = serde_json::to_string(&ctx)
        .map_err(|e| RuntimeError::Internal(format!("ctx serialize failed: {e}")))?;
    let FnRequest {
        method,
        headers,
        body,
    } = req;
    // `req` without its body, which the script collects from `RequestBody`.
    let req_json = serde_json::json!({ "method": method, "headers": headers }).to_string();
    // Valid UTF-8 is moved, not copied. Anything else is decoded lossily, as it
    // always was: `req.body` is text.
    let body = String::from_utf8(body)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());

    let module = engine.load_module(FUNCTION_MODULE, artifact_js).await?;

    // The setup/tenant boundary, and it sits here rather than after
    // `evaluate_module` on purpose.
    //
    // Everything above is ours: OS thread spawn, isolate creation, the
    // bootstrap script, and compiling the tenant's module. `evaluate_module`
    // below runs the module's **top-level statements**, which are tenant code —
    // an app doing heavy work at module scope is the app's own cost, and folding
    // it into `init_ms` would let a slow function look like a slow platform.
    //
    // So `init_ms` is what we are responsible for, and `duration_ms - init_ms`
    // is what the app is.
    meters.mark_tenant_code_entered();

    engine.evaluate_module(module).await?;

    // Parked only now, after the module's top-level statements have run, and
    // collected by the script's first statements — ahead of `__buildCtx`, which
    // is a global the artifact could have replaced with code of its own.
    // `body` is assigned after the literal, which keeps `req`'s keys in the
    // order they have always had.
    engine.park_request_body(body);

    let value = engine
        .invoke("oxy:invoke", invoke_script(&req_json, &ctx_json))
        .await?;
    serde_json::from_value(value)
        .map_err(|e| RuntimeError::Internal(format!("result shape invalid: {e}")))
}

/// The script that calls the function's default export. Its completion value
/// is the promise of `{ status, body }`.
fn invoke_script(req_json: &str, ctx_json: &str) -> String {
    format!(
        r#"
        (async () => {{
            const req = {req_json};
            req.body = Deno.core.ops.op_req_take_body();
            const ctx = globalThis.__buildCtx({ctx_json});
            const mod = await import("oxy:function");
            const handler = mod.default;
            if (typeof handler !== "function") {{
                throw new Error("function module has no default export");
            }}
            const res = await handler(req, ctx);
            return {{
                status: (res && res.status) || 200,
                body: res && res.body !== undefined ? String(res.body) : "",
            }};
        }})()
        "#,
    )
}
