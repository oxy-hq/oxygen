//! What to probe: turning a `SmokeTestConfig` into a concrete list of targets.
//!
//! Split from `runner.rs` so the two halves stay separable — this module decides
//! *what* gets probed (enumerating, resolving named selections, applying the
//! per-kind cap), while the runner decides *how* (concurrency, timeouts,
//! verdicts). It is pure apart from reading the workspace context and the
//! compiled revision, which is what makes the selection rules unit-testable
//! without a warehouse.

use std::sync::Arc;

use oxy::config::health_check::{SemanticProbeConfig, SmokeTestConfig};

use super::probes::{self, SemanticTarget};
use super::{SmokeProbeKind, SmokeVerdict, failed, note, unavailable};
use crate::agentic_wiring::project_ctx::OxyProjectContext;

/// One thing to probe. `kind`/`label` drive the verdict's identity, so they are
/// derived here rather than threaded through every probe.
pub(super) enum Target {
    Connection(String),
    Semantic(SemanticTarget),
    Agent {
        /// Pre-resolved and de-duplicated by `SmokeTestConfig::resolved_agents`,
        /// so several prompts against one agent stay distinct in the payload.
        label: String,
        agent_ref: String,
        prompt: String,
    },
}

impl Target {
    pub(super) fn kind(&self) -> SmokeProbeKind {
        match self {
            Target::Connection(_) => SmokeProbeKind::Connection,
            Target::Semantic(_) => SmokeProbeKind::Semantic,
            Target::Agent { .. } => SmokeProbeKind::Agent,
        }
    }

    pub(super) fn label(&self) -> String {
        match self {
            Target::Connection(db) => db.clone(),
            Target::Semantic(t) => t.topic.clone(),
            Target::Agent { label, .. } => label.clone(),
        }
    }
}

/// Enumerate everything the config asks us to probe, capped per kind. Returns
/// the targets plus any Healthy notes (skipped topics, truncated lists) that
/// should appear in the checks table regardless of what the probes find.
pub(super) async fn collect_targets(
    ctx: &Arc<OxyProjectContext>,
    _workspace_id: uuid::Uuid,
    cfg: &SmokeTestConfig,
) -> (Vec<Target>, Vec<SmokeVerdict>) {
    let mut targets = Vec::new();
    let mut notes = Vec::new();
    let max = cfg.max_targets();

    if cfg.connections {
        let names: Vec<String> = ctx
            .workspace_manager()
            .config_manager
            .list_databases()
            .into_iter()
            .map(|d| d.name)
            .collect();
        let (kept, note) = cap(SmokeProbeKind::Connection, names, max, "databases");
        notes.extend(note);
        targets.extend(kept.into_iter().map(Target::Connection));
    }

    if cfg.semantic.enabled() {
        match semantic_targets(ctx, &cfg.semantic, max).await {
            Ok((kept, mut semantic_notes)) => {
                notes.append(&mut semantic_notes);
                targets.extend(kept.into_iter().map(Target::Semantic));
            }
            Err(reason) => notes.push(unavailable(SmokeProbeKind::Semantic, "topics", reason)),
        }
    }

    targets.extend(cfg.resolved_agents().into_iter().map(|a| Target::Agent {
        label: a.label,
        agent_ref: a.agent_ref,
        prompt: a.prompt,
    }));

    (targets, notes)
}

/// Pair topics with a measure to query — every compiled topic for a sweep, or
/// just the named ones for a selection. `Err` when there is no compiled revision
/// to read topics from (a draft branch on a non-serve node).
async fn semantic_targets(
    ctx: &Arc<OxyProjectContext>,
    cfg: &SemanticProbeConfig,
    max: usize,
) -> Result<(Vec<SemanticTarget>, Vec<SmokeVerdict>), String> {
    let config_manager = &ctx.workspace_manager().config_manager;
    // The smoke test probes what was PROMOTED, so a manager reading the working
    // copy is an error rather than an empty probe list.
    let (views, topics) = config_manager
        .promoted_semantic_entities()
        .await
        .map_err(|e| format!("could not read the promoted semantic entities: {e}"))?;

    let plan = match cfg.selection() {
        Some(selection) => probes::plan_selected(selection, &topics, &views),
        None => probes::plan(&topics, &views),
    };

    let mut notes: Vec<SmokeVerdict> = plan
        .skipped
        .into_iter()
        .map(|reason| note(SmokeProbeKind::Semantic, "topics", reason))
        .collect();
    // A topic the config named but we could not resolve is the workspace being
    // wrong, not us — Unhealthy, the same as a measure that fails to run.
    notes.extend(
        plan.missing
            .into_iter()
            .map(|m| failed(SmokeProbeKind::Semantic, m.topic, m.reason, 0)),
    );
    let (kept, cap_note) = cap(SmokeProbeKind::Semantic, plan.targets, max, "topics");
    notes.extend(cap_note);
    Ok((kept, notes))
}

/// Truncate to `max` targets, recording what was dropped. A cap is a fact about
/// the workspace's size, not a health problem, so the note is Healthy — but it
/// is never silent, or a partial sweep would read as full coverage.
fn cap<T>(
    kind: SmokeProbeKind,
    mut items: Vec<T>,
    max: usize,
    label: &str,
) -> (Vec<T>, Option<SmokeVerdict>) {
    let total = items.len();
    if total <= max {
        return (items, None);
    }
    items.truncate(max);
    let dropped = total - max;
    (
        items,
        Some(note(
            kind,
            label,
            format!("probed {max} of {total} {label}; skipped {dropped} (max_targets={max})"),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::api::admin::workspace_health::evaluator::HealthStatus;

    #[test]
    fn cap_under_the_limit_adds_no_note() {
        let (kept, note) = cap(SmokeProbeKind::Connection, vec![1, 2, 3], 25, "databases");
        assert_eq!(kept.len(), 3);
        assert!(note.is_none());
    }

    #[test]
    fn cap_truncates_and_records_what_it_dropped() {
        let (kept, note) = cap(SmokeProbeKind::Semantic, (0..30).collect(), 25, "topics");
        assert_eq!(kept.len(), 25);
        let note = note.expect("truncation must not be silent");
        // Healthy: a big workspace is large, not unhealthy.
        assert_eq!(note.status, HealthStatus::Healthy);
        let reason = note.reason.unwrap();
        assert!(reason.contains("probed 25 of 30 topics"));
        assert!(reason.contains("skipped 5"));
    }

    #[test]
    fn cap_at_exactly_the_limit_is_not_truncation() {
        let (kept, note) = cap(SmokeProbeKind::Semantic, (0..25).collect(), 25, "topics");
        assert_eq!(kept.len(), 25);
        assert!(note.is_none());
    }

    #[test]
    fn target_labels_and_kinds() {
        assert_eq!(
            Target::Connection("bq".into()).kind(),
            SmokeProbeKind::Connection
        );
        assert_eq!(Target::Connection("bq".into()).label(), "bq");
        assert_eq!(
            Target::Semantic(SemanticTarget {
                topic: "sales".into(),
                measures: vec!["orders.net".into()],
            })
            .label(),
            "sales"
        );
        let agent = Target::Agent {
            label: "nightly check".into(),
            agent_ref: "agents/a.agentic.yml".into(),
            prompt: "hi".into(),
        };
        assert_eq!(agent.kind(), SmokeProbeKind::Agent);
        assert_eq!(
            agent.label(),
            "nightly check",
            "the resolved label identifies the probe, not the agent_ref"
        );
    }
}
