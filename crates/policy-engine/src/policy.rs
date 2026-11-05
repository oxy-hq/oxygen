//! The values that cross the boundary: the policy the script reads, the
//! decision it writes back, and where written decisions are kept.

use serde::{Deserialize, Serialize};

/// The policy `PolicySource::Mock` serves as `"mock"` — the stand-in for a
/// database read when no database is configured.
pub(crate) const MOCK_POLICY_JSON: &str = r#"{
  "id": "mock",
  "variants": [
    { "name": "control", "weight": 50, "priceMultiplier": 1.0 },
    { "name": "discount_10", "weight": 30, "priceMultiplier": 0.9 },
    { "name": "premium_5", "weight": 20, "priceMultiplier": 1.05 }
  ],
  "bounds": { "minMultiplier": 0.8, "maxMultiplier": 1.2 }
}"#;

/// A pricing policy, as the script receives it. Every policy is parsed into
/// this on the host before it crosses, whichever store it came from, so the
/// script sees one shape and a row with a stray or missing field is refused at
/// the read rather than surfacing as `undefined` in the script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub id: String,
    pub variants: Vec<Variant>,
    pub bounds: Bounds,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Variant {
    pub name: String,
    pub weight: u32,
    pub price_multiplier: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bounds {
    pub min_multiplier: f64,
    pub max_multiplier: f64,
}

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
