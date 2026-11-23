//! The eval's merge step (and the engine the served merge task asks too,
//! [`memory_engine`]): after a learning pass, one model call merges the
//! pass's notes file when the prompt can no longer list every note
//! ([`merge_notes`]). Runs once per file, after every case has finished, so
//! it is the file's only writer.

use std::path::Path;
use std::sync::Arc;

use agentic_analytics::config::BuildContext;
use agentic_core::react::Engine;
use agentic_llm::LlmEngine;
use agentic_llm::engine::EngineConfig;
use agentic_runtime::react::{FileMemoryStore, MergeReport, merge_notes};

use crate::platform::PlatformContext;

/// Merges the notes in `notes` with `agent_ref`'s model (its solving model,
/// without the advisor). `Ok(None)`: they fit the prompt, and no model call
/// was made. On an error the file is left as it was.
pub async fn merge_eval_notes(
    platform: Arc<dyn PlatformContext>,
    agent_ref: &str,
    notes: &Path,
) -> Result<Option<MergeReport>, String> {
    let engine = memory_engine(&*platform, agent_ref).await?;
    let run_id = format!("merge-{}", uuid::Uuid::new_v4());
    let store = FileMemoryStore::new(notes);
    (merge_notes(&*engine, &store, &run_id).await).map_err(|e| e.to_string())
}

/// The engine a merge asks: `agent_ref`'s solving model, without the
/// advisor, or the platform's test engine.
pub(crate) async fn memory_engine(
    platform: &dyn PlatformContext,
    agent_ref: &str,
) -> Result<Box<dyn Engine>, String> {
    let ctx_root = platform.context_root().await;
    let base_dir = ctx_root.path().to_path_buf();
    let (config, _) = super::load_agent(platform, agent_ref, &base_dir)
        .await
        .map_err(|e| e.to_string())?;
    let mut ctx = BuildContext::default();
    ctx.project_model_info = platform
        .resolve_model(config.llm.model_ref.as_deref(), config.llm.model.is_some())
        .await;
    ctx.genai.agent_name = Some(agent_ref.to_string());
    Ok(platform.react_engine().unwrap_or_else(|| {
        let cfg = EngineConfig {
            thinking: config.solving_thinking(&ctx),
            max_tokens: config.llm.max_tokens,
            ..EngineConfig::default()
        };
        Box::new(LlmEngine::new(config.solving_llm_client(&ctx), cfg))
    }))
}
