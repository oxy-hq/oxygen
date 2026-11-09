//! Bridge: `Event<AnalyticsEvent>` stream → `AnswerStream` items for `SlackRenderer`.
//!
//! `SlackRenderer` consumes the classic agent's `AnswerStream`/`AnswerContent`
//! vocabulary (text deltas, artifact lifecycle events, chart directives).
//! The agentic pipeline emits a different shape (`CoreEvent` + domain
//! `AnalyticsEvent`). This module owns the translation so the existing
//! renderer plumbing — body markdown, SQL artifact capture, chart queueing,
//! the "View SQL queries" footer button — keeps working without
//! understanding agentic event types.
//!
//! Mapping:
//!
//! | Source event                                   | Renderer callback / `AnswerContent`         |
//! | ---------------------------------------------- | ------------------------------------------- |
//! | `AnalyticsEvent::QueryGenerated`               | `ArtifactStarted(ExecuteSQL)` + `Value`     |
//! | `AnalyticsEvent::QueryExecuted` (semantic)     | `ArtifactStarted(SemanticQuery)` + `Value`  |
//! | `AnalyticsEvent::QueryExecuted` (verified_sql) | `ArtifactStarted(ExecuteSQL, verified)`     |
//! | `AnalyticsEvent::QueryExecuted` (llm)          | `ArtifactStarted(ExecuteSQL)` + `Value`     |
//! | `AnalyticsEvent::SemanticShortcutResolved`     | `ArtifactStarted(SemanticQuery, verified)`  |
//! | `AnalyticsEvent::ChartRendered`                | `AnswerContent::Chart` after writing JSON   |
//!
//! Dropped events:
//! - `CoreEvent::LlmToken` / `LlmStart` / `LlmEnd` / etc. — the renderer
//!   accumulates prose and posts once at the end; the orchestrator's final
//!   answer text (returned by `run_agentic_streaming`) is the authoritative
//!   body. Forwarding intermediate tokens would just be noise.
//! - `CoreEvent::Error` and `AnalyticsEvent::ExecutionFailed` — terminal
//!   failures surface via the orchestrator's `Err`, which becomes
//!   "Agent run failed: …" in the body. Mid-run errors the orchestrator
//!   recovers from aren't worth surfacing on their own.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use agentic_analytics::{AnalyticsEvent, ChartConfig, QuerySource};
use agentic_core::events::Event;
use agentic_pipeline::platform::PlatformContext;
use oxy::exec_types::event::ArtifactKind;
use oxy::types::{AnswerContent, AnswerStream, ArtifactValue, ExecuteSQL, SemanticQuery};
use oxy_shared::errors::OxyError;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::integrations::slack::chart_render;

/// Run the agentic agent `agent_ref` names with `question`, translating
/// the agent's event stream into `AnswerStream` items written to
/// `answer_tx`. Returns the final answer text from the orchestrator.
///
/// The caller is expected to drain `answer_rx` through a `SlackRenderer`
/// concurrently (typically via `oxy::render_stream(rx, renderer).await`).
/// `answer_tx` is closed once the run finishes so the consumer's loop
/// terminates.
pub async fn run_with_renderer(
    platform: Arc<dyn PlatformContext>,
    agent_ref: &str,
    question: &str,
    workspace_id: Uuid,
    answer_tx: mpsc::Sender<AnswerStream>,
) -> Result<String, OxyError> {
    let (event_tx, mut event_rx) = mpsc::channel::<Event<AnalyticsEvent>>(256);

    // Translator: read agentic events, push AnswerStream items into the
    // renderer channel. Lives as long as the pipeline runs.
    let translator_tx = answer_tx.clone();
    let translator = tokio::spawn(async move {
        let state = TranslatorState::new(workspace_id);
        while let Some(event) = event_rx.recv().await {
            state.handle(event, &translator_tx).await;
        }
    });

    let pipeline_result = agentic_pipeline::run_agentic_streaming(
        platform,
        agent_ref,
        question.to_string(),
        event_tx,
    )
    .await
    .map_err(OxyError::RuntimeError);

    // Pipeline finished; the translator's `event_rx` will close as
    // `event_tx` drops out of scope. Wait for it to finish draining so
    // every SQL/chart artifact lands in the renderer before the caller
    // reads `render_result`.
    let _ = translator.await;

    // The orchestrator's final answer text is *not* sent through the
    // renderer channel — the caller (`execution.rs`) uses our return value
    // directly as the Slack message body. Forwarding it via `on_text` would
    // round-trip through `SlackRenderer.body` without actually being read
    // (execution.rs prefers the orchestrator's text over the accumulated
    // body), so we just close the channel and return.
    drop(answer_tx);
    pipeline_result
}

// Translator state

struct TranslatorState {
    /// Workspace ID — needed so chart events can write the translated
    /// echarts spec into the workspace's charts dir before emitting an
    /// `AnswerContent::Chart` event that points at the new file.
    workspace_id: Uuid,
}

impl TranslatorState {
    fn new(workspace_id: Uuid) -> Self {
        Self { workspace_id }
    }

    async fn handle(&self, event: Event<AnalyticsEvent>, tx: &mpsc::Sender<AnswerStream>) {
        if let Event::Domain(domain) = event {
            self.handle_domain(domain, tx).await;
        }
        // `CoreEvent`s (state transitions, tokens, tool calls, suspension,
        // delegation, errors) are all dropped. The renderer accumulates the
        // orchestrator's final answer instead of token-streaming, and
        // terminal errors surface via the orchestrator's `Err` upstream.
    }

    async fn handle_domain(&self, event: AnalyticsEvent, tx: &mpsc::Sender<AnswerStream>) {
        match event {
            AnalyticsEvent::QueryGenerated { sql, .. } => {
                self.emit_sql_artifact(tx, sql, String::new(), "Generated query", false)
                    .await;
            }
            AnalyticsEvent::QueryExecuted {
                query,
                source,
                success,
                ..
            } => {
                if !success {
                    // Mid-run query failures aren't surfaced — the agent
                    // typically retries and the user only cares about the
                    // final answer. Terminal failures bubble up via the
                    // orchestrator's `Err` and become the message body.
                    return;
                }
                match source {
                    QuerySource::Semantic => {
                        self.emit_semantic_artifact(tx, query, true).await;
                    }
                    QuerySource::VerifiedSql => {
                        self.emit_sql_artifact(tx, query, String::new(), "Verified query", true)
                            .await;
                    }
                    QuerySource::Llm => {
                        self.emit_sql_artifact(tx, query, String::new(), "Executed query", false)
                            .await;
                    }
                    QuerySource::Vendor => {
                        // Omni / Looker / Cube don't go through the Slack SQL
                        // capture path today — see render.rs's artifact filter.
                    }
                }
            }
            AnalyticsEvent::SemanticShortcutResolved { sql } => {
                self.emit_semantic_artifact(tx, sql, true).await;
            }
            AnalyticsEvent::ChartRendered {
                config,
                columns,
                rows,
            } => {
                self.handle_chart(tx, config, &columns, &rows).await;
            }
            _ => {}
        }
    }

    async fn emit_sql_artifact(
        &self,
        tx: &mpsc::Sender<AnswerStream>,
        sql: String,
        database: String,
        title: &str,
        is_verified: bool,
    ) {
        if sql.trim().is_empty() {
            return;
        }
        let id = Uuid::new_v4().to_string();
        let _ = tx
            .send(make_stream(AnswerContent::ArtifactStarted {
                id: id.clone(),
                title: title.to_string(),
                is_verified,
                kind: ArtifactKind::ExecuteSQL {
                    sql: sql.clone(),
                    database: database.clone(),
                },
            }))
            .await;
        let _ = tx
            .send(make_stream(AnswerContent::ArtifactValue {
                id: id.clone(),
                value: ArtifactValue::ExecuteSQL(ExecuteSQL {
                    database,
                    sql_query: sql,
                    result: Vec::new(),
                    is_result_truncated: false,
                }),
            }))
            .await;
        let _ = tx
            .send(make_stream(AnswerContent::ArtifactDone { id, error: None }))
            .await;
    }

    async fn emit_semantic_artifact(
        &self,
        tx: &mpsc::Sender<AnswerStream>,
        sql: String,
        is_verified: bool,
    ) {
        if sql.trim().is_empty() {
            return;
        }
        let id = Uuid::new_v4().to_string();
        // `ArtifactKind::SemanticQuery {}` is empty by design — the renderer
        // stashes pending and waits for the value with `sql_query` populated.
        // Other fields on the value struct are unused by Slack; defaulting
        // them keeps the bridge independent of analytics' internal types.
        let _ = tx
            .send(make_stream(AnswerContent::ArtifactStarted {
                id: id.clone(),
                title: "Semantic query".to_string(),
                is_verified,
                kind: ArtifactKind::SemanticQuery {},
            }))
            .await;
        let _ = tx
            .send(make_stream(AnswerContent::ArtifactValue {
                id: id.clone(),
                value: ArtifactValue::SemanticQuery(SemanticQuery {
                    database: String::new(),
                    sql_query: sql,
                    result: Vec::new(),
                    error: None,
                    validation_error: None,
                    sql_generation_error: None,
                    is_result_truncated: false,
                    topic: None,
                    dimensions: Vec::new(),
                    measures: Vec::new(),
                    time_dimensions: Vec::new(),
                    filters: Vec::new(),
                    orders: Vec::new(),
                    limit: None,
                    offset: None,
                }),
            }))
            .await;
        let _ = tx
            .send(make_stream(AnswerContent::ArtifactDone { id, error: None }))
            .await;
    }

    /// Translate a `ChartRendered` event into a chart JSON file in the
    /// workspace's charts dir, then emit `AnswerContent::Chart` so the
    /// renderer's `on_chart` callback queues the PNG for upload. On
    /// translator / write failure, fall back to a body breadcrumb so
    /// the user at least knows a chart was rendered.
    async fn handle_chart(
        &self,
        tx: &mpsc::Sender<AnswerStream>,
        config: ChartConfig,
        columns: &[String],
        rows: &[Vec<serde_json::Value>],
    ) {
        let title = config.title.clone();
        let title_for_fallback = title.clone().unwrap_or_else(|| "Chart".to_string());
        let echarts_spec = match chart_config_to_echarts(&config, columns, rows) {
            Some(spec) => spec,
            None => {
                tracing::warn!(
                    chart_type = %config.chart_type,
                    "agentic_bridge: ChartConfig had no convertible shape; falling back to breadcrumb"
                );
                let _ = tx
                    .send(make_stream(AnswerContent::Text {
                        content: format!(
                            "\n\n📊 *{title_for_fallback}* — view full chart in Oxygen\n"
                        ),
                    }))
                    .await;
                return;
            }
        };
        let filename = chart_filename_from_spec(&echarts_spec);
        match chart_render::write_chart_json(self.workspace_id, &filename, &echarts_spec).await {
            Ok(_) => {
                let _ = tx
                    .send(make_stream(AnswerContent::Chart {
                        chart_src: filename,
                    }))
                    .await;
            }
            Err(e) => {
                tracing::warn!(
                    workspace_id = %self.workspace_id,
                    "agentic_bridge: chart write failed: {e}; falling back to breadcrumb"
                );
                let _ = tx
                    .send(make_stream(AnswerContent::Text {
                        content: format!(
                            "\n\n📊 *{title_for_fallback}* — view full chart in Oxygen\n"
                        ),
                    }))
                    .await;
            }
        }
    }
}

/// Translate an agentic `ChartConfig` + result data into the simplified
/// echarts spec `chart_render::build_render_html` expects. Returns `None`
/// when the config is malformed (e.g. missing x/y for a bar chart) so the
/// caller can fall back to a body breadcrumb instead of writing a broken
/// chart JSON the headless Chromium pipeline would render as a blank page.
///
/// Shape (matches the JS in `chart_render.rs::build_render_html`):
///
/// ```json
/// {
///   "title": "string" | null,
///   "xAxis": { "type": "category" | "value", "name": "string", "data": [...] } | null,
///   "yAxis": { "type": "value" | "category", "name": "string" } | null,
///   "series": [{ "name": "string", "type": "bar" | "line" | "pie", "data": [...] }]
/// }
/// ```
fn chart_config_to_echarts(
    config: &ChartConfig,
    columns: &[String],
    rows: &[Vec<serde_json::Value>],
) -> Option<serde_json::Value> {
    let column_index = |name: &str| columns.iter().position(|c| c == name);
    let take_column = |idx: usize| -> Vec<serde_json::Value> {
        rows.iter()
            .map(|row| row.get(idx).cloned().unwrap_or(serde_json::Value::Null))
            .collect()
    };
    let to_string_value = |v: &serde_json::Value| -> String {
        match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        }
    };

    match config.chart_type.as_str() {
        "bar_chart" | "line_chart" => {
            let x_col = config.x.as_ref()?;
            let y_col = config.y.as_ref()?;
            let x_idx = column_index(x_col)?;
            let y_idx = column_index(y_col)?;
            let series_kind = if config.chart_type == "bar_chart" {
                "bar"
            } else {
                "line"
            };

            let x_data: Vec<serde_json::Value> = take_column(x_idx)
                .iter()
                .map(|v| serde_json::Value::String(to_string_value(v)))
                .collect();

            let series_array = if let Some(series_col) = config.series.as_ref() {
                let series_idx = column_index(series_col)?;
                // Group y values by series label, preserving x order within each group.
                let mut buckets: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
                for row in rows {
                    let label =
                        to_string_value(row.get(series_idx).unwrap_or(&serde_json::Value::Null));
                    let value = row.get(y_idx).cloned().unwrap_or(serde_json::Value::Null);
                    match buckets.iter_mut().find(|(name, _)| name == &label) {
                        Some((_, data)) => data.push(value),
                        None => buckets.push((label, vec![value])),
                    }
                }
                buckets
                    .into_iter()
                    .map(|(name, data)| {
                        serde_json::json!({
                            "name": name,
                            "type": series_kind,
                            "data": data,
                        })
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![serde_json::json!({
                    "name": y_col,
                    "type": series_kind,
                    "data": take_column(y_idx),
                })]
            };

            Some(serde_json::json!({
                "title": config.title,
                "xAxis": {
                    "type": "category",
                    "name": config.x_axis_label.clone().unwrap_or_else(|| x_col.clone()),
                    "data": x_data,
                },
                "yAxis": {
                    "type": "value",
                    "name": config.y_axis_label.clone().unwrap_or_else(|| y_col.clone()),
                },
                "series": series_array,
            }))
        }
        "pie_chart" => {
            let name_col = config.name.as_ref()?;
            let value_col = config.value.as_ref()?;
            let name_idx = column_index(name_col)?;
            let value_idx = column_index(value_col)?;
            let data: Vec<serde_json::Value> = rows
                .iter()
                .map(|row| {
                    let n = to_string_value(row.get(name_idx).unwrap_or(&serde_json::Value::Null));
                    let v = row
                        .get(value_idx)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    serde_json::json!({ "name": n, "value": v })
                })
                .collect();
            Some(serde_json::json!({
                "title": config.title,
                "series": [{
                    "name": name_col,
                    "type": "pie",
                    "data": data,
                }],
            }))
        }
        // `table` is the only other variant the analytics tool emits; Slack
        // doesn't render tables inline today.
        _ => None,
    }
}

/// Stable filename for the chart JSON. Hash the serialized echarts spec
/// so identical charts produced by concurrent runs land at the same path
/// (the existing PNG cache then dedupes the headless-render work too).
fn chart_filename_from_spec(spec: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(spec).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("agentic_{:016x}.json", hasher.finish())
}

fn make_stream(content: AnswerContent) -> AnswerStream {
    let is_error = matches!(content, AnswerContent::Error { .. });
    AnswerStream {
        content,
        references: Vec::new(),
        is_error,
        step: String::new(),
    }
}

#[cfg(test)]
mod chart_translator_tests {
    use super::*;
    use serde_json::json;

    fn cfg(chart_type: &str) -> ChartConfig {
        ChartConfig {
            chart_type: chart_type.to_string(),
            x: None,
            y: None,
            series: None,
            name: None,
            value: None,
            title: None,
            x_axis_label: None,
            y_axis_label: None,
        }
    }

    #[test]
    fn bar_chart_basic_translation() {
        let mut config = cfg("bar_chart");
        config.x = Some("month".into());
        config.y = Some("revenue".into());
        config.title = Some("Q1".into());

        let columns = vec!["month".to_string(), "revenue".to_string()];
        let rows = vec![
            vec![json!("Jan"), json!(100)],
            vec![json!("Feb"), json!(150)],
        ];

        let spec = chart_config_to_echarts(&config, &columns, &rows).unwrap();
        assert_eq!(spec["title"], json!("Q1"));
        assert_eq!(spec["xAxis"]["type"], json!("category"));
        assert_eq!(spec["xAxis"]["data"], json!(["Jan", "Feb"]));
        assert_eq!(spec["yAxis"]["type"], json!("value"));
        let series = spec["series"].as_array().unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0]["type"], json!("bar"));
        assert_eq!(series[0]["data"], json!([100, 150]));
    }

    #[test]
    fn line_chart_uses_line_series_type() {
        let mut config = cfg("line_chart");
        config.x = Some("day".into());
        config.y = Some("count".into());

        let columns = vec!["day".to_string(), "count".to_string()];
        let rows = vec![vec![json!(1), json!(5)]];

        let spec = chart_config_to_echarts(&config, &columns, &rows).unwrap();
        // x values get stringified even when the source column is numeric —
        // echarts category axis expects strings, not numbers.
        assert_eq!(spec["xAxis"]["data"], json!(["1"]));
        assert_eq!(spec["series"][0]["type"], json!("line"));
    }

    #[test]
    fn series_column_splits_into_multiple_series_preserving_order() {
        let mut config = cfg("bar_chart");
        config.x = Some("month".into());
        config.y = Some("revenue".into());
        config.series = Some("region".into());

        let columns = vec![
            "month".to_string(),
            "region".to_string(),
            "revenue".to_string(),
        ];
        let rows = vec![
            vec![json!("Jan"), json!("US"), json!(100)],
            vec![json!("Jan"), json!("EU"), json!(80)],
            vec![json!("Feb"), json!("US"), json!(110)],
            vec![json!("Feb"), json!("EU"), json!(85)],
        ];

        let spec = chart_config_to_echarts(&config, &columns, &rows).unwrap();
        let series = spec["series"].as_array().unwrap();
        assert_eq!(series.len(), 2);
        // First-seen label leads (US in this fixture).
        assert_eq!(series[0]["name"], json!("US"));
        assert_eq!(series[0]["data"], json!([100, 110]));
        assert_eq!(series[1]["name"], json!("EU"));
        assert_eq!(series[1]["data"], json!([80, 85]));
    }

    #[test]
    fn pie_chart_maps_name_value_columns() {
        let mut config = cfg("pie_chart");
        config.name = Some("category".into());
        config.value = Some("count".into());
        config.title = Some("Categories".into());

        let columns = vec!["category".to_string(), "count".to_string()];
        let rows = vec![vec![json!("A"), json!(7)], vec![json!("B"), json!(3)]];

        let spec = chart_config_to_echarts(&config, &columns, &rows).unwrap();
        assert_eq!(spec["title"], json!("Categories"));
        assert!(spec.get("xAxis").is_none(), "pie has no xAxis");
        assert_eq!(spec["series"][0]["type"], json!("pie"));
        let data = spec["series"][0]["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["name"], json!("A"));
        assert_eq!(data[0]["value"], json!(7));
    }

    #[test]
    fn missing_x_y_for_bar_chart_returns_none() {
        let mut config = cfg("bar_chart");
        config.x = Some("month".into());
        // y is missing
        let spec = chart_config_to_echarts(&config, &[], &[]);
        assert!(spec.is_none());
    }

    #[test]
    fn missing_column_returns_none() {
        let mut config = cfg("bar_chart");
        config.x = Some("missing".into());
        config.y = Some("revenue".into());
        let columns = vec!["month".to_string(), "revenue".to_string()];
        let spec = chart_config_to_echarts(&config, &columns, &[]);
        assert!(spec.is_none());
    }

    #[test]
    fn unsupported_chart_type_returns_none() {
        let config = cfg("table");
        let spec = chart_config_to_echarts(&config, &[], &[]);
        assert!(spec.is_none());
    }

    #[test]
    fn empty_rows_produces_empty_series_data() {
        let mut config = cfg("bar_chart");
        config.x = Some("month".into());
        config.y = Some("revenue".into());
        let columns = vec!["month".to_string(), "revenue".to_string()];
        let spec = chart_config_to_echarts(&config, &columns, &[]).unwrap();
        assert_eq!(spec["xAxis"]["data"], json!([]));
        assert_eq!(spec["series"][0]["data"], json!([]));
    }

    #[test]
    fn x_axis_label_overrides_x_column_name() {
        let mut config = cfg("bar_chart");
        config.x = Some("month".into());
        config.y = Some("revenue".into());
        config.x_axis_label = Some("Time".into());
        config.y_axis_label = Some("Revenue ($)".into());
        let columns = vec!["month".to_string(), "revenue".to_string()];
        let spec = chart_config_to_echarts(&config, &columns, &[]).unwrap();
        assert_eq!(spec["xAxis"]["name"], json!("Time"));
        assert_eq!(spec["yAxis"]["name"], json!("Revenue ($)"));
    }

    #[test]
    fn filename_is_stable_across_calls() {
        let spec = json!({"a": 1, "b": [1, 2]});
        let a = chart_filename_from_spec(&spec);
        let b = chart_filename_from_spec(&spec);
        assert_eq!(a, b);
        assert!(a.starts_with("agentic_"));
        assert!(a.ends_with(".json"));
    }

    #[test]
    fn filename_differs_for_different_specs() {
        let a = chart_filename_from_spec(&json!({"a": 1}));
        let b = chart_filename_from_spec(&json!({"a": 2}));
        assert_ne!(a, b);
    }
}

#[cfg(test)]
mod translator_tests {
    use super::*;

    fn collect(rx: &mut mpsc::Receiver<AnswerStream>) -> Vec<AnswerStream> {
        let mut out = Vec::new();
        while let Ok(item) = rx.try_recv() {
            out.push(item);
        }
        out
    }

    fn artifact_kind_label(content: &AnswerContent) -> Option<&'static str> {
        match content {
            AnswerContent::ArtifactStarted { kind, .. } => Some(match kind {
                ArtifactKind::ExecuteSQL { .. } => "execute_sql",
                ArtifactKind::SemanticQuery {} => "semantic_query",
                _ => "other",
            }),
            _ => None,
        }
    }

    #[tokio::test]
    async fn query_generated_emits_execute_sql_artifact() {
        let (tx, mut rx) = mpsc::channel::<AnswerStream>(8);
        let state = TranslatorState::new(Uuid::new_v4());
        state
            .handle(
                Event::Domain(AnalyticsEvent::QueryGenerated {
                    sql: "SELECT 1".to_string(),
                    sub_spec_index: None,
                }),
                &tx,
            )
            .await;
        let items = collect(&mut rx);
        assert_eq!(items.len(), 3, "started + value + done");
        assert_eq!(artifact_kind_label(&items[0].content), Some("execute_sql"));
        assert!(matches!(
            items[1].content,
            AnswerContent::ArtifactValue { .. }
        ));
        assert!(matches!(
            items[2].content,
            AnswerContent::ArtifactDone { .. }
        ));
    }

    #[tokio::test]
    async fn query_executed_semantic_emits_semantic_artifact() {
        let (tx, mut rx) = mpsc::channel::<AnswerStream>(8);
        let state = TranslatorState::new(Uuid::new_v4());
        state
            .handle(
                Event::Domain(AnalyticsEvent::QueryExecuted {
                    query: "SELECT 1".to_string(),
                    row_count: 1,
                    duration_ms: 0,
                    success: true,
                    error: None,
                    columns: vec![],
                    rows: vec![],
                    source: QuerySource::Semantic,
                    sub_spec_index: None,
                    semantic_query: None,
                    is_preagg: false,
                }),
                &tx,
            )
            .await;
        let items = collect(&mut rx);
        assert_eq!(items.len(), 3);
        assert_eq!(
            artifact_kind_label(&items[0].content),
            Some("semantic_query")
        );
        match &items[0].content {
            AnswerContent::ArtifactStarted { is_verified, .. } => assert!(*is_verified),
            _ => panic!("expected ArtifactStarted"),
        }
    }

    #[tokio::test]
    async fn query_executed_verified_sql_marks_artifact_verified() {
        let (tx, mut rx) = mpsc::channel::<AnswerStream>(8);
        let state = TranslatorState::new(Uuid::new_v4());
        state
            .handle(
                Event::Domain(AnalyticsEvent::QueryExecuted {
                    query: "SELECT 1".to_string(),
                    row_count: 1,
                    duration_ms: 0,
                    success: true,
                    error: None,
                    columns: vec![],
                    rows: vec![],
                    source: QuerySource::VerifiedSql,
                    sub_spec_index: None,
                    semantic_query: None,
                    is_preagg: false,
                }),
                &tx,
            )
            .await;
        let items = collect(&mut rx);
        assert_eq!(items.len(), 3);
        assert_eq!(artifact_kind_label(&items[0].content), Some("execute_sql"));
        match &items[0].content {
            AnswerContent::ArtifactStarted {
                is_verified, title, ..
            } => {
                assert!(*is_verified);
                assert_eq!(title, "Verified query");
            }
            _ => panic!("expected ArtifactStarted"),
        }
    }

    #[tokio::test]
    async fn query_executed_failed_drops_event() {
        let (tx, mut rx) = mpsc::channel::<AnswerStream>(8);
        let state = TranslatorState::new(Uuid::new_v4());
        state
            .handle(
                Event::Domain(AnalyticsEvent::QueryExecuted {
                    query: "BAD".to_string(),
                    row_count: 0,
                    duration_ms: 0,
                    success: false,
                    error: Some("boom".to_string()),
                    columns: vec![],
                    rows: vec![],
                    source: QuerySource::Llm,
                    sub_spec_index: None,
                    semantic_query: None,
                    is_preagg: false,
                }),
                &tx,
            )
            .await;
        let items = collect(&mut rx);
        assert!(items.is_empty(), "failed queries are intentionally dropped");
    }

    #[tokio::test]
    async fn vendor_source_drops_event() {
        let (tx, mut rx) = mpsc::channel::<AnswerStream>(8);
        let state = TranslatorState::new(Uuid::new_v4());
        state
            .handle(
                Event::Domain(AnalyticsEvent::QueryExecuted {
                    query: "SELECT 1".to_string(),
                    row_count: 1,
                    duration_ms: 0,
                    success: true,
                    error: None,
                    columns: vec![],
                    rows: vec![],
                    source: QuerySource::Vendor,
                    sub_spec_index: None,
                    semantic_query: None,
                    is_preagg: false,
                }),
                &tx,
            )
            .await;
        let items = collect(&mut rx);
        assert!(items.is_empty(), "vendor queries aren't captured by slack");
    }

    #[tokio::test]
    async fn semantic_shortcut_emits_verified_semantic_artifact() {
        let (tx, mut rx) = mpsc::channel::<AnswerStream>(8);
        let state = TranslatorState::new(Uuid::new_v4());
        state
            .handle(
                Event::Domain(AnalyticsEvent::SemanticShortcutResolved {
                    sql: "SELECT 1".to_string(),
                }),
                &tx,
            )
            .await;
        let items = collect(&mut rx);
        assert_eq!(items.len(), 3);
        assert_eq!(
            artifact_kind_label(&items[0].content),
            Some("semantic_query")
        );
        match &items[0].content {
            AnswerContent::ArtifactStarted { is_verified, .. } => assert!(*is_verified),
            _ => panic!("expected ArtifactStarted"),
        }
    }

    #[tokio::test]
    async fn empty_sql_skips_artifact_emission() {
        let (tx, mut rx) = mpsc::channel::<AnswerStream>(8);
        let state = TranslatorState::new(Uuid::new_v4());
        state
            .handle(
                Event::Domain(AnalyticsEvent::QueryGenerated {
                    sql: "  ".to_string(),
                    sub_spec_index: None,
                }),
                &tx,
            )
            .await;
        let items = collect(&mut rx);
        assert!(items.is_empty(), "whitespace-only SQL is dropped");
    }
}
