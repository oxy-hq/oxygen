//! UniFi device proxy for the world-model.
//!
//! Pokehouse's UniFi setup has 17 Protect consoles (one per store) with ~120
//! cameras total. The cloud-connector Protect REST API (`/v1/cameras`,
//! snapshot, RTSPS provisioning) is gated on host *ownership* — our API key
//! has visibility but not ownership, so we get 403 on those endpoints.
//!
//! The **Site Manager API** at `https://api.ui.com/v1/devices` is more
//! permissive: it returns device metadata for hosts where the caller is
//! "owner or super admin" — and this returns 200 with the full device list
//! for all 17 stores. So we surface real camera metadata (name, model,
//! status, IP, firmware) even without live-video access.
//!
//! Endpoint: `GET /api/{workspace_id}/world-model/cameras` returns the
//! filtered Protect camera list grouped by host. Cached for 60s
//! process-wide so the FE can poll freely without hammering UniFi.
//!
//! Reads the API key from `UNIFI_API_KEY` — the same env var used by
//! existing pokehouse-oxy `.env` files.

use axum::Json;
use axum::extract::Path as AxumPath;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::apps_helpers;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;

const UNIFI_LIST_DEVICES_URL: &str = "https://api.ui.com/v1/devices?pageSize=200";
const CACHE_TTL: Duration = Duration::from_secs(60);

// ── Upstream shape (trimmed) ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct UnifiListDevicesResponse {
    data: Vec<UnifiHost>,
}

#[derive(Debug, Deserialize)]
struct UnifiHost {
    #[serde(rename = "hostId")]
    host_id: String,
    #[serde(rename = "hostName")]
    host_name: Option<String>,
    #[serde(default)]
    devices: Vec<UnifiDevice>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnifiDevice {
    id: String,
    mac: Option<String>,
    name: Option<String>,
    model: Option<String>,
    shortname: Option<String>,
    ip: Option<String>,
    #[serde(default)]
    product_line: String,
    status: Option<String>,
    version: Option<String>,
    firmware_status: Option<String>,
    update_available: Option<String>,
    startup_time: Option<String>,
}

// ── FE-facing shape ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct WorldModelCamerasResponse {
    pub hosts: Vec<HostCameras>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostCameras {
    pub host_id: String,
    pub host_name: Option<String>,
    pub cameras: Vec<Camera>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Camera {
    pub id: String,
    pub mac: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub shortname: Option<String>,
    pub ip: Option<String>,
    pub status: Option<String>,
    pub firmware_status: Option<String>,
    pub update_available: Option<String>,
    pub version: Option<String>,
    pub startup_time: Option<String>,
}

// ── Cache + fetcher ───────────────────────────────────────────────────────

#[derive(Clone)]
struct CachedCameras {
    at: Instant,
    value: WorldModelCamerasResponse,
}

// Keyed by workspace_id: a roster fetched with workspace A's UniFi key (real
// device names / IPs / MACs) must never be served to workspace B.
fn cache() -> &'static Mutex<HashMap<Uuid, CachedCameras>> {
    static CACHE: OnceLock<Mutex<HashMap<Uuid, CachedCameras>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolves the UniFi API key from the workspace's `integrations.unifi` config
/// entry (its `api_key_var` secret), mirroring how the weather and foot-traffic
/// handlers resolve theirs instead of reading a raw process env var. Returns
/// `None` when no `unifi` integration is configured — `list_cameras` surfaces
/// that as a 503 so the world-model FE renders the empty camera state cleanly.
async fn unifi_key<S>(workspace: &WorkspaceManager<S>) -> Option<String> {
    match apps_helpers::resolve_unifi(
        workspace.config_manager.get_config(),
        &workspace.secrets_manager,
    )
    .await
    {
        Ok(key) => key,
        Err(e) => {
            tracing::warn!(
                workspace = %workspace.workspace_id,
                error = %e,
                "failed to resolve unifi api key from config",
            );
            None
        }
    }
}

async fn fetch_from_unifi(key: &str) -> Result<WorldModelCamerasResponse, (StatusCode, String)> {
    let client = oxy_shared::http::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let resp = client
        .get(UNIFI_LIST_DEVICES_URL)
        .header("X-API-Key", key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("unifi: {e}")))?;
    let status = resp.status();
    let body = resp
        .bytes()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    if !status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("unifi {}: {}", status, String::from_utf8_lossy(&body)),
        ));
    }
    let parsed: UnifiListDevicesResponse = serde_json::from_slice(&body)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("unifi parse: {e}")))?;

    let hosts: Vec<HostCameras> = parsed
        .data
        .into_iter()
        .map(|h| {
            let cameras: Vec<Camera> = h
                .devices
                .into_iter()
                .filter(|d| d.product_line == "protect")
                .map(|d| Camera {
                    id: d.id,
                    mac: d.mac,
                    name: d.name,
                    model: d.model,
                    shortname: d.shortname,
                    ip: d.ip,
                    status: d.status,
                    firmware_status: d.firmware_status,
                    update_available: d.update_available,
                    version: d.version,
                    startup_time: d.startup_time,
                })
                .collect();
            // Host → restaurant mapping now lives entirely in the
            // world-model app (src/camerasByStore.ts). The backend just
            // returns the raw UniFi roster.
            HostCameras {
                host_id: h.host_id,
                host_name: h.host_name,
                cameras,
            }
        })
        .collect();
    Ok(WorldModelCamerasResponse { hosts })
}

/// `GET /api/{workspace_id}/world-model/cameras`
///
/// Returns the full Protect camera roster across every UniFi host the key
/// can see. Cached process-wide for 60s. Frontend filters by host_name →
/// restaurant_id mapping.
#[axum::debug_handler]
#[tracing::instrument(skip_all)]
pub async fn list_cameras(
    WorkspaceManagerReadOnly(workspace): WorkspaceManagerReadOnly,
) -> Result<Json<WorldModelCamerasResponse>, (StatusCode, String)> {
    let workspace_id = workspace.workspace_id;
    {
        let guard = cache().lock().await;
        if let Some(cached) = guard.get(&workspace_id)
            && cached.at.elapsed() < CACHE_TTL
        {
            return Ok(Json(cached.value.clone()));
        }
    }
    let key = unifi_key(&workspace).await.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "unifi integration not configured for this workspace".to_string(),
    ))?;
    let fresh = fetch_from_unifi(&key).await?;
    cache().lock().await.insert(
        workspace_id,
        CachedCameras {
            at: Instant::now(),
            value: fresh.clone(),
        },
    );
    Ok(Json(fresh))
}

// ── Background poller for camera state changes ────────────────────────────
//
// Polls the same endpoint every 60s, diffs against the last snapshot, and
// publishes `camera_offline` / `camera_back_online` / `firmware_pending`
// events through the same broadcast bus the Toast webhook uses. This is
// what fills the CAMERAS tab in LIVE EVENTS without owner-level Protect
// access.

use oxy_world_model::live_feed::{CameraStateEvent, publish_camera_event};

/// Spawn the background watcher. Called once from server startup; idempotent.
pub fn spawn_camera_watcher(workspace_id: Uuid, api_key: String) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    if api_key.is_empty() {
        tracing::info!("unifi api key empty — camera watcher not spawned");
        return;
    }
    tokio::spawn(async move {
        // First tick after a short warm-up so server has a chance to bind.
        tokio::time::sleep(Duration::from_secs(15)).await;
        let mut last: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new(); // camera_id -> (host_name, status)
        loop {
            match fetch_from_unifi(&api_key).await {
                Ok(resp) => {
                    let mut current: std::collections::HashMap<String, (String, String)> =
                        std::collections::HashMap::new();
                    for host in &resp.hosts {
                        let host_name = host.host_name.clone().unwrap_or_default();
                        for cam in &host.cameras {
                            let status = cam.status.clone().unwrap_or_default();
                            current.insert(cam.id.clone(), (host_name.clone(), status));
                        }
                    }
                    // Compare against last snapshot — only emit on transitions.
                    for (cam_id, (host_name, status)) in &current {
                        match last.get(cam_id) {
                            Some((_, prev_status)) if prev_status != status => {
                                let kind = match status.as_str() {
                                    "online" => "camera_back_online",
                                    "offline" => "camera_offline",
                                    _ => "camera_state_change",
                                };
                                publish_camera_event(
                                    workspace_id,
                                    CameraStateEvent {
                                        kind,
                                        camera_id: cam_id.clone(),
                                        host_name: host_name.clone(),
                                        status: status.clone(),
                                    },
                                );
                            }
                            None
                                // First time we see this camera — only emit if offline,
                                // so the panel doesn't fire ~120 "online" events on boot.
                                if status == "offline" => {
                                    publish_camera_event(
                                        workspace_id,
                                        CameraStateEvent {
                                            kind: "camera_offline",
                                            camera_id: cam_id.clone(),
                                            host_name: host_name.clone(),
                                            status: status.clone(),
                                        },
                                    );
                                }
                            _ => {}
                        }
                    }
                    last = current;
                }
                Err((_, msg)) => {
                    tracing::warn!("camera watcher: unifi fetch failed: {}", msg);
                }
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

// ── OpenWeatherMap tile proxy ─────────────────────────────────────────────
//
// `GET /api/{workspace_id}/world-model/weather/{layer}/{z}/{x}/{y}.png`
//
// Proxies OpenWeatherMap's free weather raster tiles
// (<https://openweathermap.org/api/weathermaps>) so the API key stays
// server-side. Only the layers we render are whitelisted; anything else
// returns 400.
//
// Tiles are cached aggressively browser-side (15-minute freshness window
// matches OWM's recommendation for the free tier — temperature and
// precipitation rasters refresh on the OWM side roughly every 15 min).

const OWM_LAYERS: &[&str] = &["temp_new", "precipitation_new", "clouds_new", "wind_new"];

#[derive(Debug, Deserialize)]
pub struct WeatherTilePath {
    pub layer: String,
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// Resolves the OpenWeatherMap API key from the workspace's
/// `integrations.openweathermap` config entry. Returns `None` when no
/// `openweathermap` integration is configured — both weather handlers
/// surface that as a 503 so the world-model FE falls back gracefully.
async fn owm_key<S>(workspace: &WorkspaceManager<S>) -> Option<String> {
    match apps_helpers::resolve_openweather(
        workspace.config_manager.get_config(),
        &workspace.secrets_manager,
    )
    .await
    {
        Ok(key) => key,
        Err(e) => {
            tracing::warn!(
                workspace = %workspace.workspace_id,
                error = %e,
                "failed to resolve openweathermap api key from config",
            );
            None
        }
    }
}

#[axum::debug_handler]
#[tracing::instrument(skip_all, fields(layer = %path.layer, z = path.z, x = path.x, y = path.y))]
pub async fn weather_tile(
    WorkspaceManagerReadOnly(workspace): WorkspaceManagerReadOnly,
    AxumPath(path): AxumPath<WeatherTilePath>,
) -> Result<Response, (StatusCode, String)> {
    if !OWM_LAYERS.contains(&path.layer.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "unsupported weather layer '{}'; supported: {}",
                path.layer,
                OWM_LAYERS.join(", ")
            ),
        ));
    }
    let key = owm_key(&workspace).await.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "openweathermap integration not configured for this workspace".to_string(),
    ))?;

    let url = format!(
        "https://tile.openweathermap.org/map/{}/{}/{}/{}.png?appid={}",
        path.layer, path.z, path.x, path.y, key
    );
    let client = oxy_shared::http::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("owm: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    if !status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("owm {}: {}", status, String::from_utf8_lossy(&bytes)),
        ));
    }

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("image/png"));
    // 15-minute browser cache — matches OWM's tile refresh cadence.
    headers.insert(
        "Cache-Control",
        HeaderValue::from_static("public, max-age=900"),
    );

    let mut response = Response::new(axum::body::Body::from(bytes));
    *response.headers_mut() = headers;
    Ok(response)
}

// ── OpenWeatherMap current-weather batch endpoint ─────────────────────────
//
// `POST /api/{workspace_id}/world-model/weather/current`
// body: `[{ key, lat, lon }, …]`
//
// Returns `[{ key, temp_f, feels_like_f, precipitation_mm_1h, condition,
// description, icon }, …]` — one entry per requested coord. Calls OWM's
// `/data/2.5/weather` (works with the free-tier key, unlike the tile
// endpoint which lives behind a paid subscription).
//
// 10-minute cache per (lat,lon) — matches OWM's data refresh cadence.

#[derive(Debug, Deserialize)]
pub struct WeatherCoordRequest {
    pub key: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeatherCurrent {
    pub key: String,
    pub temp_f: f64,
    pub feels_like_f: f64,
    pub precipitation_mm_1h: f64,
    pub condition: String,
    pub description: String,
    pub icon: String,
    pub humidity_pct: f64,
    pub pressure_hpa: f64,
    pub wind_speed_mph: f64,
    pub wind_deg: f64,
    pub clouds_pct: f64,
}

#[derive(Debug, Deserialize)]
struct OwmWeatherResponse {
    main: OwmMain,
    weather: Vec<OwmCondition>,
    #[serde(default)]
    rain: Option<OwmPrecipitation>,
    #[serde(default)]
    snow: Option<OwmPrecipitation>,
    #[serde(default)]
    wind: Option<OwmWind>,
    #[serde(default)]
    clouds: Option<OwmClouds>,
}

#[derive(Debug, Deserialize)]
struct OwmMain {
    temp: f64,
    feels_like: f64,
    #[serde(default)]
    humidity: f64,
    #[serde(default)]
    pressure: f64,
}

#[derive(Debug, Deserialize, Default)]
struct OwmWind {
    #[serde(default)]
    speed: f64,
    #[serde(default)]
    deg: f64,
}

#[derive(Debug, Deserialize, Default)]
struct OwmClouds {
    #[serde(default)]
    all: f64,
}

#[derive(Debug, Deserialize)]
struct OwmCondition {
    main: String,
    description: String,
    icon: String,
}

#[derive(Debug, Deserialize, Default)]
struct OwmPrecipitation {
    #[serde(rename = "1h")]
    one_h: Option<f64>,
}

#[derive(Clone)]
struct CachedWeather {
    at: Instant,
    value: WeatherCurrent,
}

fn weather_cache() -> &'static Mutex<HashMap<String, CachedWeather>> {
    static C: OnceLock<Mutex<HashMap<String, CachedWeather>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const WEATHER_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

async fn fetch_one(
    client: &reqwest::Client,
    workspace_id: Uuid,
    key: &str,
    req: WeatherCoordRequest,
) -> Result<WeatherCurrent, String> {
    // Cache key — scoped to the workspace, then rounded to 2 decimals so
    // neighbouring stores within the same workspace share entries. The
    // workspace prefix keeps one tenant's cached fetches from being served to
    // another (matching the camera-roster cache).
    let cache_key = format!("{workspace_id}:{:.2},{:.2}", req.lat, req.lon);
    {
        let cache = weather_cache().lock().await;
        if let Some(cached) = cache.get(&cache_key)
            && cached.at.elapsed() < WEATHER_CACHE_TTL
        {
            let mut v = cached.value.clone();
            v.key = req.key;
            return Ok(v);
        }
    }
    let url = format!(
        "https://api.openweathermap.org/data/2.5/weather?lat={}&lon={}&units=imperial&appid={}",
        req.lat, req.lon, key
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("owm: {e}"))?;
    let status = resp.status();
    let body = resp.bytes().await.map_err(|e| format!("owm read: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "owm {}: {}",
            status,
            String::from_utf8_lossy(&body)
        ));
    }
    let parsed: OwmWeatherResponse =
        serde_json::from_slice(&body).map_err(|e| format!("owm parse: {e}"))?;

    let condition_obj = parsed.weather.into_iter().next();
    let precipitation_mm_1h = parsed.rain.and_then(|r| r.one_h).unwrap_or(0.0)
        + parsed.snow.and_then(|s| s.one_h).unwrap_or(0.0);

    let wind = parsed.wind.unwrap_or_default();
    let clouds = parsed.clouds.unwrap_or_default();
    let value = WeatherCurrent {
        key: req.key,
        temp_f: parsed.main.temp,
        feels_like_f: parsed.main.feels_like,
        precipitation_mm_1h,
        condition: condition_obj
            .as_ref()
            .map(|c| c.main.clone())
            .unwrap_or_default(),
        description: condition_obj
            .as_ref()
            .map(|c| c.description.clone())
            .unwrap_or_default(),
        icon: condition_obj.map(|c| c.icon).unwrap_or_default(),
        humidity_pct: parsed.main.humidity,
        pressure_hpa: parsed.main.pressure,
        // units=imperial → wind speed already in mph
        wind_speed_mph: wind.speed,
        wind_deg: wind.deg,
        clouds_pct: clouds.all,
    };

    let mut cache = weather_cache().lock().await;
    cache.insert(
        cache_key,
        CachedWeather {
            at: Instant::now(),
            value: value.clone(),
        },
    );
    Ok(value)
}

#[axum::debug_handler]
#[tracing::instrument(skip_all)]
pub async fn weather_current_batch(
    WorkspaceManagerReadOnly(workspace): WorkspaceManagerReadOnly,
    axum::Json(coords): axum::Json<Vec<WeatherCoordRequest>>,
) -> Result<Json<Vec<WeatherCurrent>>, (StatusCode, String)> {
    let workspace_id = workspace.workspace_id;
    let key = owm_key(&workspace).await.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "openweathermap integration not configured for this workspace".to_string(),
    ))?;
    let client = oxy_shared::http::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let futures = coords
        .into_iter()
        .map(|c| fetch_one(&client, workspace_id, &key, c))
        .collect::<Vec<_>>();
    let results = futures::future::join_all(futures).await;

    let mut out = Vec::with_capacity(results.len());
    for r in results {
        match r {
            Ok(w) => out.push(w),
            Err(e) => {
                tracing::warn!("owm fetch failed: {}", e);
            }
        }
    }
    Ok(Json(out))
}
