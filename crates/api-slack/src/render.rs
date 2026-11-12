//! `SlackRenderer` — [`oxy::ClientRenderer`] impl for Slack.
//!
//! Prose is buffered, not streamed: live token reveal looks frozen during
//! tool calls (the bubble shows "Let me check…" while SQL runs for 10s).
//! The `loading_messages` rotation via `setStatus` carries the "alive"
//! signal; `chat.postMessage` reveals the full prose at the end.
//!
//! Charts: render PNG bytes during streaming and queue them. The actual
//! upload happens *after* `chat.postMessage` posts the prose, so the text
//! answer always reaches the thread first and the auto-shared chart files
//! land as thread replies underneath. Embedding charts as `slack_file`
//! blocks inside the prose message looked tidier but turned out to be
//! brittle — Slack rejects the message when the file isn't yet visible
//! to the channel — so we use the same "post text, then auto-share" shape
//! Datadog and Grafana use. See `events::execution::run_for_slack` for
//! the orchestration. When `OXY_SLACK_CHART_UPLOAD` is unset (the dev
//! default), uploads are skipped and PNGs surface as on-disk paths in a
//! footer context block — Slack can't fetch localhost so there's no
//! inline preview, but devs can `open` the file.
//!
//! Reasoning events are dropped — Slack has no collapsible-reasoning UI.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use oxy::ClientRenderer;
use oxy::exec_types::Usage;
use oxy::exec_types::event::{ArtifactKind, Step};
use oxy::types::ArtifactValue;
use uuid::Uuid;

use crate::chart_render::{ensure_chart_png_cached, get_or_render_chart_png};
use crate::client::SlackClient;
use crate::events::artifact_filter::ArtifactFilter;

/// One SQL-bearing artifact captured by `SlackRenderer`. Built for both
/// `ExecuteSQL` (sql is on the kind itself) and `SemanticQuery` (sql
/// arrives later via `on_artifact_value`). `events::execution` consumes
/// these and uploads each as a `.sql` thread reply via `files.uploadV2`
/// after the prose message lands — Slack renders the snippet collapsed
/// with native SQL syntax highlighting. Other artifact kinds (`OmniQuery`,
/// `LookerQuery`, `Workflow`, `Agent`, `SandboxApp`) are deliberately
/// not captured today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSqlArtifact {
    pub title: String,
    pub sql: String,
    pub database: String,
    pub is_verified: bool,
}

/// Status messages Slack rotates as the AI-assistant indicator. Pair with
/// a non-empty `status` string in `setStatus` — an empty string clears
/// the indicator. We keep 5 entries: half the original 10-message list,
/// enough motion to read as "alive" without the shuffly carousel.
///
/// Verbs are deliberately neutral and product-flavored — they describe
/// what an Oxy agent generally does (query, read, analyze, compile,
/// draft) rather than committing to specific outputs (e.g. "Visualizing…"
/// would be misleading on runs that don't produce charts). Slack rotates
/// these on its own timer with no relation to actual agent state, so
/// they're ambient motion only — the plan-mode task block carries the
/// detailed progress signal. "Thinking…" is deliberately absent (Slack
/// uses it as the fallback word).
pub const LOADING_MESSAGES: &[&str] = &[
    "Querying data…",
    "Reading results…",
    "Analyzing…",
    "Compiling insights…",
    "Drafting answer…",
];

/// PNG bytes for a chart, queued during streaming and uploaded to Slack
/// after the prose `chat.postMessage` has landed. `chart_src` is the
/// source JSON filename — used to derive the Slack file display name.
pub struct QueuedChart {
    pub chart_src: String,
    pub png_bytes: Vec<u8>,
}

/// Output of a Slack render run.
///
/// * `body` — accumulated prose markdown.
/// * `queued_charts` — PNG bytes ready to upload. The caller (in
///   `events::execution`) drives the upload after the prose message
///   posts, so the text appears first in the thread.
/// * `chart_local_paths` — on-disk PNGs surfaced for dev inspection
///   when `OXY_SLACK_CHART_UPLOAD` is unset.
/// * `failed_chart_count` — render-side failures only (Chromium crash,
///   missing JSON). Upload-side failures live in a separate counter on
///   the orchestration side in `events::execution` so each surface gets
///   its own warning footer.
/// * `captured_sql_artifacts` — `ExecuteSQL` and `SemanticQuery`
///   artifacts captured from the agent stream. The caller in
///   `events::execution` uploads each as a `.sql` thread reply via
///   `files.uploadV2` after the prose message lands; Slack renders
///   them as collapsible snippets with SQL syntax highlighting.
pub struct SlackRenderResult {
    pub body: String,
    pub queued_charts: Vec<QueuedChart>,
    pub chart_local_paths: Vec<PathBuf>,
    pub failed_chart_count: usize,
    pub captured_sql_artifacts: Vec<CapturedSqlArtifact>,
}

pub struct SlackRenderer<'a> {
    client: &'a SlackClient,
    bot_token: &'a str,
    channel: &'a str,
    thread_ts: &'a str,
    workspace_id: Uuid,
    /// When `true`, queued PNG bytes are uploaded to Slack after the
    /// prose message lands. When `false`, PNGs render to disk only and
    /// the path is surfaced as a context-block breadcrumb — the dev
    /// path that doesn't depend on a real Slack workspace.
    upload_charts: bool,

    body: String,
    artifact_filter: ArtifactFilter,
    queued_charts: Vec<QueuedChart>,
    chart_local_paths: Vec<PathBuf>,
    /// Charts that never made it past the render step (Chromium crash,
    /// missing chart JSON). Upload-side failures live on the
    /// orchestration side in `events::execution`.
    failed_chart_count: usize,
    /// SQL-bearing artifacts captured for upload as `.sql` thread replies
    /// (via `files.uploadV2` after the prose message lands). Populated
    /// for `ExecuteSQL` (sql arrives on the kind when pre-configured in
    /// YAML, or later via `on_artifact_value` for LLM-generated SQL) and
    /// for `SemanticQuery` (sql always arrives later via `on_artifact_value`).
    captured_sql_artifacts: Vec<CapturedSqlArtifact>,
    /// Tracks SQL artifacts that have started but whose compiled SQL hasn't
    /// arrived yet. Keyed by artifact id; populated in `on_artifact_started`
    /// and drained in `on_artifact_value` when the matching populated value
    /// event lands. Covers `SemanticQuery` (kind has no SQL) and the
    /// LLM-generated `ExecuteSQL` path (kind carries `sql=""` when the
    /// agent decides the query at runtime). Other kinds where the SQL
    /// arrives via the value path (e.g. LookerQuery) can be added here
    /// without changing the rendering pipeline.
    pending_sql_artifacts: HashMap<String, (String, bool)>,
}

impl<'a> SlackRenderer<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: &'a SlackClient,
        bot_token: &'a str,
        channel: &'a str,
        thread_ts: &'a str,
        thread_url: Option<String>,
        workspace_id: Uuid,
        upload_charts: bool,
    ) -> Self {
        let artifact_filter = match thread_url {
            Some(url) => ArtifactFilter::with_thread_url(url),
            None => ArtifactFilter::new(),
        };
        Self {
            client,
            bot_token,
            channel,
            thread_ts,
            workspace_id,
            upload_charts,
            body: String::new(),
            artifact_filter,
            queued_charts: Vec::new(),
            chart_local_paths: Vec::new(),
            failed_chart_count: 0,
            captured_sql_artifacts: Vec::new(),
            pending_sql_artifacts: HashMap::new(),
        }
    }
}

#[async_trait]
impl<'a> ClientRenderer for SlackRenderer<'a> {
    type Output = SlackRenderResult;

    async fn on_text(&mut self, content: &str) {
        // Filter strips `:::artifact{…}` directives and emits inline
        // subtext; everything else passes through. Accumulate only —
        // see file-level doc for why we don't live-stream prose.
        let filtered = self.artifact_filter.feed(content);
        if filtered.is_empty() {
            return;
        }
        self.body.push_str(&filtered);
    }

    // Reasoning dropped: Slack has no collapsible UI, and toggling the
    // status indicator on every span caused visible flicker.
    async fn on_reasoning_started(&mut self, _id: &str) {}
    async fn on_reasoning_chunk(&mut self, _id: &str, _delta: &str) {}
    async fn on_reasoning_done(&mut self, _id: &str) {}

    async fn on_chart(&mut self, chart_src: &str) {
        // Render failures are rare (missing JSON file, Chromium crash) and
        // a single chart that fails to render shouldn't kill the run —
        // the agent's text answer still posts.
        if self.upload_charts {
            // Upload path: render to bytes and queue for upload after the
            // prose message lands. The bytes also sit on disk in the
            // cache dir — useful for debugging if a Slack upload fails.
            match get_or_render_chart_png(self.workspace_id, chart_src).await {
                Ok(bytes) => self.queued_charts.push(QueuedChart {
                    chart_src: chart_src.to_string(),
                    png_bytes: bytes,
                }),
                Err(e) => {
                    tracing::warn!(chart_src, "chart render failed: {e}");
                    self.failed_chart_count += 1;
                }
            }
            return;
        }

        // Dev path: render to disk only, surface the on-disk path so the
        // developer can `open` the file. Slack won't fetch localhost so
        // this never produces an inline preview — it's a breadcrumb, not
        // a real image block. Avoids loading the PNG bytes into memory
        // since we don't need them.
        match ensure_chart_png_cached(self.workspace_id, chart_src).await {
            Ok(path) => {
                tracing::info!(
                    chart_src,
                    png_path = %path.display(),
                    "rendered chart PNG locally for validation"
                );
                self.chart_local_paths.push(path);
            }
            Err(e) => {
                tracing::warn!(chart_src, "chart render failed: {e}");
                self.failed_chart_count += 1;
            }
        }
    }

    async fn on_artifact_started(
        &mut self,
        id: &str,
        title: &str,
        kind: &ArtifactKind,
        is_verified: bool,
    ) {
        // The inline subtext is still emitted via `on_text` → `ArtifactFilter`.
        // Here we additionally capture SQL-bearing artifacts so `execution.rs`
        // can upload each one as a `.sql` thread reply via `files.uploadV2`
        // after the prose message lands.
        match kind {
            // `ExecuteSQL` carries `sql` and `database` on the kind, BUT the
            // sql is only populated when the YAML pre-configures the query
            // (via `sql:` in the tool config). For LLM-generated execute_sql
            // calls — the common case — the kind arrives with `sql=""` and
            // the actual query lands later via `ArtifactValue::ExecuteSQL`.
            // Capture immediately if we already have it; otherwise stash
            // and let `on_artifact_value` fill in the SQL.
            ArtifactKind::ExecuteSQL { sql, database } if !sql.is_empty() => {
                tracing::info!(
                    artifact_id = id,
                    title = title,
                    "slack renderer: captured ExecuteSQL artifact (sql on kind)"
                );
                self.captured_sql_artifacts.push(CapturedSqlArtifact {
                    title: title.to_string(),
                    sql: sql.clone(),
                    database: database.clone(),
                    is_verified,
                });
            }
            ArtifactKind::ExecuteSQL { .. } => {
                tracing::info!(
                    artifact_id = id,
                    title = title,
                    "slack renderer: pending ExecuteSQL (awaiting value)"
                );
                self.pending_sql_artifacts
                    .insert(id.to_string(), (title.to_string(), is_verified));
            }
            // `SemanticQuery` kind is empty — the compiled SQL always arrives
            // later via `ArtifactValue::SemanticQuery`. Stash title + verified
            // flag keyed by artifact id; `on_artifact_value` pairs them up.
            ArtifactKind::SemanticQuery {} => {
                tracing::info!(
                    artifact_id = id,
                    title = title,
                    "slack renderer: pending SemanticQuery (awaiting value)"
                );
                self.pending_sql_artifacts
                    .insert(id.to_string(), (title.to_string(), is_verified));
            }
            // OmniQuery / LookerQuery / Workflow / Agent / SandboxApp are
            // not rendered as inline blocks — see the "Deferred kinds"
            // section in `internal-docs/2026-04-29-slack-artifact-rendering-design.md`.
            other => {
                tracing::info!(
                    artifact_id = id,
                    title = title,
                    kind = %other,
                    "slack renderer: artifact kind not captured"
                );
            }
        }
    }

    async fn on_artifact_value(&mut self, id: &str, value: &ArtifactValue) {
        // Both `ExecuteSQL` (LLM-generated path) and `SemanticQuery` tools
        // emit multiple value events for one artifact: an early placeholder
        // with `sql_query: ""` when the call begins, then later the
        // populated value once the SQL is generated/compiled. Only the
        // populated value is worth capturing — empty payloads are skipped
        // without touching the pending entry, so we don't lose the title
        // before the real SQL lands.
        //
        // Once we capture, `.remove(id)` drops the entry so later duplicate
        // populated events (the tool tends to re-emit a few times as the
        // query streams) won't push a second block. If the artifact ends
        // without a non-empty SQL (compile error, validation_error), the
        // entry is left in `pending_sql_artifacts` and naturally dropped
        // when the renderer goes out of scope at end of run.
        let (sql_query, database, kind_label) = match value {
            ArtifactValue::ExecuteSQL(es) => (&es.sql_query, &es.database, "ExecuteSQL"),
            ArtifactValue::SemanticQuery(sq) => (&sq.sql_query, &sq.database, "SemanticQuery"),
            _ => return,
        };
        tracing::info!(
            artifact_id = id,
            kind = kind_label,
            sql_len = sql_query.len(),
            pending_match = self.pending_sql_artifacts.contains_key(id),
            "slack renderer: received SQL artifact value"
        );
        if !sql_query.is_empty()
            && let Some((title, is_verified)) = self.pending_sql_artifacts.remove(id)
        {
            tracing::info!(
                artifact_id = id,
                title = %title,
                kind = kind_label,
                "slack renderer: captured SQL artifact from value"
            );
            self.captured_sql_artifacts.push(CapturedSqlArtifact {
                title,
                sql: sql_query.clone(),
                database: database.clone(),
                is_verified,
            });
        }
    }
    /// `error` is intentionally ignored — a query that compiled but
    /// failed at execution time is still worth uploading as a `.sql`
    /// snippet so the user can read what was attempted, copy-edit, and
    /// re-run. Surfacing the failure separately is the agent's job (it
    /// posts the error in prose); the renderer's contract is just to
    /// pass through whatever the artifact pipeline produced.
    async fn on_artifact_done(&mut self, _id: &str, _error: Option<&str>) {}

    // Step events are dropped: the run is presented as a single opaque
    // "thinking" period driven by the setStatus loading_messages rotation.
    // The final blocks land via chat.postMessage.
    async fn on_step_started(&mut self, _step: &Step) {}
    async fn on_step_finished(&mut self, _step_id: &str, _error: Option<&str>) {}

    async fn on_error(&mut self, _message: &str) {}

    async fn on_usage(&mut self, _usage: &Usage) {}

    async fn finalize(mut self) -> Self::Output {
        // Drain trailing artifact-filter state (partial fences, split
        // colons across chunks) into body before postMessage lands.
        let trailing = self.artifact_filter.finish();
        if !trailing.is_empty() {
            self.body.push_str(&trailing);
        }
        // Clear the loading indicator. Logged-and-swallowed; UX decoration.
        if let Err(e) = self
            .client
            .assistant_threads_set_status(self.bot_token, self.channel, self.thread_ts, "", None)
            .await
        {
            tracing::warn!("clear setStatus failed: {e}");
        }

        SlackRenderResult {
            body: self.body,
            queued_charts: std::mem::take(&mut self.queued_charts),
            chart_local_paths: self.chart_local_paths,
            failed_chart_count: self.failed_chart_count,
            captured_sql_artifacts: self.captured_sql_artifacts,
        }
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
