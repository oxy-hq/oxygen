//! Semantic-model probe: for each `.topic.yml`, run its measures end to end —
//! compile the topic, resolve joins, hit the warehouse, get a scalar back.
//!
//! Measures live on views, not topics, so [`plan`] pairs each topic with a
//! measure from its base view (falling back to the first view it lists), unless
//! the config names the measures itself. That pairing is pure, and is where all
//! the interesting edge cases are, so it holds this module's tests; [`query`] is
//! the thin I/O half.

use std::sync::Arc;

use agentic_pipeline::platform::ProjectContext;
use oxy::config::health_check::SemanticProbeTarget;
use oxy_airlayer_compat::engine::query::QueryRequest;

use super::ProbeFailure;
use oxy::config::CompiledArtifact;
use oxy_app::surface::workspace_runtime::OxyProjectContext;

/// One topic and the fully-qualified `view.measure` references chosen to
/// exercise it. Never empty — a target with nothing to query is a `MissingTopic`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticTarget {
    pub topic: String,
    pub measures: Vec<String>,
}

/// A topic the config named that we could not turn into a runnable target.
///
/// Distinct from a skip note: a skip is "this topic has nothing to probe", which
/// is unusual but not wrong. A [`MissingTopic`] is the workspace pointing its
/// smoke test at a topic that isn't there — a real misconfiguration the probe
/// exists to catch, so the runner raises it as Unhealthy rather than a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MissingTopic {
    pub topic: String,
    pub reason: String,
}

/// The outcome of planning: what to run, what was skipped (notes), and which
/// explicitly-named topics could not be found at all (failures).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SemanticPlan {
    pub targets: Vec<SemanticTarget>,
    pub skipped: Vec<String>,
    pub missing: Vec<MissingTopic>,
}

/// Pair every topic with one measure to query — the `semantic: true` sweep.
///
/// Returns the runnable targets plus a note for every topic that was skipped, so
/// a topic that can never be probed (no views, or views with no measures) is
/// visible rather than silently absent from the results. A skip is a note, not a
/// failure: a topic with no measures is unusual, not broken.
pub(crate) fn plan(topics: &[CompiledArtifact], views: &[CompiledArtifact]) -> SemanticPlan {
    let mut plan = SemanticPlan::default();

    for topic in topics {
        let name = topic_name(topic);
        match pick_measure(&topic.definition, views) {
            Some(measure) => plan.targets.push(SemanticTarget {
                topic: name,
                measures: vec![measure],
            }),
            None => plan.skipped.push(format!(
                "topic '{name}' has no queryable measure on its views"
            )),
        }
    }
    plan
}

/// Plan only the topics the config named — the `semantic: [ { topic: … } ]`
/// form.
///
/// A named topic that doesn't exist is a failure, not a skip: the workspace
/// asserted it should be probed, so silently checking nothing there would be the
/// exact false OK this dimension exists to prevent. A named topic that exists but
/// has no auto-pickable measure is the same failure for the same reason — naming
/// it was a promise we can't keep. Explicit `measures:` are passed through
/// unvalidated; if one doesn't resolve, the query itself fails, which is a truer
/// signal than anything we could check here.
pub(crate) fn plan_selected(
    selection: &[SemanticProbeTarget],
    topics: &[CompiledArtifact],
    views: &[CompiledArtifact],
) -> SemanticPlan {
    let mut plan = SemanticPlan::default();

    for want in selection {
        if !want.measures.is_empty() {
            plan.targets.push(SemanticTarget {
                topic: want.topic.clone(),
                measures: want.measures.clone(),
            });
            continue;
        }
        let Some(topic) = topics.iter().find(|t| topic_name(t) == want.topic) else {
            plan.missing.push(MissingTopic {
                topic: want.topic.clone(),
                reason: format!(
                    "topic '{}' is named in health_check.smoke_test.semantic but does not exist",
                    want.topic
                ),
            });
            continue;
        };
        match pick_measure(&topic.definition, views) {
            Some(measure) => plan.targets.push(SemanticTarget {
                topic: want.topic.clone(),
                measures: vec![measure],
            }),
            None => plan.missing.push(MissingTopic {
                topic: want.topic.clone(),
                reason: format!(
                    "topic '{}' is named in health_check.smoke_test.semantic but has no queryable \
                     measure on its views — set explicit `measures:`",
                    want.topic
                ),
            }),
        }
    }
    plan
}

/// The topic's declared `name`, falling back to its file path — a compiled topic
/// should always have a name, but a probe must never panic on a malformed one.
fn topic_name(topic: &CompiledArtifact) -> String {
    topic
        .definition
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| topic.name.clone())
}

/// Resolve `topic -> base_view (or first view) -> first measure`, yielding the
/// `view.measure` reference airlayer expects.
fn pick_measure(topic: &serde_json::Value, views: &[CompiledArtifact]) -> Option<String> {
    let declared: Vec<&str> = topic
        .get("views")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    // Prefer the base view — it's the one the topic is anchored on, so a measure
    // from it needs no join to resolve.
    let base = topic.get("base_view").and_then(|v| v.as_str());
    let ordered = base.into_iter().chain(declared);

    for view_name in ordered {
        let Some(view) = views
            .iter()
            .find(|v| v.definition.get("name").and_then(|n| n.as_str()) == Some(view_name))
        else {
            continue;
        };
        if let Some(measure) = first_measure_name(&view.definition) {
            return Some(format!("{view_name}.{measure}"));
        }
    }
    None
}

fn first_measure_name(view: &serde_json::Value) -> Option<String> {
    view.get("measures")?
        .as_array()?
        .iter()
        .find_map(|m| m.get("name").and_then(|n| n.as_str()))
        .map(str::to_string)
}

/// The probe asks "does this measure resolve and execute", not "is the number
/// right", so it needs exactly one row back.
///
/// Note this bounds ROWS RETURNED, not BYTES SCANNED. A measure with no
/// dimensions aggregates to a single row anyway, so `LIMIT 1` does not shrink
/// the scan on any warehouse — an aggregate must read every value to reduce it.
/// It is set for hygiene: it stops the probe depending on airlayer's 10k
/// `DEFAULT_QUERY_LIMIT` fill, and stops the result being flagged
/// `default_limit_applied`. Bounding the actual scan needs a partition filter,
/// not a limit.
const PROBE_ROW_LIMIT: u64 = 1;

/// Run the target's measures through the semantic model to the warehouse. No
/// time window is injected, so on a bytes-scanned warehouse (BigQuery on-demand,
/// Athena) this reads each measure's full column. Keep that in mind before
/// enabling the semantic probe against large unpartitioned fact tables.
///
/// A topic's measures go out as one `QueryRequest`, which is one round-trip
/// rather than N and additionally exercises the join resolution between them.
pub(crate) async fn query(
    ctx: &Arc<OxyProjectContext>,
    target: &SemanticTarget,
) -> Result<(), ProbeFailure> {
    let runner = ctx.metric_tree_runner_system().ok_or_else(|| {
        ProbeFailure::Unavailable("semantic runner is not wired for this workspace".to_string())
    })?;

    let mut request = QueryRequest::new();
    request.measures = target.measures.clone();
    request.limit = Some(PROBE_ROW_LIMIT);

    runner
        .run_query_scalar(request)
        .await
        .map(|_| ())
        .map_err(|e| {
            ProbeFailure::Broken(format!(
                "measures [{}] failed: {e}",
                target.measures.join(", ")
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn artifact(name: &str, definition: serde_json::Value) -> CompiledArtifact {
        CompiledArtifact {
            file_path: format!("semantics/{name}.yml"),
            name: name.to_string(),
            definition,
            blob_key: None,
        }
    }

    fn view(name: &str, measures: &[&str]) -> CompiledArtifact {
        artifact(
            name,
            json!({
                "name": name,
                "measures": measures.iter().map(|m| json!({ "name": m })).collect::<Vec<_>>(),
            }),
        )
    }

    fn want(topic: &str, measures: &[&str]) -> SemanticProbeTarget {
        SemanticProbeTarget {
            topic: topic.to_string(),
            measures: measures.iter().map(|m| m.to_string()).collect(),
        }
    }

    #[test]
    fn prefers_the_base_view_over_the_first_listed_view() {
        let views = vec![view("line_items", &["qty"]), view("orders", &["net"])];
        let topics = vec![artifact(
            "sales",
            json!({ "name": "sales", "views": ["line_items", "orders"], "base_view": "orders" }),
        )];
        let p = plan(&topics, &views);
        assert!(p.skipped.is_empty());
        assert_eq!(
            p.targets,
            vec![SemanticTarget {
                topic: "sales".into(),
                measures: vec!["orders.net".into()],
            }]
        );
    }

    #[test]
    fn falls_back_to_the_first_view_with_a_measure() {
        // No base_view, and the first listed view has no measures — keep walking
        // rather than declaring the topic unprobeable.
        let views = vec![view("dim_date", &[]), view("orders", &["net"])];
        let topics = vec![artifact(
            "sales",
            json!({ "name": "sales", "views": ["dim_date", "orders"] }),
        )];
        let p = plan(&topics, &views);
        assert_eq!(p.targets[0].measures, ["orders.net"]);
    }

    #[test]
    fn topic_with_no_measurable_view_is_skipped_with_a_note() {
        let views = vec![view("dim_date", &[])];
        let topics = vec![artifact(
            "calendar",
            json!({ "name": "calendar", "views": ["dim_date"] }),
        )];
        let p = plan(&topics, &views);
        assert!(p.targets.is_empty());
        assert_eq!(p.skipped.len(), 1);
        assert!(p.skipped[0].contains("calendar"));
        assert!(p.skipped[0].contains("no queryable measure"));
        // The sweep never raises a missing-topic failure: it only probes what it
        // found, so there is nothing to be missing.
        assert!(p.missing.is_empty());
    }

    #[test]
    fn topic_referencing_a_missing_view_is_skipped_not_panicking() {
        let topics = vec![artifact(
            "ghost",
            json!({ "name": "ghost", "views": ["nonexistent"], "base_view": "nonexistent" }),
        )];
        let p = plan(&topics, &[]);
        assert!(p.targets.is_empty());
        assert_eq!(p.skipped.len(), 1);
    }

    #[test]
    fn malformed_topic_falls_back_to_its_artifact_name() {
        // No `name` key in the definition — must not panic, and must still be
        // identifiable in the skip note.
        let topics = vec![artifact("orphan", json!({ "views": [] }))];
        let p = plan(&topics, &[]);
        assert!(p.skipped[0].contains("orphan"));
    }

    #[test]
    fn every_topic_yields_at_most_one_target() {
        let views = vec![view("orders", &["net", "gross", "count"])];
        let topics = vec![
            artifact("a", json!({ "name": "a", "views": ["orders"] })),
            artifact("b", json!({ "name": "b", "views": ["orders"] })),
        ];
        let p = plan(&topics, &views);
        assert_eq!(p.targets.len(), 2);
        // The first measure, not all three — this is a smoke test, not a scan.
        assert!(p.targets.iter().all(|t| t.measures == ["orders.net"]));
    }

    #[test]
    fn a_selection_probes_only_what_it_names() {
        let views = vec![view("orders", &["net"]), view("tickets", &["open"])];
        let topics = vec![
            artifact("sales", json!({ "name": "sales", "views": ["orders"] })),
            artifact(
                "support",
                json!({ "name": "support", "views": ["tickets"] }),
            ),
        ];
        let p = plan_selected(&[want("support", &[])], &topics, &views);
        assert!(p.missing.is_empty() && p.skipped.is_empty());
        assert_eq!(
            p.targets,
            vec![SemanticTarget {
                topic: "support".into(),
                measures: vec!["tickets.open".into()],
            }],
            "the un-named topic must not be probed"
        );
    }

    #[test]
    fn an_explicit_measure_overrides_the_auto_pick() {
        // The reason to name a measure at all: pin the probe to a cheap column
        // instead of whatever the base view happens to list first.
        let views = vec![view("orders", &["net", "cheap_count"])];
        let topics = vec![artifact(
            "sales",
            json!({ "name": "sales", "views": ["orders"] }),
        )];
        let p = plan_selected(&[want("sales", &["orders.cheap_count"])], &topics, &views);
        assert_eq!(p.targets[0].measures, ["orders.cheap_count"]);
    }

    #[test]
    fn an_explicit_measure_needs_no_compiled_topic() {
        // Fully-specified target: nothing to look up, so it must not be reported
        // missing just because the topic list is empty (a draft branch, say).
        let p = plan_selected(&[want("sales", &["orders.net"])], &[], &[]);
        assert!(p.missing.is_empty());
        assert_eq!(p.targets.len(), 1);
    }

    #[test]
    fn a_named_topic_that_does_not_exist_is_a_failure_not_a_silent_skip() {
        // The false-OK guard for the selection path: the workspace asserted this
        // topic should be probed. Checking nothing there must not read Healthy.
        let p = plan_selected(&[want("typo_topic", &[])], &[], &[]);
        assert!(p.targets.is_empty());
        assert_eq!(p.missing.len(), 1);
        assert_eq!(p.missing[0].topic, "typo_topic");
        assert!(p.missing[0].reason.contains("does not exist"));
    }

    #[test]
    fn a_named_topic_with_no_measure_is_a_failure_not_a_note() {
        // The sweep treats this as a note — an unmeasurable topic it happened to
        // walk past. Naming it is a promise, so the same shape is a failure here.
        let views = vec![view("dim_date", &[])];
        let topics = vec![artifact(
            "calendar",
            json!({ "name": "calendar", "views": ["dim_date"] }),
        )];
        let p = plan_selected(&[want("calendar", &[])], &topics, &views);
        assert!(p.targets.is_empty());
        assert_eq!(p.missing.len(), 1);
        assert!(p.missing[0].reason.contains("no queryable measure"));
        assert!(
            p.missing[0].reason.contains("explicit `measures:`"),
            "the failure should say how to fix it"
        );
    }
}
