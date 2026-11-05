//! The two ops the TypeScript SDK calls. Each takes and returns JSON text, so
//! the boundary has a single encoding in both directions.

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;

use crate::policy::{AuditLog, PolicyDecision};
use crate::store::PolicySource;

/// `OxyContext.getPolicy(name)` — reads the policy from the engine's
/// [`PolicySource`]. Async: with the OLTP database behind it, the script awaits a
/// promise while the query runs.
#[op2]
#[string]
async fn op_oxy_get_policy(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
) -> Result<String, JsErrorBox> {
    // Clone the source out and drop the borrow: holding a `RefCell` borrow of
    // the op state across an await panics the next op that touches it.
    let source = state.borrow().borrow::<PolicySource>().clone();
    let policy = source
        .fetch(&name)
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    serde_json::to_string(&policy).map_err(|e| JsErrorBox::generic(e.to_string()))
}

/// `OxyAudit.record(decision)` — mocks the `policy_decisions` write. The JSON
/// is parsed into [`PolicyDecision`] here, at the edge, so a malformed
/// decision throws in the script that wrote it.
#[op2(fast)]
fn op_oxy_write_decision(state: &mut OpState, #[string] decision: &str) -> Result<(), JsErrorBox> {
    let decision: PolicyDecision = serde_json::from_str(decision)
        .map_err(|e| JsErrorBox::type_error(format!("invalid policy decision: {e}")))?;
    state.borrow_mut::<AuditLog>().0.push(decision);
    Ok(())
}

deno_core::extension!(
    oxy_policy_ext,
    ops = [op_oxy_get_policy, op_oxy_write_decision],
    state = |state| state.put(AuditLog::default()),
);
