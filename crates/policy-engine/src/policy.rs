//! The values that cross the boundary: the policy the script reads, the
//! decision it writes back, and where written decisions are kept.

use serde::{Deserialize, Serialize};

/// What `op_oxy_get_policy("mock")` returns — the stand-in for a database
/// read. A literal on purpose: Phase 1 proves the boundary, not the store.
pub(crate) const MOCK_POLICY_JSON: &str = r#"{
  "id": "mock",
  "variants": [
    { "name": "control", "weight": 50, "priceMultiplier": 1.0 },
    { "name": "discount_10", "weight": 30, "priceMultiplier": 0.9 },
    { "name": "premium_5", "weight": 20, "priceMultiplier": 1.05 }
  ],
  "bounds": { "minMultiplier": 0.8, "maxMultiplier": 1.2 }
}"#;

/// One routing decision, as the script reports it through
/// `op_oxy_write_decision` — the stand-in for a `policy_decisions` row.
///
/// `deny_unknown_fields`: the write is an audit record, so a field the host
/// does not know is a script bug to refuse, not data to drop silently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyDecision {
    pub policy_id: String,
    pub sku: String,
    pub variant: String,
    pub price_multiplier: f64,
}

/// Every decision written through the audit op, in write order. Lives in the
/// isolate's op state, so it belongs to one [`crate::PolicyEngine`].
#[derive(Debug, Default)]
pub struct AuditLog(pub Vec<PolicyDecision>);
