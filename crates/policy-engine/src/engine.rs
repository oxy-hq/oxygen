//! One V8 isolate with the policy ops registered and a policy script loaded.

use std::path::Path;
use std::rc::Rc;

use deno_core::{JsRuntime, ModuleSpecifier, PollEventLoopOptions, RuntimeOptions, v8};

use crate::loader::JsModuleLoader;
use crate::ops::oxy_policy_ext;
use crate::policy::{AuditLog, PolicyDecision};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The script could not be found, transpiled, loaded or evaluated.
    #[error("policy script failed to boot: {0}")]
    Boot(String),
    /// `decide` threw, or an op it called refused.
    #[error("policy script threw: {0}")]
    Script(String),
    /// `decide` returned something that is not a [`PolicyDecision`].
    #[error("policy script returned an invalid decision: {0}")]
    Decode(String),
}

/// A policy script, loaded once into its own isolate and asked for decisions
/// one SKU at a time. `!Send`, like the `JsRuntime` it owns: drive it from a
/// current-thread Tokio runtime.
pub struct PolicyEngine {
    runtime: JsRuntime,
    /// The script's `decide` export, read once at boot. Calling it is one
    /// function call per decision: no source is compiled per SKU.
    decide: v8::Global<v8::Function>,
}

impl PolicyEngine {
    /// Load and evaluate `script` (a `.js` ES module, compiled from `ts/` by
    /// `build-js.sh`, exporting
    /// `decide(sku)`), with the ops registered and an empty audit log.
    pub async fn boot(script: &Path) -> Result<Self, EngineError> {
        let path = std::fs::canonicalize(script)
            .map_err(|e| EngineError::Boot(format!("{}: {e}", script.display())))?;
        let script = ModuleSpecifier::from_file_path(&path)
            .map_err(|()| EngineError::Boot(format!("not a file path: {}", path.display())))?;
        let mut runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(Rc::new(JsModuleLoader)),
            extensions: vec![oxy_policy_ext::init()],
            ..Default::default()
        });
        let id = runtime
            .load_main_es_module(&script)
            .await
            .map_err(|e| EngineError::Boot(e.to_string()))?;
        let evaluated = runtime.mod_evaluate(id);
        runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .map_err(|e| EngineError::Boot(e.to_string()))?;
        evaluated
            .await
            .map_err(|e| EngineError::Boot(e.to_string()))?;
        let decide = decide_export(&mut runtime, id)?;
        Ok(Self { runtime, decide })
    }

    /// Call the script's `decide(sku)` and return the decision it made.
    ///
    /// The script is expected to have written the same decision through the
    /// audit op on the way; [`Self::audit_log`] is where that write landed.
    pub async fn decide(&mut self, sku: &str) -> Result<PolicyDecision, EngineError> {
        let sku = {
            let runtime = &mut self.runtime;
            deno_core::scope!(scope, runtime);
            let sku = v8::String::new(scope, sku)
                .ok_or_else(|| EngineError::Script("SKU too long for a V8 string".into()))?;
            v8::Global::new(scope, v8::Local::<v8::Value>::from(sku))
        };
        // The call's future settles only while the event loop is polled, which
        // matters once `decide` is async or awaits an async op.
        let call = self.runtime.call_with_args(&self.decide, &[sku]);
        let value = self
            .runtime
            .with_event_loop_promise(Box::pin(call), PollEventLoopOptions::default())
            .await
            .map_err(|e| EngineError::Script(e.to_string()))?;
        let runtime = &mut self.runtime;
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, value);
        deno_core::serde_v8::from_v8(scope, local).map_err(|e| EngineError::Decode(e.to_string()))
    }

    /// Every decision written through the audit op so far, oldest first.
    pub fn audit_log(&mut self) -> Vec<PolicyDecision> {
        self.runtime
            .op_state()
            .borrow()
            .borrow::<AuditLog>()
            .0
            .clone()
    }

    /// The isolate's live heap, in bytes, measured after asking V8 to collect
    /// everything it can — so a rising number is retained memory, not garbage
    /// waiting for a collection.
    pub fn retained_heap_bytes(&mut self) -> usize {
        let isolate = self.runtime.v8_isolate();
        isolate.low_memory_notification();
        isolate.get_heap_statistics().used_heap_size()
    }
}

/// The module's `decide` export, or a boot error naming what is wrong with it.
fn decide_export(
    runtime: &mut JsRuntime,
    id: deno_core::ModuleId,
) -> Result<v8::Global<v8::Function>, EngineError> {
    let namespace = runtime
        .get_module_namespace(id)
        .map_err(|e| EngineError::Boot(e.to_string()))?;
    deno_core::scope!(scope, runtime);
    let namespace = v8::Local::new(scope, namespace);
    let key = v8::String::new(scope, "decide").expect("a short literal fits a V8 string");
    let export = namespace
        .get(scope, key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
        .ok_or_else(|| EngineError::Boot("policy script has no `decide` function export".into()))?;
    Ok(v8::Global::new(scope, export))
}
