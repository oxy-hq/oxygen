//! The CPU half of the change check: parse each side's compiled definition,
//! build its source connector offline, read what it advertises, and classify.
//!
//! No I/O of any kind, so [`prepare_all`] runs under `spawn_blocking`, as
//! `admin::airway_config::preview_scan` does. Connectors are built with
//! placeholder credentials (`agentic_airway::placeholder`): construction issues
//! no request, and this module only reads `resources()`,
//! `table_name_mappings()` and `column_hints()`.

use std::collections::HashMap;

use agentic_airway::placeholder::substitute_secret_vars;
use agentic_airway::schema_compat::{
    self, ColumnHintsByResource, Finding, LiveColumn, PipelineCheck, PipelineInputs, Schema,
};
use agentic_airway::{
    AirwayPipelineSpec, Environment, ResourceInfo, SourceConnector, build_source_connector,
};

use super::changes::{Change, ChangedPipeline};

/// One side of a changed pipeline, read offline.
pub(super) struct Side {
    pub spec: AirwayPipelineSpec,
    resources: Vec<ResourceInfo>,
    mappings: HashMap<String, String>,
    hints: ColumnHintsByResource,
}

pub(super) enum Prepared {
    /// `None` for the side the branch added or removed.
    Ready {
        live: Option<Box<Side>>,
        branch: Option<Box<Side>>,
    },
    /// A definition that will not parse or a connector that will not build:
    /// the change cannot be judged, which is a warning, never a clean bill.
    Unevaluated(String),
}

impl Prepared {
    /// The promoted side's spec, when there is one to compare against.
    pub fn live_spec(&self) -> Option<&AirwayPipelineSpec> {
        match self {
            Prepared::Ready { live, .. } => live.as_ref().map(|s| &s.spec),
            Prepared::Unevaluated(_) => None,
        }
    }
}

/// What the live tables said, for the drift half of the check.
pub(super) enum LiveRead {
    Columns(Vec<LiveColumn>),
    /// Not read: nothing to check drift against (no stored schema, not an
    /// edit), or a destination that is not the workspace's managed Airhouse.
    NotRead,
    /// The destination is the workspace's Airhouse and it could not be read.
    Unavailable(String),
}

/// [`prepare`] for every changed pipeline, off the async threads.
pub(super) async fn prepare_all(changed: Vec<ChangedPipeline>) -> Vec<Prepared> {
    let count = changed.len();
    tokio::task::spawn_blocking(move || changed.iter().map(prepare).collect())
        .await
        .unwrap_or_else(|e| {
            let reason = format!("the offline connector build panicked: {e}");
            (0..count)
                .map(|_| Prepared::Unevaluated(reason.clone()))
                .collect()
        })
}

/// Read both sides of one change. A removed pipeline's live side is only
/// parsed: `PipelineRemoved` needs its name, not what its source advertises.
pub(super) fn prepare(changed: &ChangedPipeline) -> Prepared {
    let build_live = changed.change() == Change::Modified;
    let live = match changed.live_def.as_ref() {
        Some(def) => match side("live", def, build_live) {
            Ok(s) => Some(Box::new(s)),
            Err(reason) => return Prepared::Unevaluated(reason),
        },
        None => None,
    };
    let branch = match changed.branch_def.as_ref() {
        Some(def) => match side("branch", def, true) {
            Ok(s) => Some(Box::new(s)),
            Err(reason) => return Prepared::Unevaluated(reason),
        },
        None => None,
    };
    Prepared::Ready { live, branch }
}

fn side(label: &str, definition: &serde_json::Value, build: bool) -> Result<Side, String> {
    let spec: AirwayPipelineSpec = serde_json::from_value(definition.clone())
        .map_err(|e| format!("the {label} definition is not an airway pipeline: {e}"))?;
    spec.validate()
        .map_err(|e| format!("the {label} definition is not valid: {e}"))?;
    if !build {
        return Ok(Side {
            spec,
            resources: Vec::new(),
            mappings: HashMap::new(),
            hints: HashMap::new(),
        });
    }
    let connector = offline_connector(&spec)
        .map_err(|e| format!("the {label} source connector could not be built: {e}"))?;
    Ok(Side {
        resources: connector.resources(),
        mappings: connector.table_name_mappings(),
        hints: connector.column_hints(),
        spec,
    })
}

/// The connector a run would build, with placeholder credentials. Mirrors
/// `preview_scan::evaluate_pipeline`, including the read-only QuickBooks
/// custody stub that can never be asked for a token.
pub(crate) fn offline_connector(
    spec: &AirwayPipelineSpec,
) -> Result<Box<dyn SourceConnector>, String> {
    let mut source = spec.source.clone();
    let read_only = source
        .config
        .get("access_token_var")
        .and_then(|v| v.as_str())
        .is_some();
    substitute_secret_vars(&mut source.config);
    let tokens = read_only.then(|| {
        agentic_airway::QuickBooksTokens::ReadOnly(std::sync::Arc::new(
            crate::server::api::admin::airway_config::preview_scan::PlaceholderAccessToken,
        ))
    });
    build_source_connector(&source, tokens, Environment::Production).map_err(|e| e.to_string())
}

/// Classify one change. Drift is judged only against columns actually read:
/// when the live tables were not read its findings are dropped, and when they
/// could not be read that is said as an `Unevaluated` finding.
pub(super) fn check(
    prepared: &Prepared,
    stored: Option<&Schema>,
    live: &LiveRead,
) -> PipelineCheck {
    let (live_side, branch_side) = match prepared {
        Prepared::Unevaluated(reason) => return PipelineCheck::unevaluated(reason.clone()),
        Prepared::Ready { live, branch } => (live.as_ref(), branch.as_ref()),
    };
    let no_mappings = HashMap::new();
    let no_hints = HashMap::new();
    let columns: &[LiveColumn] = match live {
        LiveRead::Columns(c) => c,
        _ => &[],
    };
    let check = schema_compat::classify(&PipelineInputs {
        live_spec: live_side.map(|s| &s.spec),
        branch_spec: branch_side.map(|s| &s.spec),
        live_resources: live_side.map_or(&[][..], |s| &s.resources),
        branch_resources: branch_side.map_or(&[][..], |s| &s.resources),
        live_mappings: live_side.map_or(&no_mappings, |s| &s.mappings),
        branch_mappings: branch_side.map_or(&no_mappings, |s| &s.mappings),
        live_hints: live_side.map_or(&no_hints, |s| &s.hints),
        branch_hints: branch_side.map_or(&no_hints, |s| &s.hints),
        stored_schema: stored,
        live_columns: columns,
    });
    match live {
        LiveRead::Columns(_) => check,
        LiveRead::NotRead => without_drift(check, None),
        LiveRead::Unavailable(reason) => without_drift(check, Some(reason)),
    }
}

fn without_drift(check: PipelineCheck, unavailable: Option<&str>) -> PipelineCheck {
    let mut findings: Vec<Finding> = check
        .findings
        .into_iter()
        .filter(|f| !matches!(f, Finding::LiveDrift { .. }))
        .collect();
    if let Some(reason) = unavailable {
        findings.push(Finding::Unevaluated {
            reason: format!(
                "the live tables could not be read, so drift was not checked: {reason}"
            ),
        });
    }
    PipelineCheck::from_findings(findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_airway::schema_compat::Verdict;
    use serde_json::json;

    /// An edit that changes only the description, over a stored schema that
    /// lists a table: whether that table is drift depends on what was read.
    fn edited() -> Prepared {
        let def = |description: &str| {
            json!({
                "name": "nces", "description": description,
                "source": { "kind": "rest_api", "config": {
                    "base_url": "https://example.test",
                    "endpoints": [{ "name": "schools", "path": "/s", "write_disposition": "replace" }],
                } },
                "destination": { "database": "airhouse", "dataset_name": "nces" },
            })
        };
        prepare(&ChangedPipeline {
            name: "nces".into(),
            file_path: "airway/nces.airway.yml".into(),
            live_def: Some(def("old")),
            branch_def: Some(def("new")),
        })
    }

    fn stored() -> Schema {
        serde_json::from_value(json!({
            "name": "nces", "version": 1, "version_hash": "", "engine_version": 1,
            "tables": { "schools": { "name": "schools", "columns": {}, "write_disposition": "replace" } },
        }))
        .unwrap()
    }

    fn kinds(check: &PipelineCheck) -> Vec<String> {
        check
            .findings
            .iter()
            .map(|f| crate::server::previews::analyze::FindingView::from(f).kind)
            .collect()
    }

    #[test]
    fn drift_is_judged_only_against_columns_actually_read() {
        let p = edited();
        let s = stored();
        let read = check(&p, Some(&s), &LiveRead::Columns(Vec::new()));
        assert_eq!(read.verdict, Verdict::Warning);
        assert!(kinds(&read).contains(&"LiveDrift".to_string()), "{read:?}");

        let not_read = check(&p, Some(&s), &LiveRead::NotRead);
        assert_eq!(not_read.verdict, Verdict::Additive, "{not_read:?}");
        assert!(!kinds(&not_read).contains(&"LiveDrift".to_string()));

        let unavailable = check(&p, Some(&s), &LiveRead::Unavailable("down".into()));
        assert_eq!(unavailable.verdict, Verdict::Warning);
        assert_eq!(kinds(&unavailable), vec!["ConfigOnly", "Unevaluated"]);
    }

    #[test]
    fn a_source_that_will_not_build_is_unevaluated_not_clean() {
        let p = prepare(&ChangedPipeline {
            name: "mystery".into(),
            file_path: "airway/mystery.airway.yml".into(),
            live_def: None,
            branch_def: Some(json!({
                "name": "mystery",
                "source": { "kind": "no_such_source", "config": {} },
                "destination": { "database": "airhouse", "dataset_name": "m" },
            })),
        });
        let c = check(&p, None, &LiveRead::NotRead);
        assert_eq!(c.verdict, Verdict::Warning);
        assert_eq!(kinds(&c), vec!["Unevaluated"]);
    }
}
