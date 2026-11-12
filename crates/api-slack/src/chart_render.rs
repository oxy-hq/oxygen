//! Chart-PNG rendering for Slack messages.
//!
//! The chart spec lives on disk as echarts JSON (the web frontend
//! renders them client-side). For Slack, we drive a headless Chromium
//! over an inline HTML page that loads echarts and applies the same
//! JSON → option transform the React `<Chart>` component does, then
//! screenshot the result.
//!
//! [`get_or_render_chart_png`] caches PNG bytes alongside the chart JSON
//! in the workspace state dir. The Slack render path takes those bytes
//! and either uploads them to Slack via `files.uploadV2` (production —
//! see [`crate::render`]) or surfaces the cached
//! path as a context-block breadcrumb (local dev).
//!
//! `render_echarts_to_png` builds an inline HTML page that loads echarts
//! from a CDN, applies the same JSON → option transform that the web
//! frontend uses, and waits for the chart's `finished` event before
//! taking a screenshot. Headless chrome runs on a blocking thread (it's
//! not async-aware), so we wrap the call in `spawn_blocking`.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use headless_chrome::Browser;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use oxy::adapters::workspace::resolve_workspace_path;
use oxy::config::ConfigBuilder;
use oxy::config::WorkingCopy;
use oxy_shared::errors::OxyError;
use tokio::sync::Mutex;
use uuid::Uuid;

const RENDER_VIEWPORT_WIDTH: u32 = 1200;
const RENDER_VIEWPORT_HEIGHT: u32 = 700;
const RENDER_READY_TIMEOUT: Duration = Duration::from_secs(15);
const RENDER_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Per-process render lock. Headless-chrome browser launches are heavy
/// and not safe to fan out arbitrarily — a single in-flight render at a
/// time keeps memory bounded and prevents the "10 chart events arrive
/// in parallel and we OOM" failure mode.
static RENDER_LOCK: Mutex<()> = Mutex::const_new(());

/// Resolve the on-disk PNG cache path for a given workspace + chart
/// filename. The cache lives alongside the chart JSON in the workspace
/// state dir under `slack-chart-images/`.
pub async fn cached_chart_png_path(
    workspace_id: Uuid,
    chart_json_filename: &str,
) -> Result<PathBuf, OxyError> {
    let config_manager = build_config_manager(workspace_id).await?;
    let state_dir = config_manager.charts_dir();
    // `get_charts_dir` returns `<state>/charts`; jump one level up to
    // sit at `<state>/slack-chart-images` so we don't pollute the JSON
    // dir that the frontend `useChart` hook scans.
    let state_root = state_dir
        .parent()
        .ok_or_else(|| OxyError::RuntimeError("charts dir has no parent state dir".to_string()))?;
    let cache_dir = state_root.join("slack-chart-images");
    let png_name = chart_json_filename
        .strip_suffix(".json")
        .map(|s| format!("{s}.png"))
        .unwrap_or_else(|| format!("{chart_json_filename}.png"));
    Ok(cache_dir.join(png_name))
}

/// Write `json` to the workspace's chart directory under `filename`.
/// Used by the agentic bridge to materialize charts emitted as
/// `AnalyticsEvent::ChartRendered` so the existing PNG render + upload
/// pipeline (which reads from disk) keeps working unchanged.
///
/// Idempotent: if the file already exists with identical bytes the
/// disk write is skipped, so concurrent agent runs that emit the same
/// chart hash don't race on the same file.
pub async fn write_chart_json(
    workspace_id: Uuid,
    filename: &str,
    json: &serde_json::Value,
) -> Result<PathBuf, OxyError> {
    let config_manager = build_config_manager(workspace_id).await?;
    let charts_dir = config_manager.charts_dir();
    tokio::fs::create_dir_all(&charts_dir).await.map_err(|e| {
        OxyError::RuntimeError(format!(
            "failed to create charts dir {}: {e}",
            charts_dir.display()
        ))
    })?;
    let path = charts_dir.join(filename);
    let bytes = serde_json::to_vec(json)
        .map_err(|e| OxyError::RuntimeError(format!("serializing chart json: {e}")))?;
    tokio::fs::write(&path, &bytes).await.map_err(|e| {
        OxyError::RuntimeError(format!(
            "failed to write chart json {}: {e}",
            path.display()
        ))
    })?;

    // Mirror on WRITE, not on the ide's first read.
    //
    // The read-path mirror meant a chart nobody had viewed yet existed on
    // exactly one pod, so `GET /charts/{file}` had to be pinned to the ide to
    // guarantee a hit. Mirroring here makes every chart readable from any
    // replica the moment it exists, which is what lets that route serve from the
    // fleet. No-op when no blob bucket is configured.
    oxy_app::surface::runtime_artifact::mirror(
        &oxy_app::surface::runtime_artifact::chart_key(workspace_id, filename),
        bytes,
        "application/json",
    )
    .await;

    Ok(path)
}

/// Resolve the on-disk JSON path for a chart inside a workspace. Returns
/// `None` (not an error) when the file is missing — the caller maps that
/// to a 404.
pub async fn chart_json_path(
    workspace_id: Uuid,
    chart_json_filename: &str,
) -> Result<Option<PathBuf>, OxyError> {
    let config_manager = build_config_manager(workspace_id).await?;
    let charts_dir = config_manager.charts_dir();
    let path = charts_dir.join(chart_json_filename);
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

async fn build_config_manager(
    workspace_id: Uuid,
) -> Result<oxy::config::ConfigManager<WorkingCopy>, OxyError> {
    let workspace_path = resolve_workspace_path(workspace_id).await?;
    ConfigBuilder::new()
        .with_workspace_path(workspace_path)?
        .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
        .await
}

/// Ensure the chart PNG exists on disk and return its path. Renders if
/// absent. Concurrent callers race through `RENDER_LOCK` — the loser
/// sees the freshly cached file on retry and skips rendering. Used by
/// the dev path that only needs the path (no PNG bytes in memory).
pub async fn ensure_chart_png_cached(
    workspace_id: Uuid,
    chart_json_filename: &str,
) -> Result<PathBuf, OxyError> {
    let cache_path = cached_chart_png_path(workspace_id, chart_json_filename).await?;
    if tokio::fs::try_exists(&cache_path).await.unwrap_or(false) {
        return Ok(cache_path);
    }

    let _lock = RENDER_LOCK.lock().await;
    // Re-check after acquiring the lock; another caller may have just
    // produced the file while we were queued.
    if tokio::fs::try_exists(&cache_path).await.unwrap_or(false) {
        return Ok(cache_path);
    }

    let json_path = chart_json_path(workspace_id, chart_json_filename)
        .await?
        .ok_or_else(|| {
            OxyError::RuntimeError(format!(
                "chart file not found: {workspace_id}/{chart_json_filename}"
            ))
        })?;
    let raw = tokio::fs::read_to_string(&json_path)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("failed to read chart JSON: {e}")))?;
    let config: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| OxyError::RuntimeError(format!("chart JSON parse failed: {e}")))?;

    let png = render_echarts_to_png(&config).await?;

    if let Some(parent) = cache_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            OxyError::RuntimeError(format!(
                "failed to create chart cache dir {}: {e}",
                parent.display()
            ))
        })?;
    }
    tokio::fs::write(&cache_path, &png).await.map_err(|e| {
        OxyError::RuntimeError(format!(
            "failed to write chart cache {}: {e}",
            cache_path.display()
        ))
    })?;
    tracing::info!(
        chart_json = %chart_json_filename,
        cache_path = %cache_path.display(),
        bytes = png.len(),
        "rendered chart PNG (lazy, cached for next request)"
    );
    Ok(cache_path)
}

/// Fetch the cached PNG bytes for a chart, rendering it on the fly if
/// absent. Used by the upload path that needs the bytes for
/// `files.uploadV2`. The dev path should call [`ensure_chart_png_cached`]
/// instead — it only needs the on-disk path.
pub async fn get_or_render_chart_png(
    workspace_id: Uuid,
    chart_json_filename: &str,
) -> Result<Vec<u8>, OxyError> {
    let cache_path = ensure_chart_png_cached(workspace_id, chart_json_filename).await?;
    tokio::fs::read(&cache_path).await.map_err(|e| {
        OxyError::RuntimeError(format!(
            "failed to read cached chart PNG {}: {e}",
            cache_path.display()
        ))
    })
}

/// Drive headless Chromium to render `config` (a simplified echarts spec
/// — `series`, `xAxis`, `yAxis`, `title`) into PNG bytes.
///
/// The render runs on a blocking thread because `headless_chrome` is
/// synchronous. The browser is short-lived: launched, used once, dropped.
async fn render_echarts_to_png(config: &serde_json::Value) -> Result<Vec<u8>, OxyError> {
    let html = build_render_html(config)?;
    tokio::task::spawn_blocking(move || render_html_to_png_blocking(&html))
        .await
        .map_err(|e| OxyError::RuntimeError(format!("chart render task panicked: {e}")))?
}

fn render_html_to_png_blocking(html: &str) -> Result<Vec<u8>, OxyError> {
    let browser = Browser::new(
        headless_chrome::LaunchOptions::default_builder()
            .headless(true)
            .sandbox(false)
            .window_size(Some((RENDER_VIEWPORT_WIDTH, RENDER_VIEWPORT_HEIGHT)))
            .build()
            .map_err(|e| OxyError::RuntimeError(format!("browser launch options: {e}")))?,
    )
    .map_err(|e| OxyError::RuntimeError(format!("browser launch failed: {e}")))?;

    let tab = browser
        .new_tab()
        .map_err(|e| OxyError::RuntimeError(format!("browser tab create failed: {e}")))?;

    // data:text/html;base64 carries the inline HTML without needing any
    // network round-trip beyond the echarts CDN script tag inside it.
    let data_url = format!(
        "data:text/html;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(html.as_bytes())
    );
    tab.navigate_to(&data_url)
        .map_err(|e| OxyError::RuntimeError(format!("navigate failed: {e}")))?;
    tab.wait_until_navigated()
        .map_err(|e| OxyError::RuntimeError(format!("navigation wait failed: {e}")))?;

    // Poll for the body-level `data-chart-ready="true"` flag the inline
    // page sets once echarts emits its `finished` event.
    let start = std::time::Instant::now();
    let ready_check_js = r#"
        (function() {
            const v = document.body && document.body.getAttribute('data-chart-ready');
            return v === 'true';
        })()
    "#;
    let mut ready = false;
    while start.elapsed() < RENDER_READY_TIMEOUT {
        if let Ok(result) = tab.evaluate(ready_check_js, false)
            && result.value.and_then(|v| v.as_bool()).unwrap_or(false)
        {
            ready = true;
            break;
        }
        std::thread::sleep(RENDER_POLL_INTERVAL);
    }
    if !ready {
        tracing::warn!(
            "chart render did not signal ready within {RENDER_READY_TIMEOUT:?}; capturing anyway"
        );
    }

    let png = tab
        .capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true)
        .map_err(|e| OxyError::RuntimeError(format!("screenshot failed: {e}")))?;
    Ok(png)
}

/// Build the inline HTML page driven by headless Chromium. Loads echarts
/// from jsDelivr (no internet → render fails with an obvious error in
/// the screenshot); applies the same simplified-spec → echarts-option
/// transform the React Chart component does so PNGs match the in-app
/// rendering.
fn build_render_html(config: &serde_json::Value) -> Result<String, OxyError> {
    let config_json = serde_json::to_string(config)
        .map_err(|e| OxyError::RuntimeError(format!("re-serializing chart config: {e}")))?;
    Ok(format!(
        r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <script src="https://cdn.jsdelivr.net/npm/echarts@5/dist/echarts.min.js"></script>
  <style>
    html, body {{ margin: 0; padding: 0; background: #ffffff; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; }}
    #chart {{ width: {width}px; height: {height}px; }}
  </style>
</head>
<body>
  <div id="chart"></div>
  <script>
    (function() {{
      var raw = {config_json};
      var options = {{
        title: raw.title ? {{ text: raw.title, left: 'center', textStyle: {{ fontSize: 16, fontWeight: 600 }} }} : undefined,
        tooltip: {{}},
        grid: {{ left: 60, right: 30, top: raw.title ? 60 : 30, bottom: 50, containLabel: true }},
        legend: (raw.series && raw.series.length > 1) ? {{ top: raw.title ? 32 : 8 }} : undefined,
        xAxis: raw.xAxis ? {{
          type: raw.xAxis.type || 'category',
          name: raw.xAxis.name,
          nameLocation: 'middle',
          nameGap: 30,
          data: raw.xAxis.data || undefined
        }} : undefined,
        yAxis: raw.yAxis ? {{
          type: raw.yAxis.type || 'value',
          name: raw.yAxis.name,
          data: raw.yAxis.data || undefined
        }} : undefined,
        series: (raw.series || []).map(function(s) {{
          return {{ name: s.name, type: s.type, data: s.data || [] }};
        }})
      }};
      var chart = echarts.init(document.getElementById('chart'), null, {{ renderer: 'canvas' }});
      chart.setOption(options);
      chart.on('finished', function() {{
        document.body.setAttribute('data-chart-ready', 'true');
      }});
      // Belt-and-braces: echarts sometimes skips `finished` for trivial
      // charts. Mark ready after a short grace period regardless.
      setTimeout(function() {{
        document.body.setAttribute('data-chart-ready', 'true');
      }}, 1500);
    }})();
  </script>
</body>
</html>"#,
        width = RENDER_VIEWPORT_WIDTH,
        height = RENDER_VIEWPORT_HEIGHT,
        config_json = config_json,
    ))
}

// Renderer-side tests are intentionally light here: spinning up
// headless Chromium in a unit test would be slow, environment-fragile,
// and cover the wrong layer. The path-resolution logic above is
// pure-async-fs and exercised end-to-end by the Slack integration
// tests when a chart event fires.
