//! The V8 startup snapshot of the bootstrap script.
//!
//! Every isolate used to be created bare and then parse and run
//! `BOOTSTRAP_JS` (about 400 lines: the `Response` polyfill, base64, the
//! `console` shim and `__buildCtx`) before the tenant's module was loaded.
//! The script is the same text for every invocation, so it is run once, in a
//! throwaway isolate, and the heap it leaves behind is serialized. A new
//! isolate deserializes that heap instead of re-running the script.
//!
//! ## What the snapshot holds
//!
//! Only what the bootstrap script defines. The `ctx.*` ops are registered
//! by the same extension at snapshot time and at start-up, so the snapshot
//! refers to them by position; nothing per-invocation is in it, and the
//! `OpContext` is parked in the op state after the isolate exists
//! (`IsolateEngine::bind_invocation`), never captured in the heap.
//!
//! ## V8 must not be initialized for snapshotting
//!
//! `JsRuntimeForSnapshot` initializes V8 in snapshot mode when it is the
//! first thing in the process to touch V8, and that mode is process-wide: it
//! sets `--predictable --random-seed=42`, which would make every tenant's
//! `Math.random()` the same sequence in every process and serialize V8's
//! garbage collection. [`v8_platform::init`](super::v8_platform::init) runs
//! first so the mode is always the one isolates run under, with the runtime's
//! V8 flags, and the snapshot is built under it.

use std::sync::OnceLock;

use deno_core::{JsRuntimeForSnapshot, RuntimeOptions};

use super::{ops, v8_platform};

/// The script name the bootstrap runs under, in a snapshot or on a bare
/// isolate. Appears in stack traces, so it must not depend on the path taken.
pub(super) const BOOTSTRAP_NAME: &str = "oxy:bootstrap";

/// The snapshot of `bootstrap`, built the first time it is asked for and
/// shared by every isolate after. `None` when it could not be built, in which
/// case the caller runs the script on a bare isolate as before.
///
/// One bootstrap script per process: the second caller gets the first
/// caller's snapshot whatever it passes, which is correct only because
/// `IsolateConfig::bootstrap` is a constant.
pub(super) fn bootstrap_snapshot(bootstrap: &'static str) -> Option<&'static [u8]> {
    static SNAPSHOT: OnceLock<Option<&'static [u8]>> = OnceLock::new();
    *SNAPSHOT.get_or_init(|| {
        let started = std::time::Instant::now();
        let built = std::panic::catch_unwind(|| build(bootstrap))
            .unwrap_or_else(|_| Err("snapshot builder panicked".to_string()));
        match built {
            Ok(bytes) => {
                tracing::info!(
                    target: "oxy.custom_app.snapshot",
                    bytes = bytes.len(),
                    build_ms = started.elapsed().as_millis() as u64,
                    "built the function bootstrap snapshot"
                );
                // Lives for the rest of the process, as `startup_snapshot`
                // requires.
                Some(&*Box::leak(bytes))
            }
            Err(error) => {
                tracing::warn!(
                    target: "oxy.custom_app.snapshot",
                    %error,
                    "could not build the function bootstrap snapshot; isolates will run the bootstrap script themselves"
                );
                None
            }
        }
    })
}

fn build(bootstrap: &'static str) -> Result<Box<[u8]>, String> {
    v8_platform::init();
    let mut runtime = JsRuntimeForSnapshot::try_new(RuntimeOptions {
        extensions: vec![ops::oxy_functions_ext::init()],
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    runtime
        .execute_script(BOOTSTRAP_NAME, bootstrap)
        .map_err(|e| e.to_string())?;
    Ok(runtime.snapshot())
}
