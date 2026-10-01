//! Helpers for constructing and decoding the `TaskSpec::Airway` variant.
//!
//! The variant itself lives in `agentic-core::delegation` so the
//! runtime queue can serialize/deserialize it without taking a dep on
//! this crate. These helpers exist so callers (pipeline facade, CLI,
//! tests) can build/parse airway tasks ergonomically without poking at
//! the enum directly.

use agentic_core::delegation::TaskSpec;
use serde_json::Value;

/// Kinds whose airway source reads `backfill_start` / `backfill_end`.
///
/// Lives here, in the crate every oxy crate enters airway through, because two
/// callers decide by it: the executor's backfill window
/// (`agentic_pipeline::executor`, which re-exports it under its old path) and a
/// workspace preview's sample policy ([`crate::preview`]). One list, so the two
/// cannot disagree about which sources take a window.
///
/// Adding a kind here is only half the job — the source builder in
/// [`crate::source_factory`] has to parse the pair and hand it to the
/// connector, or the window is accepted and silently ignored there.
pub const WINDOWED_BACKFILL_KINDS: [&str; 3] = ["toast", "quickbooks", "sp_api"];

/// Strongly-typed view of the data carried by a [`TaskSpec::Airway`].
#[derive(Debug, Clone)]
pub struct AirwayTaskSpec {
    pub pipeline_ref: String,
    pub variables: Option<Value>,
    /// Subset of resources to run (empty = whole spec). See
    /// [`TaskSpec::Airway::resources`].
    pub resources: Vec<String>,
    /// Bounded-backfill window `[from, to)` as RFC3339 strings, applied to the
    /// date-windowed sources ([`WINDOWED_BACKFILL_KINDS`] is the list that
    /// decides). `None` = normal run.
    pub backfill_from: Option<String>,
    pub backfill_to: Option<String>,
    /// Contract policy for this run; `None` = airway's default. See
    /// [`crate::AirwayAdmission`].
    pub contract_policy: Option<String>,
    /// Vendor environment for this run; `None` = airway's default.
    pub environment: Option<String>,
}

impl AirwayTaskSpec {
    /// Build a new airway task spec.
    pub fn new(pipeline_ref: impl Into<String>) -> Self {
        Self {
            pipeline_ref: pipeline_ref.into(),
            variables: None,
            resources: Vec::new(),
            backfill_from: None,
            backfill_to: None,
            contract_policy: None,
            environment: None,
        }
    }

    /// Attach variables that will be rendered into the pipeline YAML
    /// at run time.
    pub fn with_variables(mut self, variables: Value) -> Self {
        self.variables = Some(variables);
        self
    }

    /// Restrict the run to a subset of resources (e.g. retry failed tables).
    pub fn with_resources(mut self, resources: Vec<String>) -> Self {
        self.resources = resources;
        self
    }

    /// Attach the admission policies this run should be checked under.
    /// Either may be `None`, which takes airway's default.
    pub fn with_admission(
        mut self,
        contract_policy: Option<String>,
        environment: Option<String>,
    ) -> Self {
        self.contract_policy = contract_policy;
        self.environment = environment;
        self
    }

    /// Materialise as a runtime [`TaskSpec`] for the durable queue.
    pub fn into_task_spec(self) -> TaskSpec {
        TaskSpec::Airway {
            pipeline_ref: self.pipeline_ref,
            variables: self.variables,
            resources: self.resources,
            backfill_from: self.backfill_from,
            backfill_to: self.backfill_to,
            contract_policy: self.contract_policy,
            environment: self.environment,
        }
    }

    /// Inverse of [`AirwayTaskSpec::into_task_spec`]. Returns `None`
    /// when the spec is some other variant.
    pub fn from_task_spec(spec: &TaskSpec) -> Option<Self> {
        match spec {
            TaskSpec::Airway {
                pipeline_ref,
                variables,
                resources,
                backfill_from,
                backfill_to,
                contract_policy,
                environment,
            } => Some(Self {
                pipeline_ref: pipeline_ref.clone(),
                variables: variables.clone(),
                resources: resources.clone(),
                backfill_from: backfill_from.clone(),
                backfill_to: backfill_to.clone(),
                contract_policy: contract_policy.clone(),
                environment: environment.clone(),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_task_spec() {
        let spec = AirwayTaskSpec::new("pipelines/shopify.airway.yml")
            .with_variables(serde_json::json!({"since": "2026-01-01"}));
        let runtime_spec = spec.into_task_spec();

        let back = AirwayTaskSpec::from_task_spec(&runtime_spec).expect("airway variant");
        assert_eq!(back.pipeline_ref, "pipelines/shopify.airway.yml");
        assert_eq!(
            back.variables.as_ref().and_then(|v| v.get("since")),
            Some(&serde_json::json!("2026-01-01")),
        );
    }

    #[test]
    fn rejects_non_airway_variant() {
        let other = TaskSpec::Agent {
            agent_id: "x".to_string(),
            question: "q".to_string(),
            extra: None,
        };
        assert!(AirwayTaskSpec::from_task_spec(&other).is_none());
    }

    #[test]
    fn admission_strings_round_trip_through_task_spec() {
        let spec = AirwayTaskSpec::new("pipelines/toast_pos.airway.yml").with_admission(
            Some("require_declared".to_string()),
            Some("sandbox".to_string()),
        );
        let back = AirwayTaskSpec::from_task_spec(&spec.into_task_spec()).expect("airway variant");
        assert_eq!(back.contract_policy.as_deref(), Some("require_declared"));
        assert_eq!(back.environment.as_deref(), Some("sandbox"));
    }

    /// Rows queued before this change carry neither key. They must decode to
    /// `None` — which `AirwayAdmission::from_strings` reads as the permissive
    /// production default — rather than failing to deserialize and wedging
    /// every in-flight run at upgrade time.
    #[test]
    fn a_row_queued_without_the_new_keys_still_decodes() {
        let legacy = serde_json::json!({
            "type": "airway",
            "pipeline_ref": "pipelines/toast_pos.airway.yml"
        });
        let spec: TaskSpec = serde_json::from_value(legacy).expect("legacy row must decode");
        let airway = AirwayTaskSpec::from_task_spec(&spec).expect("airway variant");
        assert_eq!(airway.contract_policy, None);
        assert_eq!(airway.environment, None);
    }
}
