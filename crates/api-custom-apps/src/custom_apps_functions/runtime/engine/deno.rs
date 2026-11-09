//! The `deno_core` (V8) engine: the only [`IsolateEngine`] today.
//!
//! Everything that names a `deno_core` type lives under this module — the
//! `#[op2]` bindings and `extension!` in [`ops`], and the `JsRuntime`
//! lifecycle below.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use deno_core::{JsRuntime, ModuleId, RuntimeOptions, v8};

use super::{IsolateConfig, IsolateEngine, TerminateHandle};
use crate::custom_apps_functions::runtime::RuntimeError;
use crate::custom_apps_functions::runtime::ops::{OpContext, RequestBody};

mod ops;
mod snapshot;
mod v8_platform;

/// How much headroom the near-limit callback grants so V8 can finish unwinding.
///
/// The callback must return a limit **above** the current one. Returning the
/// same value makes V8 abort the process immediately — the exact outcome the
/// ceiling exists to prevent — because from V8's perspective the limit was
/// raised to a value it has already exceeded. The grant is generous on purpose:
/// it is only ever used for the few allocations between the callback firing and
/// `terminate_execution` taking effect, and it is bounded because the isolate
/// is already being torn down.
const HEAP_LIMIT_GRACE_BYTES: usize = 32 * 1024 * 1024;

/// What a **repeat** near-limit fire grants.
///
/// A second fire means the isolate breached the already-raised limit before
/// `terminate_execution` landed — a tight allocation loop. Granting the full
/// [`HEAP_LIMIT_GRACE_BYTES`] again would let it ratchet the ceiling upward
/// 32 MiB at a time, which is the unbounded growth the ceiling exists to stop.
/// This is the smallest grant that still satisfies V8's "must exceed the
/// current limit" rule, so the loop buys almost nothing per fire while the
/// termination continues to land.
const HEAP_LIMIT_RATCHET_BYTES: usize = 1024 * 1024;

pub(in super::super) struct DenoEngine {
    runtime: JsRuntime,
    oom: Arc<AtomicBool>,
}

pub(in super::super) struct DenoTerminate(v8::IsolateHandle);

impl TerminateHandle for DenoTerminate {
    fn terminate(&self) {
        self.0.terminate_execution();
    }
}

impl DenoEngine {
    /// Start from `snapshot` when there is one; otherwise run the bootstrap
    /// script on a bare isolate, which is what every isolate did before the
    /// snapshot and what an isolate does if the snapshot could not be built.
    fn create_with(config: IsolateConfig, snapshot: Option<&'static [u8]>) -> Result<Self, String> {
        // The runtime's V8 flags, before this or any isolate exists.
        v8_platform::init();
        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![ops::oxy_functions_ext::init()],
            startup_snapshot: snapshot,
            // The ceiling. Initial is left at 0 so V8 sizes the young generation
            // itself — pinning it would cost startup time on every invocation to
            // constrain something that was never the problem.
            create_params: config
                .heap_limit
                .map(|max| v8::CreateParams::default().heap_limits(0, max)),
            ..Default::default()
        });
        let oom = Arc::new(AtomicBool::new(false));
        if config.heap_limit.is_some() {
            terminate_near_heap_limit(&mut runtime, Arc::clone(&oom));
        }
        if snapshot.is_none() {
            runtime
                .execute_script(snapshot::BOOTSTRAP_NAME, config.bootstrap)
                .map_err(|e| e.to_string())?;
        }
        Ok(Self { runtime, oom })
    }

    /// An isolate that runs the bootstrap script itself instead of starting
    /// from the snapshot — what [`IsolateEngine::create`] did before the
    /// snapshot existed. Kept so a test can compare the two.
    #[cfg(test)]
    pub(in super::super) fn create_without_snapshot(config: IsolateConfig) -> Result<Self, String> {
        Self::create_with(config, None)
    }
}

impl IsolateEngine for DenoEngine {
    type Terminate = DenoTerminate;
    type Module = ModuleId;

    fn create(config: IsolateConfig) -> Result<Self, String> {
        let snapshot = snapshot::bootstrap_snapshot(config.bootstrap);
        Self::create_with(config, snapshot)
    }

    fn oom_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.oom)
    }

    fn bind_invocation(&mut self, ops: OpContext) {
        let state = self.runtime.op_state();
        let mut state = state.borrow_mut();
        // Whatever the previous invocation left behind goes first, so an op
        // can never read a value this invocation did not put there.
        let _ = state.try_take::<RequestBody>();
        state.put(ops);
    }

    fn terminate_handle(&mut self) -> DenoTerminate {
        DenoTerminate(self.runtime.v8_isolate().thread_safe_handle())
    }

    async fn load_module(
        &mut self,
        specifier: &'static str,
        code: String,
    ) -> Result<ModuleId, RuntimeError> {
        let specifier = deno_core::resolve_url(specifier)
            .map_err(|e| RuntimeError::Internal(format!("bad module specifier: {e}")))?;
        self.runtime
            .load_side_es_module_from_code(&specifier, code)
            .await
            .map_err(|e| RuntimeError::Js(format!("module load failed: {e}")))
    }

    async fn evaluate_module(&mut self, module: ModuleId) -> Result<(), RuntimeError> {
        let eval = self.runtime.mod_evaluate(module);
        self.runtime
            .run_event_loop(Default::default())
            .await
            .map_err(|e| RuntimeError::Js(e.to_string()))?;
        eval.await
            .map_err(|e| RuntimeError::Js(format!("module evaluation failed: {e}")))
    }

    fn park_request_body(&mut self, body: String) {
        self.runtime.op_state().borrow_mut().put(RequestBody(body));
    }

    async fn invoke(
        &mut self,
        name: &'static str,
        source: String,
    ) -> Result<serde_json::Value, RuntimeError> {
        let promise = self
            .runtime
            .execute_script(name, source)
            .map_err(|e| RuntimeError::Js(format!("invoke failed: {e}")))?;
        // `resolve()` alone does NOT pump the event loop — it just returns a future
        // that settles when the promise does. A handler whose promise awaits an
        // async host op (ctx.query/fetch/…) would then hang forever: the op's reply
        // arrives on the broker, but nothing re-polls the isolate to deliver it back
        // into JS, so the promise never resolves (→ wall-clock timeout). Drive the
        // event loop while awaiting, exactly like the module-eval path does.
        let resolve_fut = Box::pin(self.runtime.resolve(promise));
        let result = self
            .runtime
            .with_event_loop_promise(resolve_fut, deno_core::PollEventLoopOptions::default())
            .await
            .map_err(|e| RuntimeError::Js(e.to_string()))?;

        // deno_core 0.410 removed `JsRuntime::handle_scope()`; `scope!` is the
        // exported replacement (it enters the runtime's main context the same way).
        let runtime = &mut self.runtime;
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, result);
        deno_core::serde_v8::from_v8(scope, local)
            .map_err(|e| RuntimeError::Internal(format!("result deserialize failed: {e}")))
    }
}

/// Terminate the isolate when it nears its heap ceiling, recording why in
/// `oom` first.
fn terminate_near_heap_limit(runtime: &mut JsRuntime, oom: Arc<std::sync::atomic::AtomicBool>) {
    let terminate_handle = runtime.v8_isolate().thread_safe_handle();
    runtime.add_near_heap_limit_callback(move |current, _initial| {
        // Ordering matters: record the cause before terminating, so the
        // error path cannot observe the termination without the reason.
        // `swap` also tells us whether this is a REPEAT fire.
        let already_terminating = oom.swap(true, Ordering::Relaxed);
        terminate_handle.terminate_execution();
        // The return must exceed `current`, or V8 aborts the process right
        // here rather than letting the termination unwind.
        //
        // The first fire grants real headroom for the unwind. A repeat fire
        // means the isolate breached the *raised* limit before the
        // termination landed — so granting another full 32 MiB each time
        // would let a tight allocation loop ratchet the ceiling upward
        // without bound, which is the failure the ceiling exists to
        // prevent. Subsequent fires grant the minimum that keeps V8 from
        // aborting.
        if already_terminating {
            current + HEAP_LIMIT_RATCHET_BYTES
        } else {
            current + HEAP_LIMIT_GRACE_BYTES
        }
    });
}
