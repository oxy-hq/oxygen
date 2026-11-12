use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use crate::server::runtime_artifact;
use axum::extract::{self, Path};
use axum::http::StatusCode;
use uuid::Uuid;

/// Serve a legacy visualize-task chart file (the `:chart{chart_src=NAME}`
/// directive renders by fetching this). The file is written to the ide's local
/// disk by the run, so this route is `IdeOnly` — but it is **S3-mirrored for
/// ide-down resilience**, matching the dashboard/app PNG charts that moved to S3
/// earlier (this route was the one left behind):
///
/// - When the ide serves a chart it mirrors the file to S3 (best-effort, no-op
///   without a bucket), so the object exists for cross-node reads.
/// - The route is `FleetOk` (`route_fleet` in `router/workspace.rs`), so a serve
///   replica runs this handler itself, misses on disk, and serves the S3 mirror
///   — a chart you've already viewed keeps loading while Oxygen Factory
///   restarts, instead of a hard 502.
///
/// Modern agentic-analytics charts are Postgres `DisplayBlock`s and never reach
/// this route; they already render on any replica.
pub async fn get_chart(
    Path((workspace_id, file_path)): Path<(Uuid, String)>,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<extract::Json<String>, StatusCode> {
    // Chart files are flat names; reject any separator/traversal before using
    // `file_path` as an S3 object key. `None` => skip the S3 paths (defensive;
    // such a request can't match a real chart anyway).
    let safe_key = (!file_path.contains('/') && !file_path.contains(".."))
        .then(|| runtime_artifact::chart_key(workspace_id, &file_path));

    // Fast path: this pod wrote the chart, or shares a volume with whoever did.
    // No mirror here — the writer mirrors (`oxy_api_slack::chart_render::write_chart_json`),
    // which is what makes the S3 fallback below reliable enough for this route to
    // leave the singleton.
    let local = workspace_manager
        .config_manager
        .charts_dir()
        .join(&file_path);
    if let Ok(content) = tokio::fs::read_to_string(&local).await {
        return Ok(extract::Json(content));
    }

    // Cross-node / ide-down fallback: no local file — read the S3 mirror the ide
    // wrote when it last served this chart.
    if let Some(key) = safe_key
        && let Some(bytes) = runtime_artifact::fetch(&key).await
        && let Ok(content) = String::from_utf8(bytes)
    {
        return Ok(extract::Json(content));
    }

    Err(StatusCode::NOT_FOUND)
}
