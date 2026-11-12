//! Slack agent execution against the agentic pipeline.
//!
//! Drives an `.agentic.yml` agent for one Slack message: persists the
//! user's text, runs the agentic pipeline with event streaming, feeds
//! events through [`super::agentic_bridge`] into the existing
//! `SlackRenderer`, then posts the answer + uploads SQL/chart artifacts
//! as thread replies.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;

use crate::blocks;
use crate::client::SlackClient;
use crate::config::SlackConfig;
use crate::error::SlackError;
use crate::events::agentic_bridge;
use crate::render::{CapturedSqlArtifact, QueuedChart, SlackRenderer};
use crate::resolution::thread_context::{CreateThreadContext, ThreadContextService};
use entity::slack_installations::Model as InstallationRow;
use entity::slack_user_links::Model as UserLinkRow;
use oxy::database::client::establish_connection;
use oxy::types::AnswerStream;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use tokio::sync::mpsc;
use uuid::Uuid;

/// Per-file upload budget. Used by chart PNG uploads and SQL artifact
/// uploads — the same `files.uploadV2` shape, the same edge node behavior.
/// Slack typically responds in well under a second; a stuck multipart POST
/// shouldn't block the rest of the queue or the post-message bookkeeping.
pub(crate) const FILE_UPLOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// Cap on uploaded SQL `.sql` files per Slack message. Beyond this the
/// inline placeholder still marks each artifact, and a follow-up
/// "📎 N more queries — view in Oxygen →" context block is posted in the
/// same thread so the overflow is visible to the user.
const MAX_SQL_ARTIFACTS_PER_MESSAGE: usize = 10;

// Public Types

/// All parameters needed to execute the agent from a Slack event.
pub struct SlackRunRequest {
    pub installation: InstallationRow,
    /// Pre-decrypted bot token — avoids a redundant AES-GCM decrypt per event.
    pub bot_token: String,
    pub user_link: UserLinkRow,
    pub workspace_id: Uuid,
    pub agent_path: String,
    pub question: String,
    pub channel_id: String,
    pub thread_ts: String,
    /// The node's Layer-1 pre-aggregation cache. A default carries no cache,
    /// which is always a correct answer and only a slower one: the agent's
    /// semantic queries compile to warehouse SQL instead of reading a local
    /// Parquet rollup.
    pub preagg: PreaggCacheCtx,
}

// Public API

/// Execute an agentic agent from a Slack event and deliver the response.
///
/// Lifecycle:
/// 1. Find-or-create slack_threads + Oxy thread rows
/// 2. Persist user message
/// 3. Send rotating "is working on…" status
/// 4. Drive the agentic pipeline, bridging events into `SlackRenderer`
/// 5. Post the prose body as Block Kit
/// 6. Upload chart PNGs as `files.uploadV2` thread replies
/// 7. Post follow-up context blocks for chart upload failures / SQL overflow
/// 8. Persist agent output + update slack_threads.last_ts
pub async fn run_for_slack(req: SlackRunRequest) -> Result<(), SlackError> {
    tracing::info!(
        workspace_id = %req.workspace_id,
        agent = %req.agent_path,
        channel = %req.channel_id,
        "run_for_slack: starting agentic execution"
    );

    let bot_token = req.bot_token.clone();
    let client = SlackClient::new();

    let (slack_thread_row, oxy_thread_id, is_new_thread) = find_or_create_slack_thread(&req)
        .await
        .map_err(|e| SlackError::Internal {
            source: e,
            thread_url: None,
        })?;

    if is_new_thread {
        let title = truncate(&req.question, 100);
        let _ = client
            .assistant_threads_set_title(&bot_token, &req.channel_id, &req.thread_ts, &title)
            .await;
    }

    let thread_url = match SlackConfig::cached().as_runtime() {
        Some(c) => Some(
            build_thread_url(
                &c.app_base_url,
                req.installation.org_id,
                req.workspace_id,
                oxy_thread_id,
            )
            .await,
        ),
        None => None,
    };

    let internal = |e: OxyError| SlackError::Internal {
        source: e,
        thread_url: thread_url.clone(),
    };

    persist_user_message(oxy_thread_id, &req.question)
        .await
        .map_err(&internal)?;

    set_loading_status(&client, &bot_token, &req.channel_id, &req.thread_ts).await;

    let upload_charts = crate::config::chart_upload_enabled();

    let exec = run_agentic_for_slack(
        req.workspace_id,
        &req.agent_path,
        &req.question,
        &req.channel_id,
        &req.thread_ts,
        thread_url.clone(),
        upload_charts,
        &client,
        &bot_token,
        req.preagg,
    )
    .await
    .map_err(&internal)?;

    let _ = persist_agent_message(oxy_thread_id, &exec.final_markdown).await;
    let _ = update_thread_with_output(oxy_thread_id, &exec.final_markdown).await;

    let (all_blocks, view_sql, sql_overflow) = build_message_blocks(
        &exec,
        thread_url.as_deref(),
        &req.question,
        req.installation.org_id,
        &req.user_link.slack_user_id,
        &req.agent_path,
    )
    .await;

    let fallback_text = blocks::pick_fallback_text(exec.agent_errored, &exec.final_markdown);
    if let Err(e) = client
        .chat_post_message_with_blocks(
            &bot_token,
            &req.channel_id,
            &fallback_text,
            Some(&req.thread_ts),
            Some(serde_json::Value::Array(all_blocks)),
        )
        .await
    {
        tracing::warn!("chat.postMessage failed: {e}");
    }

    let upload_failures = upload_charts_sequentially(
        &client,
        &bot_token,
        &req.channel_id,
        &req.thread_ts,
        exec.queued_charts,
        &req.question,
        exec.agent_errored,
    )
    .await;

    post_overflow_followups(
        &client,
        &bot_token,
        &req.channel_id,
        &req.thread_ts,
        upload_failures,
        sql_overflow,
        view_sql,
        thread_url.as_deref(),
        exec.agent_errored,
    )
    .await;

    ThreadContextService::update_last_ts(slack_thread_row.id, &req.thread_ts)
        .await
        .map_err(&internal)?;

    Ok(())
}

// Agentic execution

/// Output of one agentic Slack run, fed into block assembly + uploads.
struct AgentExecOutput {
    queued_charts: Vec<QueuedChart>,
    chart_local_paths: Vec<std::path::PathBuf>,
    failed_chart_count: usize,
    captured_sql_artifacts: Vec<CapturedSqlArtifact>,
    final_markdown: String,
    agent_errored: bool,
}

#[allow(clippy::too_many_arguments)]
async fn run_agentic_for_slack(
    workspace_id: Uuid,
    agent_path: &str,
    question: &str,
    channel_id: &str,
    thread_ts: &str,
    thread_url: Option<String>,
    upload_charts: bool,
    client: &SlackClient,
    bot_token: &str,
    preagg: PreaggCacheCtx,
) -> Result<AgentExecOutput, OxyError> {
    let workspace_manager =
        crate::workspace::build_manager(workspace_id, "slack_agentic_run").await?;

    // Without the node's cache the agent still answers correctly, it just
    // compiles every semantic query to warehouse SQL rather than reading a
    // rollup, so an absent cache is a slowdown and never a wrong result.
    let project_ctx = Arc::new(
        oxy_app::surface::workspace_runtime::OxyProjectContext::new(workspace_manager)
            .with_preagg(&preagg),
    );
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = project_ctx;

    // Renderer channel: bridge writes AnswerStream items; renderer drains
    // them into the SlackRenderResult (body markdown + queued charts + SQL).
    let (answer_tx, answer_rx) = mpsc::channel::<AnswerStream>(256);

    let renderer = SlackRenderer::new(
        client,
        bot_token,
        channel_id,
        thread_ts,
        thread_url.clone(),
        workspace_id,
        upload_charts,
    );

    // Spawn the bridge: drives the agentic pipeline, translates events,
    // and closes `answer_tx` when the pipeline finishes. The renderer
    // loop below terminates naturally on channel close.
    // `agent_path` is a ref, read from the compiled revision on a fleet pod
    // (`ConfigManager::agent_by_ref`), never joined onto a working copy. A
    // stored classic `.agent.yml` path names no compiled agent and the run
    // answers "not found" rather than reading the file.
    let agent_ref = agent_path.to_string();
    let question_owned = question.to_string();
    let platform_for_bridge = platform.clone();
    let bridge_handle = tokio::spawn(async move {
        agentic_bridge::run_with_renderer(
            platform_for_bridge,
            &agent_ref,
            &question_owned,
            workspace_id,
            answer_tx,
        )
        .await
    });

    let render_result = oxy::render_stream(answer_rx, renderer).await;
    let bridge_result = bridge_handle
        .await
        .map_err(|e| OxyError::RuntimeError(format!("agentic bridge task panicked: {e}")))?;

    let (final_markdown, agent_errored) = match bridge_result {
        Ok(text) if text.trim().is_empty() => ("Done.".to_string(), false),
        Ok(text) => (text, false),
        Err(e) => (format!("Agent run failed: {e}"), true),
    };

    Ok(AgentExecOutput {
        queued_charts: render_result.queued_charts,
        chart_local_paths: render_result.chart_local_paths,
        failed_chart_count: render_result.failed_chart_count,
        captured_sql_artifacts: render_result.captured_sql_artifacts,
        final_markdown,
        agent_errored,
    })
}

// Block assembly

async fn build_message_blocks(
    exec: &AgentExecOutput,
    thread_url: Option<&str>,
    question: &str,
    org_id: Uuid,
    slack_user_id: &str,
    agent_path: &str,
) -> (Vec<serde_json::Value>, Option<(Uuid, usize)>, usize) {
    let mut all_blocks: Vec<serde_json::Value> = if exec.agent_errored {
        blocks::build_error_alert_blocks(&exec.final_markdown)
    } else {
        blocks::build_body_blocks(&exec.final_markdown)
    };

    // Local-render breadcrumbs for the dev path where chart upload is off.
    if !exec.agent_errored {
        for path in &exec.chart_local_paths {
            all_blocks.push(serde_json::json!({
                "type": "context",
                "elements": [{
                    "type": "mrkdwn",
                    "text": format!(
                        "📊 Chart rendered locally — `{}` (open this file to validate; Slack can't fetch localhost paths so no inline preview)",
                        path.display()
                    ),
                }],
            }));
        }
    }

    // Render-failure footer: charts that never produced bytes get a "view
    // in Oxygen" link. Upload failures live in post_overflow_followups.
    if !exec.agent_errored
        && exec.failed_chart_count > 0
        && let Some(url) = thread_url
    {
        let label = if exec.failed_chart_count == 1 {
            "⚠️ Chart render failed — view in Oxygen →".to_string()
        } else {
            format!(
                "⚠️ {} chart renders failed — view in Oxygen →",
                exec.failed_chart_count
            )
        };
        all_blocks.push(serde_json::json!({
            "type": "context",
            "elements": [{"type": "mrkdwn", "text": format!("<{url}|{label}>")}],
        }));
    }

    // SQL artifacts: stash behind a deferred-upload button.
    let captured_total = exec.captured_sql_artifacts.len();
    let sql_to_upload = captured_total.min(MAX_SQL_ARTIFACTS_PER_MESSAGE);
    let sql_overflow = captured_total.saturating_sub(MAX_SQL_ARTIFACTS_PER_MESSAGE);
    if sql_overflow > 0 {
        tracing::warn!(
            captured = captured_total,
            uploaded = sql_to_upload,
            dropped = sql_overflow,
            "sql artifact cap reached; some queries will not be uploaded even if the user clicks the button"
        );
    }
    let view_sql: Option<(Uuid, usize)> = if !exec.agent_errored && sql_to_upload > 0 {
        let to_stash: Vec<_> = exec
            .captured_sql_artifacts
            .iter()
            .take(sql_to_upload)
            .cloned()
            .collect();
        let upload_id = crate::services::pending_sql_uploads::insert(to_stash).await;
        Some((upload_id, sql_to_upload))
    } else {
        None
    };

    if !all_blocks.is_empty() {
        all_blocks.push(serde_json::json!({ "type": "divider" }));
    }

    if !exec.agent_errored
        && let Some(url) = thread_url
    {
        let reopen_q = resolve_reopen_query(org_id, question).await;
        all_blocks.push(blocks::build_footer_actions(
            url,
            reopen_q.as_deref(),
            view_sql,
        ));
    } else if let Some((upload_id, count)) = view_sql {
        all_blocks.push(blocks::build_view_sql_only_actions(upload_id, count));
    }

    if !exec.agent_errored {
        all_blocks.push(blocks::build_attribution_context(
            slack_user_id,
            &blocks::agent_display_name(agent_path),
        ));
    }

    (all_blocks, view_sql, sql_overflow)
}

/// Upload chart PNGs sequentially into the Slack thread, returning the
/// number of failures. Sequential order preserves the agent's chart
/// emission order. Each upload is bounded by [`FILE_UPLOAD_TIMEOUT`].
async fn upload_charts_sequentially(
    client: &SlackClient,
    bot_token: &str,
    channel_id: &str,
    thread_ts: &str,
    queued_charts: Vec<QueuedChart>,
    question: &str,
    agent_errored: bool,
) -> usize {
    if agent_errored || queued_charts.is_empty() {
        return 0;
    }

    let chart_label_base = chart_label_from_question(question);
    let chart_filename_stem = chart_filename_stem_from_question(question);
    let total = queued_charts.len();
    let mut failures: usize = 0;

    for (idx, chart) in queued_charts.into_iter().enumerate() {
        let title = if total > 1 {
            format!("{chart_label_base} ({} of {total})", idx + 1)
        } else {
            chart_label_base.clone()
        };
        let filename = if total > 1 {
            format!("{}-{}.png", chart_filename_stem, idx + 1)
        } else {
            format!("{chart_filename_stem}.png")
        };
        let chart_src = chart.chart_src;
        let upload = client.files_upload_v2(
            bot_token,
            channel_id,
            Some(thread_ts),
            &filename,
            chart.png_bytes,
            Some(&title),
            "image/png",
        );
        match tokio::time::timeout(FILE_UPLOAD_TIMEOUT, upload).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                tracing::warn!(chart_src = %chart_src, "files.uploadV2 failed: {e}");
                failures += 1;
            }
            Err(_) => {
                tracing::warn!(
                    chart_src = %chart_src,
                    timeout_secs = FILE_UPLOAD_TIMEOUT.as_secs(),
                    "files.uploadV2 timed out"
                );
                failures += 1;
            }
        }
    }

    failures
}

/// Post follow-up context messages for chart upload failures and SQL
/// artifact overflow. Errors are logged + swallowed.
async fn post_overflow_followups(
    client: &SlackClient,
    bot_token: &str,
    channel_id: &str,
    thread_ts: &str,
    upload_failures: usize,
    sql_overflow: usize,
    view_sql: Option<(Uuid, usize)>,
    thread_url: Option<&str>,
    agent_errored: bool,
) {
    if agent_errored {
        return;
    }

    if upload_failures > 0
        && let Some(url) = thread_url
    {
        let label = if upload_failures == 1 {
            "⚠️ Chart upload failed — view in Oxygen →".to_string()
        } else {
            format!("⚠️ {upload_failures} chart uploads failed — view in Oxygen →")
        };
        let blocks = serde_json::json!([{
            "type": "context",
            "elements": [{"type": "mrkdwn", "text": format!("<{url}|{label}>")}],
        }]);
        if let Err(e) = client
            .chat_post_message_with_blocks(
                bot_token,
                channel_id,
                "Some charts couldn't be uploaded",
                Some(thread_ts),
                Some(blocks),
            )
            .await
        {
            tracing::warn!("upload-failure follow-up post failed: {e}");
        }
    }

    if sql_overflow > 0
        && view_sql.is_some()
        && let Some(url) = thread_url
    {
        let plural = if sql_overflow == 1 {
            "query"
        } else {
            "queries"
        };
        let label = format!("📎 {sql_overflow} more {plural} — view in Oxygen →");
        let blocks = serde_json::json!([{
            "type": "context",
            "elements": [{"type": "mrkdwn", "text": format!("<{url}|{label}>")}],
        }]);
        if let Err(e) = client
            .chat_post_message_with_blocks(
                bot_token,
                channel_id,
                "More SQL queries are available in Oxygen",
                Some(thread_ts),
                Some(blocks),
            )
            .await
        {
            tracing::warn!("sql cap-overflow follow-up post failed: {e}");
        }
    }
}

// Setup helpers

async fn set_loading_status(
    client: &SlackClient,
    bot_token: &str,
    channel_id: &str,
    thread_ts: &str,
) {
    if let Err(e) = client
        .assistant_threads_set_status(
            bot_token,
            channel_id,
            thread_ts,
            "is working on your request…",
            Some(crate::render::LOADING_MESSAGES),
        )
        .await
    {
        tracing::warn!(
            channel = %channel_id,
            thread_ts = %thread_ts,
            "assistant.threads.setStatus failed: {e}"
        );
    }
}

async fn resolve_reopen_query(org_id: Uuid, question: &str) -> Option<String> {
    match crate::resolution::workspace_agent::count_org_workspaces(org_id).await {
        Ok(n) if n > 1 => {
            Some(base64::engine::general_purpose::STANDARD.encode(question.as_bytes()))
        }
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(
                org_id = %org_id,
                "count_org_workspaces failed, hiding reopen-picker button: {e}"
            );
            None
        }
    }
}

// URL helpers

async fn fetch_org_slug(org_id: Uuid) -> Option<String> {
    let conn = establish_connection().await.ok()?;
    entity::prelude::Organizations::find_by_id(org_id)
        .one(&conn)
        .await
        .ok()
        .flatten()
        .map(|o| o.slug)
}

async fn build_thread_url(
    base_url: &str,
    org_id: Uuid,
    workspace_id: Uuid,
    oxy_thread_id: Uuid,
) -> String {
    let url = match fetch_org_slug(org_id).await {
        Some(slug) => {
            format!("{base_url}/{slug}/workspaces/{workspace_id}/threads/{oxy_thread_id}")
        }
        None => format!("{base_url}/threads/{oxy_thread_id}"),
    };
    debug_assert!(
        !url.contains('|'),
        "thread URL must not contain '|' — Slack mrkdwn `<url|label>` has no escape: {url}"
    );
    url.replace('|', "")
}

// Thread + persistence

async fn find_or_create_slack_thread(
    req: &SlackRunRequest,
) -> Result<(entity::slack_threads::Model, Uuid, bool), OxyError> {
    if let Some(mut row) =
        ThreadContextService::find(req.installation.id, &req.channel_id, &req.thread_ts).await?
    {
        if row.workspace_id != req.workspace_id || row.agent_path != req.agent_path {
            ThreadContextService::update_workspace(row.id, req.workspace_id, &req.agent_path)
                .await?;
            row.workspace_id = req.workspace_id;
            row.agent_path = req.agent_path.clone();
        }
        let oxy_thread_id = row.oxy_thread_id;
        return Ok((row, oxy_thread_id, false));
    }

    let title = format!("Slack: {}", truncate(&req.question, 50));
    let oxy_thread_id = create_oxy_thread(
        req.workspace_id,
        req.user_link.oxy_user_id,
        &title,
        &req.question,
        &req.agent_path,
    )
    .await?;

    let row = ThreadContextService::create(CreateThreadContext {
        installation_id: req.installation.id,
        slack_channel_id: req.channel_id.clone(),
        slack_thread_ts: req.thread_ts.clone(),
        workspace_id: req.workspace_id,
        agent_path: req.agent_path.clone(),
        oxy_thread_id,
        initiated_by_user_link_id: Some(req.user_link.id),
    })
    .await?;

    Ok((row, oxy_thread_id, true))
}

async fn create_oxy_thread(
    workspace_id: Uuid,
    user_id: Uuid,
    title: &str,
    input: &str,
    agent_path: &str,
) -> Result<Uuid, OxyError> {
    let conn = establish_connection().await?;
    let new_thread = entity::threads::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        user_id: ActiveValue::Set(Some(user_id)),
        created_at: ActiveValue::NotSet,
        title: ActiveValue::Set(title.to_string()),
        input: ActiveValue::Set(input.to_string()),
        output: ActiveValue::Set(String::new()),
        source_type: ActiveValue::Set("agent".to_string()),
        source: ActiveValue::Set(agent_path.to_string()),
        references: ActiveValue::Set("[]".to_string()),
        is_processing: ActiveValue::Set(true),
        project_id: ActiveValue::Set(workspace_id),
        sandbox_info: ActiveValue::Set(None),
    };
    let thread = new_thread
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
    Ok(thread.id)
}

async fn persist_user_message(thread_id: Uuid, content: &str) -> Result<Uuid, OxyError> {
    persist_message(thread_id, content, true).await
}

async fn persist_agent_message(thread_id: Uuid, content: &str) -> Result<Uuid, OxyError> {
    persist_message(thread_id, content, false).await
}

async fn persist_message(thread_id: Uuid, content: &str, is_human: bool) -> Result<Uuid, OxyError> {
    let conn = establish_connection().await?;
    let message_id = Uuid::new_v4();
    let new_message = entity::messages::ActiveModel {
        id: ActiveValue::Set(message_id),
        thread_id: ActiveValue::Set(thread_id),
        content: ActiveValue::Set(content.to_string()),
        is_human: ActiveValue::Set(is_human),
        created_at: ActiveValue::NotSet,
        input_tokens: ActiveValue::Set(0),
        output_tokens: ActiveValue::Set(0),
    };
    new_message
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
    Ok(message_id)
}

async fn update_thread_with_output(thread_id: Uuid, output: &str) -> Result<(), OxyError> {
    let conn = establish_connection().await?;
    let thread = entity::prelude::Threads::find_by_id(thread_id)
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?
        .ok_or_else(|| OxyError::DBError("Thread not found".to_string()))?;

    let mut active_thread: entity::threads::ActiveModel = thread.into();
    active_thread.output = ActiveValue::Set(output.to_string());
    active_thread.is_processing = ActiveValue::Set(false);
    active_thread
        .update(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
    Ok(())
}

// Utilities

fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        return s.to_string();
    }
    let boundary = s
        .char_indices()
        .nth(max_len)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..boundary])
}

fn chart_label_from_question(question: &str) -> String {
    let trimmed = question.trim();
    if trimmed.is_empty() {
        return "Chart".to_string();
    }
    truncate(trimmed, 80)
}

fn chart_filename_stem_from_question(question: &str) -> String {
    let mut out = String::with_capacity(question.len());
    let mut last_was_dash = false;
    for ch in question.chars() {
        if ch.is_ascii_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        return "chart".to_string();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 60 {
        return trimmed.to_string();
    }
    chars[..60]
        .iter()
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// Filename-safe slug for artifact titles. Preserves `_` (semantic-query
/// tool names use it heavily — `query_retail_analytics`) and collapses
/// any other non-alphanumeric run to a single `_`. Falls back to `query`
/// for an empty input.
pub(crate) fn sanitize_filename(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_underscore = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
            last_was_underscore = ch == '_';
        } else if !last_was_underscore && !out.is_empty() {
            out.push('_');
            last_was_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        return "query".to_string();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 60 {
        return trimmed.to_string();
    }
    chars[..60]
        .iter()
        .collect::<String>()
        .trim_end_matches('_')
        .to_string()
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate;

    #[test]
    fn passes_through_short_ascii() {
        assert_eq!(truncate("hi", 50), "hi");
    }

    #[test]
    fn truncates_long_ascii() {
        assert_eq!(truncate("abcdefghij", 5), "abcde...");
    }

    #[test]
    fn truncates_multibyte_utf8_without_panic() {
        let input = "🙂🙂🙂🙂🙂";
        let out = truncate(input, 2);
        assert_eq!(out, "🙂🙂...");
    }

    #[test]
    fn truncates_cjk_at_character_boundary() {
        assert_eq!(truncate("你好世界", 2), "你好...");
    }

    #[test]
    fn max_len_equal_to_char_count_returns_original() {
        assert_eq!(truncate("你好", 2), "你好");
    }
}

#[cfg(test)]
mod sanitize_filename_tests {
    use super::sanitize_filename;

    #[test]
    fn preserves_underscores_in_semantic_query_names() {
        assert_eq!(
            sanitize_filename("query_retail_analytics"),
            "query_retail_analytics"
        );
    }

    #[test]
    fn slugifies_titles_with_punctuation_and_spaces() {
        assert_eq!(
            sanitize_filename("Top Stores — Weekly Sales"),
            "Top_Stores_Weekly_Sales"
        );
    }

    #[test]
    fn alphanumerics_pass_through_unchanged() {
        assert_eq!(sanitize_filename("abc123"), "abc123");
    }

    #[test]
    fn falls_back_to_query_for_empty_or_punctuation_only() {
        assert_eq!(sanitize_filename(""), "query");
        assert_eq!(sanitize_filename("   "), "query");
        assert_eq!(sanitize_filename("!!!"), "query");
    }
}
