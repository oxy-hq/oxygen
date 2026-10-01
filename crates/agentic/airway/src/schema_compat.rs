//! Is a branch's `.airway.yml` change safe to merge onto the live tables?
//!
//! Airway's schema migration is **additive only**: a new table or column
//! evolves in on the next load, but a renamed pipeline, a moved destination, a
//! changed write disposition or key, a renamed table or a retyped column does
//! not — production then needs an explicit **Reset schema**, usually followed
//! by a backfill. Workspace previews run this check on every changed pipeline
//! of a branch so that is known before the merge, not after the first failed
//! load.
//!
//! [`classify`] is pure. The host loads everything it needs — both specs,
//! both connectors' `resources()` / `table_name_mappings()` / `column_hints()`
//! (built offline with [`crate::placeholder::substitute_secret_vars`]), the
//! production stored schema, and the live columns from Airhouse — and hands
//! them in as [`PipelineInputs`].
//!
//! **Only what the branch changed is reported.** Where the live spec and the
//! stored schema already disagree on main, production is running with that
//! today and merging the branch does not change it, so it is not the branch's
//! finding. The one exception is [`Finding::LiveDrift`], which is about the
//! live tables themselves and is reported whenever the pipeline changed.

mod compare;
mod finding;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::config::{AirwayPipelineSpec, DestinationSpec};
use airway::connector::ResourceInfo;
use airway::schema::Table;
use airway::types::WriteDisposition;

pub use compare::compare_schemas;
pub use finding::{Finding, Verdict};

/// Airway's stored-schema artifact and hint types, re-exported so a host can
/// name them without depending on the engine (this crate is the boundary every
/// other oxy crate enters airway through).
pub use airway::Schema;
pub use airway::types::{ColumnHints, DataType};

/// `SourceConnector::column_hints()`: resource name → column name → hints.
pub type ColumnHintsByResource = HashMap<String, HashMap<String, ColumnHints>>;

/// One row of Airhouse's `information_schema.columns` for the live dataset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiveColumn {
    pub table: String,
    pub column: String,
    pub data_type: String,
    pub nullable: bool,
}

/// Everything [`classify`] reads. `None` for a spec means the pipeline does
/// not exist on that side (added or removed by the branch).
pub struct PipelineInputs<'a> {
    pub live_spec: Option<&'a AirwayPipelineSpec>,
    pub branch_spec: Option<&'a AirwayPipelineSpec>,
    pub live_resources: &'a [ResourceInfo],
    pub branch_resources: &'a [ResourceInfo],
    pub live_mappings: &'a HashMap<String, String>,
    pub branch_mappings: &'a HashMap<String, String>,
    /// Declared column types. The live side is what tells "the branch changed
    /// this hint" apart from "the stored type already differs on main".
    pub live_hints: &'a ColumnHintsByResource,
    pub branch_hints: &'a ColumnHintsByResource,
    /// Production's stored schema (`airway_workspace_pipeline_state`), keyed by
    /// the **live** spec's name. `None` when the pipeline never loaded.
    pub stored_schema: Option<&'a Schema>,
    /// Columns Airhouse has under the live spec's `dataset_name`.
    pub live_columns: &'a [LiveColumn],
}

/// The check's answer for one pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PipelineCheck {
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
}

impl PipelineCheck {
    /// The verdict is the worst finding's; no findings is additive.
    pub fn from_findings(findings: Vec<Finding>) -> Self {
        let verdict = findings
            .iter()
            .map(Finding::verdict)
            .max()
            .unwrap_or(Verdict::Additive);
        Self { verdict, findings }
    }

    /// For a pipeline the host could not evaluate (a connector that would not
    /// build, a spec that would not parse). Unknown is a warning, never clean.
    pub fn unevaluated(reason: impl Into<String>) -> Self {
        Self::from_findings(vec![Finding::Unevaluated {
            reason: reason.into(),
        }])
    }
}

/// Classify one pipeline's change. See the module doc for what counts.
pub fn classify(inputs: &PipelineInputs<'_>) -> PipelineCheck {
    let findings = match (inputs.live_spec, inputs.branch_spec) {
        (None, None) => Vec::new(),
        (Some(live), None) => vec![Finding::PipelineRemoved {
            name: live.name.clone(),
        }],
        (None, Some(branch)) => selected(branch, inputs.branch_resources)
            .into_keys()
            .map(|r| Finding::ResourceAdded {
                resource: r.to_string(),
            })
            .collect(),
        (Some(live), Some(branch)) => changed_pipeline(inputs, live, branch),
    };
    PipelineCheck::from_findings(findings)
}

fn changed_pipeline(
    inputs: &PipelineInputs<'_>,
    live: &AirwayPipelineSpec,
    branch: &AirwayPipelineSpec,
) -> Vec<Finding> {
    let mut findings = spec_findings(live, branch);
    let branch_sel = selected(branch, inputs.branch_resources);
    resource_findings(inputs, live, &branch_sel, &mut findings);
    mapping_findings(inputs, &mut findings);
    hint_findings(inputs, &branch_sel, &mut findings);
    drift_findings(inputs, live, &mut findings);
    findings
}

/// Spec-level fields: identity, destination, source kind, and config that
/// changes no stored shape.
fn spec_findings(live: &AirwayPipelineSpec, branch: &AirwayPipelineSpec) -> Vec<Finding> {
    let mut findings = Vec::new();
    if live.name != branch.name {
        findings.push(Finding::PipelineRenamed {
            from: live.name.clone(),
            to: branch.name.clone(),
        });
    }
    let (from, to) = (destination_label(live), destination_label(branch));
    if from != to {
        findings.push(Finding::DestinationMoved { from, to });
    }
    let (from, to) = (separator(live), separator(branch));
    if from != to {
        findings.push(Finding::SchemaSeparatorChanged { from, to });
    }
    if live.source.kind != branch.source.kind {
        findings.push(Finding::SourceKindChanged {
            from: live.source.kind.clone(),
            to: branch.source.kind.clone(),
        });
    } else if live.source.config != branch.source.config {
        findings.push(Finding::ConfigOnly {
            field: "source.config",
        });
    }
    let config_only = [
        ("description", live.description != branch.description),
        ("concurrency", live.concurrency != branch.concurrency),
        ("streaming", live.streaming != branch.streaming),
        (
            "channel_capacity",
            live.channel_capacity != branch.channel_capacity,
        ),
        (
            "allow_concurrent_runs",
            live.allow_concurrent_runs != branch.allow_concurrent_runs,
        ),
    ];
    for (field, changed) in config_only {
        if changed {
            findings.push(Finding::ConfigOnly { field });
        }
    }
    findings
}

/// Added, removed and narrowed resources, and — for resources both sides
/// extract — a changed write disposition or key.
fn resource_findings(
    inputs: &PipelineInputs<'_>,
    live: &AirwayPipelineSpec,
    branch_sel: &BTreeMap<&str, &ResourceInfo>,
    findings: &mut Vec<Finding>,
) {
    let live_sel = selected(live, inputs.live_resources);
    let branch_advertises: BTreeSet<&str> = inputs
        .branch_resources
        .iter()
        .map(|r| r.name.as_str())
        .collect();

    let mut narrowed = false;
    for name in live_sel.keys().filter(|n| !branch_sel.contains_key(*n)) {
        if branch_advertises.contains(name) {
            narrowed = true;
        } else {
            findings.push(Finding::ResourceRemoved {
                resource: name.to_string(),
            });
        }
    }
    if narrowed {
        findings.push(Finding::ConfigOnly { field: "resources" });
    }

    for (name, branch_res) in branch_sel {
        let live_res = live_sel.get(name).copied();
        if live_res.is_none() {
            findings.push(Finding::ResourceAdded {
                resource: name.to_string(),
            });
        }
        let stored = stored_table(inputs, name);
        disposition_finding(name, live_res, branch_res, stored, findings);
        if let Some(live_res) = live_res {
            key_finding(name, live_res, branch_res, findings);
        }
    }
}

/// The disposition production's table has is the stored one when it exists
/// (it is what airway's evolution compares against), otherwise the live
/// resource's. A branch that leaves the resource's disposition alone changes
/// nothing, whatever the stored table says.
fn disposition_finding(
    name: &str,
    live: Option<&ResourceInfo>,
    branch: &ResourceInfo,
    stored: Option<&Table>,
    findings: &mut Vec<Finding>,
) {
    if live.is_some_and(|l| l.write_disposition == branch.write_disposition) {
        return;
    }
    let from = stored
        .map(|t| &t.write_disposition)
        .or(live.map(|l| &l.write_disposition));
    if let Some(from) = from.filter(|from| **from != branch.write_disposition) {
        findings.push(Finding::WriteDispositionChanged {
            resource: name.to_string(),
            from: disposition_label(from),
            to: disposition_label(&branch.write_disposition),
        });
    }
}

/// A key change matters only where rows collapse on it (`merge`,
/// `replacing`); an `append` table's key is advisory.
fn key_finding(
    name: &str,
    live: &ResourceInfo,
    branch: &ResourceInfo,
    findings: &mut Vec<Finding>,
) {
    let keyed = key_sensitive(&live.write_disposition) || key_sensitive(&branch.write_disposition);
    let (from, to) = (
        normalized_key(live.primary_key.clone()),
        normalized_key(branch.primary_key.clone()),
    );
    if keyed && from != to {
        findings.push(Finding::PrimaryKeyChanged {
            resource: name.to_string(),
            from,
            to,
        });
    }
}

/// `table_name_mappings` are keyed by normalizer output name; an unmapped
/// name passes through, so a mapping added or removed renames the table too.
fn mapping_findings(inputs: &PipelineInputs<'_>, findings: &mut Vec<Finding>) {
    let keys: BTreeSet<&String> = inputs
        .live_mappings
        .keys()
        .chain(inputs.branch_mappings.keys())
        .collect();
    for key in keys {
        let from = inputs.live_mappings.get(key).unwrap_or(key);
        let to = inputs.branch_mappings.get(key).unwrap_or(key);
        if from != to {
            findings.push(Finding::TableRenamed {
                resource: key.clone(),
                from: from.clone(),
                to: to.clone(),
            });
        }
    }
}

/// A declared column type the branch changed, against the type production
/// has: the stored column's when it exists, else the live hint. A hint for a
/// column production does not have yet is an addition, not a finding.
fn hint_findings(
    inputs: &PipelineInputs<'_>,
    branch_sel: &BTreeMap<&str, &ResourceInfo>,
    findings: &mut Vec<Finding>,
) {
    for resource in branch_sel.keys() {
        let Some(columns) = inputs.branch_hints.get(*resource) else {
            continue;
        };
        let stored = stored_table(inputs, resource);
        let sorted: BTreeMap<&String, &ColumnHints> = columns.iter().collect();
        for (column, hint) in sorted {
            let Some(candidate) = &hint.data_type else {
                continue;
            };
            let live_hint = inputs
                .live_hints
                .get(*resource)
                .and_then(|c| c.get(column))
                .and_then(|h| h.data_type.as_ref());
            if live_hint == Some(candidate) {
                continue;
            }
            let live_type = stored
                .and_then(|t| t.columns.get(column))
                .map(|c| &c.data_type)
                .or(live_hint);
            if let Some(live_type) = live_type.filter(|t| *t != candidate) {
                findings.push(Finding::ColumnTypeChanged {
                    table: stored.map_or_else(|| resource.to_string(), |t| t.name.clone()),
                    column: column.clone(),
                    live: live_type.to_string(),
                    candidate: candidate.to_string(),
                });
            }
        }
    }
}

/// Tables the stored schema says production loaded that Airhouse does not
/// have under the live dataset. With a `schema_separator`, a flattened
/// `<schema><sep><table>` lands in `<schema>`, not the dataset, so those names
/// are not looked for there.
fn drift_findings(
    inputs: &PipelineInputs<'_>,
    live: &AirwayPipelineSpec,
    findings: &mut Vec<Finding>,
) {
    let Some(stored) = inputs.stored_schema else {
        return;
    };
    let present: BTreeSet<&str> = inputs
        .live_columns
        .iter()
        .map(|c| c.table.as_str())
        .collect();
    let sep = separator(live);
    let tables: BTreeSet<&String> = stored.tables.keys().collect();
    for table in tables {
        let elsewhere = sep.as_deref().is_some_and(|s| table.contains(s));
        if !elsewhere && !present.contains(table.as_str()) {
            findings.push(Finding::LiveDrift {
                table: table.clone(),
            });
        }
    }
}

/// The resources a spec extracts: every advertised one, or those its
/// `resources:` list names. Sorted by name for a stable report.
fn selected<'r>(
    spec: &AirwayPipelineSpec,
    advertised: &'r [ResourceInfo],
) -> BTreeMap<&'r str, &'r ResourceInfo> {
    advertised
        .iter()
        .filter(|r| spec.resources.is_empty() || spec.resources.contains(&r.name))
        .map(|r| (r.name.as_str(), r))
        .collect()
}

/// Production's stored table for a resource: its mapped name first, then the
/// resource name itself.
fn stored_table<'s>(inputs: &PipelineInputs<'s>, resource: &str) -> Option<&'s Table> {
    let stored = inputs.stored_schema?;
    let mapped = inputs
        .live_mappings
        .get(resource)
        .or_else(|| inputs.branch_mappings.get(resource));
    mapped
        .and_then(|m| stored.tables.get(m))
        .or_else(|| stored.tables.get(resource))
}

fn destination_label(spec: &AirwayPipelineSpec) -> String {
    match &spec.destination {
        DestinationSpec::Reference(r) => format!("{}.{}", r.database, r.dataset_name),
        DestinationSpec::Inline(c) => {
            let dataset = c.config.get("dataset_name").and_then(|v| v.as_str());
            format!("{}.{}", c.kind, dataset.unwrap_or_default())
        }
    }
}

fn separator(spec: &AirwayPipelineSpec) -> Option<String> {
    match &spec.destination {
        DestinationSpec::Reference(r) => r.schema_separator.clone(),
        DestinationSpec::Inline(_) => None,
    }
}

/// The YAML spelling (`replace`, `merge`, `replacing`, …).
fn disposition_label(disposition: &WriteDisposition) -> String {
    serde_json::to_value(disposition)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{disposition:?}").to_lowercase())
}

fn key_sensitive(disposition: &WriteDisposition) -> bool {
    matches!(
        disposition,
        WriteDisposition::Merge | WriteDisposition::Replacing
    )
}

/// Order-insensitive, and an empty key is no key.
fn normalized_key(key: Option<Vec<String>>) -> Option<Vec<String>> {
    key.filter(|k| !k.is_empty()).map(|mut k| {
        k.sort();
        k
    })
}
