//! Implements [`BuilderTestRunner`] for the builder copilot by delegating to
//! the Oxy eval pipeline.  This lives in `oxy-app` so it can access
//! `run_eval_with_tag` without creating a circular dependency in the lower
//! crates.

use std::path::Path;

use agentic_builder::BuilderTestRunner;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::exec_runtime::writer::EventHandler;
use oxy::exec_types::Event;
use oxy_shared::errors::OxyError;
use serde_json::{Value, json};

use crate::integrations::eval::MetricKind;
use crate::server::service::eval::run_eval_with_tag;

/// Noop event handler — discards all eval pipeline events.
struct NoopEventHandler;

#[async_trait::async_trait]
impl EventHandler for NoopEventHandler {
    async fn handle_event(&mut self, _event: Event) -> Result<(), OxyError> {
        Ok(())
    }
}

/// [`BuilderTestRunner`] that executes `.test.yml` files via the Oxy eval pipeline.
pub struct OxyTestRunner;

#[async_trait::async_trait]
impl BuilderTestRunner for OxyTestRunner {
    async fn run_tests(&self, workspace_root: &Path, test_file: &str) -> Result<Value, String> {
        let abs_path = workspace_root.join(test_file);

        let workspace_manager = WorkspaceBuilder::new(uuid::Uuid::new_v4())
            .with_working_copy(workspace_root, None, oxy::config::OnMissing::Fail)
            .await
            .map_err(|e| e.to_string())?
            .build()
            .await
            .map_err(|e| e.to_string())?;

        let results = run_eval_with_tag(workspace_manager, abs_path, None, None, NoopEventHandler)
            .await
            .map_err(|e| e.to_string())?;

        // Summarise results into a JSON object the LLM can understand.
        let summaries: Vec<Value> = results
            .iter()
            .map(|r| {
                let score = r.metrics.iter().find_map(|m| match m {
                    MetricKind::Correctness(c) => Some(c.score),
                    MetricKind::Execution(e) => Some(e.answer_acc),
                    _ => None,
                });
                json!({
                    "score": score,
                    "total_attempted": r.stats.total_attempted,
                    "answered": r.stats.answered,
                    "errors": r.errors,
                })
            })
            .collect();

        let overall_score: Option<f32> = if results.is_empty() {
            None
        } else {
            let scores: Vec<f32> = results
                .iter()
                .filter_map(|r| {
                    r.metrics.iter().find_map(|m| match m {
                        MetricKind::Correctness(c) => Some(c.score),
                        MetricKind::Execution(e) => Some(e.answer_acc),
                        _ => None,
                    })
                })
                .collect();
            if scores.is_empty() {
                None
            } else {
                Some(scores.iter().sum::<f32>() / scores.len() as f32)
            }
        };

        Ok(json!({
            "overall_score": overall_score,
            "suites": summaries,
        }))
    }
}
