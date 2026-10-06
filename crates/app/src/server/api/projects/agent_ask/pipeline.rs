//! The pipeline a custom-app ask is driven with.
//!
//! One definition for the two places that build it: the start handler, which
//! drives the ask in the request's process today, and
//! [`super::executor::AgentAskExecutor`], which drives one a driver claimed
//! from the queue. Sharing it is what makes "a queued ask is driven the way
//! the handler drives it" a fact about the code rather than a claim about two
//! copies: the thread travels (a follow-up sees its earlier turns), and no
//! human-input provider is installed over the default, so the agent can still
//! stop to ask its user a question.
//!
//! Analytics domain only — the builder is workspace-scoped and not exposed to
//! bundles.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agentic_pipeline::platform::PlatformContext;
use agentic_pipeline::{AnalyticsSchemaCatalog, PipelineBuilder};
use uuid::Uuid;

/// The builder for one ask. The caller adds the one thing that differs between
/// the two drives: the handler records the caller on the run it is about to
/// insert (`run_metadata`), a queued attempt names the run that already exists
/// (`existing_run`).
pub(super) fn ask_pipeline(
    platform: Arc<dyn PlatformContext>,
    project_id: Uuid,
    question: &str,
    thread_id: Option<Uuid>,
    schema_cache: Option<Arc<Mutex<HashMap<String, AnalyticsSchemaCatalog>>>>,
    agent_id: &str,
) -> PipelineBuilder {
    let mut builder = PipelineBuilder::new(platform)
        .workspace_id(project_id)
        .question(question);
    if let Some(cache) = schema_cache {
        builder = builder.schema_cache(cache);
    }
    if let Some(thread_id) = thread_id {
        builder = builder.thread(thread_id);
    }
    builder.analytics(agent_id)
}
