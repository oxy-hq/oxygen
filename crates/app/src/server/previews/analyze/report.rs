//! What the change check reports, in the shapes the checks API serves
//! (`phase2a-contract.md`, "Checks (S7)"). The report is also the analyze run's
//! `TaskOutcome::Done` metadata, so it holds ids, names and verdicts only —
//! never rows.

use agentic_airway::schema_compat::{Finding, PipelineCheck, Verdict};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::changes::ChangedPipeline;
use super::transforms::TransformReport;

/// The analyze run's result: every `.airway.yml` the branch changed against the
/// promoted revision, and what merging each would need.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzeReport {
    /// The staging revision checked.
    pub revision_id: Uuid,
    /// The promoted revision it was checked against, when the workspace had one.
    pub promoted_revision_id: Option<Uuid>,
    pub pipelines: Vec<PipelineReport>,
    /// The automations the branch changed, and whether each is built in the
    /// preview (`auto`) or left to be run by hand (`manual`). Absent from
    /// reports written before phase 2b.
    #[serde(default)]
    pub transforms: Vec<TransformReport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineReport {
    pub name: String,
    pub file_path: String,
    /// `added` | `modified` | `removed`.
    pub change: String,
    pub verdict: Verdict,
    pub findings: Vec<FindingView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingView {
    /// The finding's name, e.g. `WriteDispositionChanged`.
    pub kind: String,
    pub verdict: Verdict,
    /// One line naming what changed, e.g. `orders: append → merge`.
    pub detail: String,
    /// What production needs after merge; `None` for an additive finding.
    pub prod_action: Option<String>,
}

impl AnalyzeReport {
    /// Pipelines whose verdict is `needs_reset`, whose verdict is `warning`,
    /// and the branch's changed transforms — the badge counts on a preview's
    /// list item.
    pub fn counts(&self) -> (u32, u32, u32) {
        let count = |v: Verdict| self.pipelines.iter().filter(|p| p.verdict == v).count() as u32;
        (
            count(Verdict::NeedsReset),
            count(Verdict::Warning),
            self.transforms.len() as u32,
        )
    }

    /// The run's one-line answer.
    pub fn answer(&self) -> String {
        let (needs_reset, warnings, transforms) = self.counts();
        let pipelines = format!(
            "{} changed pipeline(s): {needs_reset} need Reset schema after merge, {warnings} with warnings",
            self.pipelines.len()
        );
        if transforms == 0 {
            return pipelines;
        }
        format!("{pipelines}, {transforms} changed transform(s)")
    }
}

impl PipelineReport {
    pub fn new(changed: &ChangedPipeline, check: PipelineCheck) -> Self {
        Self {
            name: changed.name.clone(),
            file_path: changed.file_path.clone(),
            change: changed.change().as_str().to_string(),
            verdict: check.verdict,
            findings: check.findings.iter().map(FindingView::from).collect(),
        }
    }
}

impl From<&Finding> for FindingView {
    fn from(f: &Finding) -> Self {
        let (kind, detail) = describe(f);
        Self {
            kind: kind.to_string(),
            verdict: f.verdict(),
            detail,
            prod_action: f.prod_action().map(str::to_string),
        }
    }
}

/// The finding's name and its one-line detail. Exhaustive on purpose: a new
/// finding must say how it reads before it can ship.
fn describe(f: &Finding) -> (&'static str, String) {
    match f {
        Finding::ResourceAdded { resource } => ("ResourceAdded", resource.clone()),
        Finding::TableAdded { table } => ("TableAdded", table.clone()),
        Finding::ColumnAdded { table, column } => ("ColumnAdded", format!("{table}.{column}")),
        Finding::ConfigOnly { field } => ("ConfigOnly", (*field).to_string()),
        Finding::PipelineRenamed { from, to } => ("PipelineRenamed", arrow(from, to)),
        Finding::DestinationMoved { from, to } => ("DestinationMoved", arrow(from, to)),
        Finding::SchemaSeparatorChanged { from, to } => (
            "SchemaSeparatorChanged",
            arrow(&or_none(from.as_deref()), &or_none(to.as_deref())),
        ),
        Finding::SourceKindChanged { from, to } => ("SourceKindChanged", arrow(from, to)),
        Finding::WriteDispositionChanged { resource, from, to } => (
            "WriteDispositionChanged",
            format!("{resource}: {}", arrow(from, to)),
        ),
        Finding::PrimaryKeyChanged { resource, from, to } => (
            "PrimaryKeyChanged",
            format!("{resource}: {}", arrow(&key(from), &key(to))),
        ),
        Finding::TableRenamed { resource, from, to } => {
            ("TableRenamed", format!("{resource}: {}", arrow(from, to)))
        }
        Finding::ColumnTypeChanged {
            table,
            column,
            live,
            candidate,
        } => (
            "ColumnTypeChanged",
            format!("{table}.{column}: {}", arrow(live, candidate)),
        ),
        Finding::ColumnRemoved { table, column } => ("ColumnRemoved", format!("{table}.{column}")),
        Finding::ResourceRemoved { resource } => ("ResourceRemoved", resource.clone()),
        Finding::PipelineRemoved { name } => ("PipelineRemoved", name.clone()),
        Finding::LiveDrift { table } => ("LiveDrift", table.clone()),
        Finding::Unevaluated { reason } => ("Unevaluated", reason.clone()),
    }
}

fn arrow(from: &str, to: &str) -> String {
    format!("{from} → {to}")
}

fn or_none(v: Option<&str>) -> String {
    v.map_or_else(|| "none".to_string(), |s| format!("\"{s}\""))
}

fn key(k: &Option<Vec<String>>) -> String {
    match k {
        Some(cols) if !cols.is_empty() => format!("({})", cols.join(", ")),
        _ => "none".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finding_reads_as_the_contract_shows_it() {
        let view = FindingView::from(&Finding::WriteDispositionChanged {
            resource: "orders".into(),
            from: "append".into(),
            to: "merge".into(),
        });
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            serde_json::json!({
                "kind": "WriteDispositionChanged",
                "verdict": "needs_reset",
                "detail": "orders: append → merge",
                "prod_action": "Reset schema, then backfill",
            })
        );
    }

    #[test]
    fn an_additive_finding_names_no_prod_action() {
        let view = FindingView::from(&Finding::ResourceAdded {
            resource: "refunds".into(),
        });
        assert_eq!(view.verdict, Verdict::Additive);
        assert_eq!(view.prod_action, None);
        assert_eq!(view.detail, "refunds");
    }

    #[test]
    fn a_changed_key_names_both_sides() {
        let view = FindingView::from(&Finding::PrimaryKeyChanged {
            resource: "schools".into(),
            from: None,
            to: Some(vec!["ncessch".into(), "year".into()]),
        });
        assert_eq!(view.detail, "schools: none → (ncessch, year)");
    }

    fn transform(name: &str) -> TransformReport {
        TransformReport {
            name: name.to_string(),
            file_path: format!("workflows/{name}.procedure.yml"),
            change: "modified".to_string(),
            build: "auto".to_string(),
            reason: None,
            build_run_id: None,
        }
    }

    fn report(transforms: Vec<TransformReport>) -> AnalyzeReport {
        AnalyzeReport {
            revision_id: Uuid::nil(),
            promoted_revision_id: None,
            pipelines: Vec::new(),
            transforms,
        }
    }

    #[test]
    fn counts_reports_changed_transforms_alongside_pipeline_verdicts() {
        let report = report(vec![transform("je"), transform("reconcile")]);
        assert_eq!(report.counts(), (0, 0, 2));
    }

    #[test]
    fn a_transform_only_branch_answer_names_the_transform_count() {
        let report = report(vec![transform("je"), transform("reconcile")]);
        assert_eq!(
            report.answer(),
            "0 changed pipeline(s): 0 need Reset schema after merge, 0 with warnings, 2 changed transform(s)"
        );
    }

    #[test]
    fn an_answer_with_no_changed_transforms_omits_the_transform_clause() {
        let report = report(Vec::new());
        assert_eq!(
            report.answer(),
            "0 changed pipeline(s): 0 need Reset schema after merge, 0 with warnings"
        );
    }
}
