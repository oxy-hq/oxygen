//! The two ops the TypeScript SDK calls. Each takes and returns JSON text, so
//! the boundary has a single encoding in both directions.

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;

use crate::policy::{AuditLog, MOCK_POLICY_JSON, PolicyDecision};

/// `OxyContext.getPolicy(name)` — mocks the database read. Only `"mock"`
/// exists; any other name is refused, the way a missing row would be.
#[op2]
#[string]
fn op_oxy_get_policy(#[string] name: &str) -> Result<String, JsErrorBox> {
    match name {
        "mock" => Ok(MOCK_POLICY_JSON.to_string()),
        other => Err(JsErrorBox::generic(format!("unknown policy: {other}"))),
    }
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
