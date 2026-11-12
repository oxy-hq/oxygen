//! Oxy's wire projection of airway's [`SourceContract`].
//!
//! airway 0.1.23 gave every source resource a contract describing how it
//! behaves — whether rows are ever corrected, whether a version orders those
//! corrections, and how far back a pull has to reach to catch them. The run
//! UI needs that per resource, so the projection rides on the
//! [`crate::events::AirwayEvent::PipelinePlan`] payload.
//!
//! Defined here rather than re-exporting airway's struct for the reason
//! `SchemaEvolved::changes` is carried as JSON: this is a **serialization
//! contract** oxy owns and persists in `agentic_run_events`, and it must not
//! drift because the engine reshaped a private field.
//!
//! ## The undeclared case is the whole point
//!
//! Read the declared map with [`SourceConnector::contracts`], never
//! [`SourceConnector::contract_for`]. `contract_for` falls back to
//! `SourceContract::default()`, which is `opaque` — so a resource nobody has
//! described would arrive at the UI wearing a *checked vendor fact*. Those are
//! different states to an operator: `opaque` means "the vendor exposes no
//! version", `undeclared` means "nobody has said", and only the second is what
//! `require_declared` refuses. `crates/api-airway/src/admin/preview.rs`
//! carries the same distinction for the admin policy preview; the two agree by
//! construction because both are driven off `contracts()`.
//!
//! [`SourceConnector::contracts`]: airway::connector::SourceConnector::contracts
//! [`SourceConnector::contract_for`]: airway::connector::SourceConnector::contract_for

use std::collections::HashMap;
use std::time::Duration;

use airway::connector::{Mutability, SourceContract};
use serde::{Deserialize, Serialize};

/// How a resource's rows behave, as the UI presents it.
///
/// Four states, not three: [`Self::Undeclared`] is deliberately **not**
/// [`Self::Opaque`] — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractMutability {
    /// Immutable once published. Rows are never corrected.
    Immutable,
    /// Mutable, carrying a monotonic version corrections can be ordered on.
    Versioned,
    /// Mutable with no version signal — only a whole re-pull detects a change.
    Opaque,
    /// The connector declared no contract for this resource.
    Undeclared,
}

/// One resource's contract, flattened for the wire.
///
/// Every field except `resource` and `mutability` is `None` when
/// `mutability == Undeclared`: there is nothing to report, and reporting
/// airway's defaults (opaque, no windows, zero lag) would let "unknown" read
/// as a positive statement. For a *declared* contract, `None` is a real fact —
/// `restatement_window_ms: None` means the contract declares no restatement
/// window, so `mutability` is what disambiguates the two meanings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceContract {
    /// The source resource this describes. Matches an entry of
    /// `PipelinePlan::resources`.
    pub resource: String,
    pub mutability: ContractMutability,
    /// `Mutability::Versioned`'s source-side version field — the API path the
    /// vendor exposes. Distinct from [`Self::version_column`].
    pub version_field: Option<String>,
    /// The **landed** column carrying that version, when the contract names
    /// one. This is what the destination's version-guarded write compares on.
    pub version_column: Option<String>,
    /// Does the resource's cursor move when a row is corrected? `false` means
    /// a cursor window cannot see late edits at all.
    pub cursor_tracks_modification: Option<bool>,
    /// How far back of the cursor corrections are still expected.
    pub restatement_window_ms: Option<u64>,
    /// Source-side visibility lag on the cursor itself.
    pub cursor_lag_ms: Option<u64>,
    /// `cursor_lag + restatement_window` — what a pull actually rewinds by.
    pub rewind_ms: Option<u64>,
    /// Must corrections be caught by re-pulling whole partitions rather than
    /// by a cursor window? True whenever nothing orders the corrections.
    pub requires_partition_repull: Option<bool>,
}

impl ResourceContract {
    /// Project a declared [`SourceContract`] onto the wire shape.
    fn declared(resource: &str, contract: &SourceContract) -> Self {
        let (mutability, version_field) = match contract.mutability() {
            Mutability::Immutable => (ContractMutability::Immutable, None),
            Mutability::Versioned { version_field } => (
                ContractMutability::Versioned,
                Some(version_field.to_string()),
            ),
            Mutability::Opaque => (ContractMutability::Opaque, None),
        };
        Self {
            resource: resource.to_string(),
            mutability,
            version_field,
            version_column: contract.version_column().map(str::to_string),
            cursor_tracks_modification: Some(contract.cursor_tracks_modification()),
            // Milliseconds, not seconds: a sub-second `cursor_lag` truncated
            // to `0s` would render as "no lag", which is a statement the
            // contract never made. u64 ms covers any window a source could
            // plausibly declare.
            restatement_window_ms: contract.restatement_window().map(duration_ms),
            cursor_lag_ms: Some(duration_ms(contract.cursor_lag())),
            rewind_ms: Some(duration_ms(contract.rewind())),
            requires_partition_repull: Some(contract.requires_partition_repull()),
        }
    }

    /// The connector said nothing about this resource. Everything stays
    /// `None`; see the struct docs for why no default is substituted.
    fn undeclared(resource: &str) -> Self {
        Self {
            resource: resource.to_string(),
            mutability: ContractMutability::Undeclared,
            version_field: None,
            version_column: None,
            cursor_tracks_modification: None,
            restatement_window_ms: None,
            cursor_lag_ms: None,
            rewind_ms: None,
            requires_partition_repull: None,
        }
    }
}

/// `Duration` → whole milliseconds, saturating.
fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Project `declared` onto the resources a run actually plans.
///
/// One entry per planned resource, in plan order, so a consumer never has to
/// decide what a missing key means — a resource with no declaration is present
/// and labelled [`ContractMutability::Undeclared`].
///
/// `declared` is the connector's `contracts()` map. Resources it names that the
/// run does not plan (a narrowed `resources:` list, or an orphaned declaration)
/// are simply absent from the output: the run UI shows the run's resources, and
/// the admin policy preview is where an orphan is a finding.
pub fn project_contracts(
    resources: &[String],
    declared: &HashMap<String, SourceContract>,
) -> Vec<ResourceContract> {
    resources
        .iter()
        .map(|name| match declared.get(name) {
            Some(contract) => ResourceContract::declared(name, contract),
            None => ResourceContract::undeclared(name),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn undeclared_resource_is_labelled_not_defaulted() {
        let out = project_contracts(&names(&["orders"]), &HashMap::new());
        assert_eq!(out.len(), 1);
        let c = &out[0];
        assert_eq!(c.resource, "orders");
        assert_eq!(
            c.mutability,
            ContractMutability::Undeclared,
            "an undeclared resource must not inherit `SourceContract::default()` (= opaque)"
        );
        // Nothing may be asserted on its behalf.
        assert!(c.version_field.is_none());
        assert!(c.version_column.is_none());
        assert!(c.cursor_tracks_modification.is_none());
        assert!(c.restatement_window_ms.is_none());
        assert!(c.cursor_lag_ms.is_none());
        assert!(c.rewind_ms.is_none());
        assert!(c.requires_partition_repull.is_none());
    }

    #[test]
    fn declared_opaque_is_distinct_from_undeclared() {
        let declared = HashMap::from([("things".to_string(), SourceContract::opaque())]);
        let out = project_contracts(&names(&["things", "other"]), &declared);
        assert_eq!(out[0].mutability, ContractMutability::Opaque);
        assert_eq!(
            out[0].cursor_tracks_modification,
            Some(false),
            "a declared opaque contract states its flags; only `undeclared` withholds them"
        );
        assert_eq!(out[0].requires_partition_repull, Some(true));
        assert_eq!(out[1].mutability, ContractMutability::Undeclared);
    }

    #[test]
    fn immutable_projects_with_no_windows() {
        let declared = HashMap::from([("events".to_string(), SourceContract::immutable())]);
        let c = &project_contracts(&names(&["events"]), &declared)[0];
        assert_eq!(c.mutability, ContractMutability::Immutable);
        assert_eq!(c.cursor_tracks_modification, Some(true));
        assert_eq!(c.restatement_window_ms, None);
        assert_eq!(c.cursor_lag_ms, Some(0));
        assert_eq!(c.rewind_ms, Some(0));
        assert_eq!(c.requires_partition_repull, Some(false));
    }

    #[test]
    fn versioned_carries_both_version_names_and_the_summed_rewind() {
        let contract = SourceContract::versioned("modifiedDate")
            .tracking_modification(true)
            .expect("versioned may track modification")
            .restating_within(3 * DAY)
            .expect("a tracking cursor may carry a window")
            .lagging_by(Duration::from_secs(30))
            .landing_version_as("modified_date")
            .expect("versioned may name a landing column");
        let declared = HashMap::from([("orders".to_string(), contract)]);

        let c = &project_contracts(&names(&["orders"]), &declared)[0];
        assert_eq!(c.mutability, ContractMutability::Versioned);
        assert_eq!(c.version_field.as_deref(), Some("modifiedDate"));
        assert_eq!(c.version_column.as_deref(), Some("modified_date"));
        assert_eq!(c.cursor_tracks_modification, Some(true));
        assert_eq!(c.restatement_window_ms, Some(3 * 86_400_000));
        assert_eq!(c.cursor_lag_ms, Some(30_000));
        // rewind = cursor_lag + restatement_window, the two summed.
        assert_eq!(c.rewind_ms, Some(3 * 86_400_000 + 30_000));
        assert_eq!(c.requires_partition_repull, Some(false));
    }

    #[test]
    fn sub_second_lag_survives_as_milliseconds() {
        // `as_secs()` would report 0 here, which reads as "declares no lag".
        let contract = SourceContract::opaque().lagging_by(Duration::from_millis(500));
        let declared = HashMap::from([("things".to_string(), contract)]);
        let c = &project_contracts(&names(&["things"]), &declared)[0];
        assert_eq!(c.cursor_lag_ms, Some(500));
        assert_eq!(c.rewind_ms, Some(500));
    }

    #[test]
    fn plan_order_is_preserved_and_unplanned_declarations_are_dropped() {
        let declared = HashMap::from([
            ("orders".to_string(), SourceContract::immutable()),
            ("not_planned".to_string(), SourceContract::opaque()),
        ]);
        let out = project_contracts(&names(&["users", "orders"]), &declared);
        assert_eq!(
            out.iter().map(|c| c.resource.as_str()).collect::<Vec<_>>(),
            vec!["users", "orders"]
        );
    }

    #[test]
    fn mutability_serializes_snake_case() {
        let out = project_contracts(&names(&["a"]), &HashMap::new());
        let v = serde_json::to_value(&out[0]).expect("serialize");
        assert_eq!(v["mutability"], serde_json::json!("undeclared"));
        assert_eq!(v["resource"], serde_json::json!("a"));
        // Absent facts are explicit nulls on the wire, not missing keys —
        // the consumer must be able to tell them from a stale payload shape.
        assert!(v.get("rewind_ms").is_some());
        assert_eq!(v["rewind_ms"], serde_json::Value::Null);

        let versioned = ResourceContract::declared(
            "orders",
            &SourceContract::versioned("modifiedDate")
                .landing_version_as("modified_date")
                .expect("versioned may name a landing column"),
        );
        let v = serde_json::to_value(&versioned).expect("serialize");
        assert_eq!(v["mutability"], serde_json::json!("versioned"));
        assert_eq!(v["version_field"], serde_json::json!("modifiedDate"));
        assert_eq!(v["version_column"], serde_json::json!("modified_date"));
    }

    #[test]
    fn round_trips_through_json() {
        let out = project_contracts(
            &names(&["orders", "misc"]),
            &HashMap::from([("orders".to_string(), SourceContract::immutable())]),
        );
        let json = serde_json::to_value(&out).expect("serialize");
        let back: Vec<ResourceContract> = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, out);
    }
}
