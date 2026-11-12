//! Foot traffic for the world-model — powered by BestTime.app.
//!
//! BestTime exposes a `/api/v1/forecasts/live` endpoint that returns the
//! current foot-traffic busyness percentage for a venue, plus the
//! forecasted value and the delta between them. We batch one request per
//! store and return a vector keyed by restaurant id.
//!
//! Lookup pattern matches `cameras_by_store.yml`:
//! `foot_traffic_by_store.yml` in the workspace cwd pairs each store
//! (`location_name`) to either a stored `venue_id` (preferred — created
//! once via `POST /api/v1/forecasts`) or `venue_name` + `venue_address`
//! (BestTime accepts either combination). Stores without an entry are
//! omitted from the response.
//!
//! Reads the API key from `BESTTIME_API_KEY` (the value of the private
//! key BestTime issues; their own docs call it `api_key_private`).
//!

use axum::Json;
use axum::http::StatusCode;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::apps_helpers;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, OnceCell};

use oxy_app::surface::WorkspaceManagerReadOnly;

const BESTTIME_LIVE_URL: &str = "https://besttime.app/api/v1/forecasts/live";
// `/radar/filter` serves BestTime's web UI (HTML). The JSON equivalent
// is `/venues/filter` — same filter params, returns the venue list.
const BESTTIME_RADAR_URL: &str = "https://besttime.app/api/v1/venues/filter";
const CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 336);
// The foot-traffic caches are in-memory only (process lifetime). They start
// warm from the compile-time seed (`foot_traffic_seed::SEED_JSON`) and are
// updated by live BestTime fetches, but nothing is written to disk. This
// shields the deployment from BestTime's free-tier quota (300 req/min) without
// baking a builder-specific path into the binary or needing a writable
// filesystem.

// ── API request / response ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct FootTrafficRequest {
    /// Restaurant id (matches `restaurants.restaurant_id`). Echoed back
    /// in the response so the frontend can key its Map<id, entry>.
    pub key: String,
    /// BestTime venue name + address, built by the frontend directly from
    /// the store record (`restaurants.location_name` + the address/city/
    /// state/zip dimensions). No server-side mapping file: the store data
    /// already carries everything BestTime needs to geocode the venue.
    /// BestTime re-geocodes name+address on each call (the old `venue_id`
    /// fast-path is dropped — same quota cost, marginally slower).
    pub venue_name: String,
    pub venue_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FootTrafficCurrent {
    pub key: String,
    /// 0–100 (can exceed 100 on rare busy peaks per BestTime docs).
    pub live_busyness: f64,
    /// Forecasted busyness for the current hour, same scale.
    pub forecast_busyness: f64,
    /// -100..+100 — live minus forecast. Positive = busier than usual.
    pub delta: f64,
    pub venue_open: bool,
}

// ── BestTime upstream ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BesttimeLiveResponse {
    /// Optional because BestTime returns a 200 with no `analysis` block
    /// for venues it hasn't indexed yet (or that are currently closed
    /// outside forecasted hours). We coerce missing → all zeros so the
    /// frontend can still render a "no data / closed" card.
    #[serde(default)]
    analysis: BesttimeAnalysis,
    #[serde(default)]
    venue_info: BesttimeVenueInfo,
}

#[derive(Debug, Default, Deserialize)]
struct BesttimeAnalysis {
    #[serde(default)]
    venue_live_busyness: f64,
    #[serde(default)]
    venue_forecasted_busyness: f64,
    #[serde(default)]
    venue_live_forecasted_delta: f64,
}

#[derive(Debug, Default, Deserialize)]
struct BesttimeVenueInfo {
    #[serde(default)]
    venue_open: BesttimeOpen,
}

// BestTime returns this as either `true` / `false` or as a numeric/string
// flag depending on the version. Default to false-on-anything-weird.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum BesttimeOpen {
    Bool(bool),
    Str(String),
    Num(serde_json::Number),
    #[default]
    Missing,
}

impl BesttimeOpen {
    fn as_bool(&self) -> bool {
        match self {
            BesttimeOpen::Bool(b) => *b,
            BesttimeOpen::Str(s) => {
                matches!(s.to_ascii_lowercase().as_str(), "open" | "true" | "1")
            }
            BesttimeOpen::Num(n) => n.as_i64().map(|v| v != 0).unwrap_or(false),
            BesttimeOpen::Missing => false,
        }
    }
}

/// Resolves the BestTime API key from the workspace's `integrations.besttime`
/// config entry. Returns `None` when no `besttime` integration is configured —
/// both handlers surface that as a 503 so the world-model FE renders the empty
/// foot-traffic state cleanly.
async fn besttime_key<S>(workspace: &WorkspaceManager<S>) -> Option<String> {
    match apps_helpers::resolve_besttime(
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
                "failed to resolve besttime api key from config",
            );
            None
        }
    }
}

// reqwest sometimes echoes the full request URL in its error messages,
// which leaks the api_key_private query param into logs. Scrub the
// literal key value before propagating any error string upward.
fn scrub_api_key(msg: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        return msg.to_string();
    }
    msg.replace(api_key, "<redacted>")
}

// ── Live cache ────────────────────────────────────────────────────────────

#[derive(Clone, Serialize, Deserialize)]
struct CachedLive {
    /// Unix seconds since epoch — chosen over Instant so the value
    /// serializes onto the persisted JSON cache file.
    at_unix: u64,
    value: FootTrafficCurrent,
}

fn live_cache() -> &'static Mutex<HashMap<String, CachedLive>> {
    static C: OnceLock<Mutex<HashMap<String, CachedLive>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn elapsed_since(at_unix: u64) -> Duration {
    Duration::from_secs(now_unix().saturating_sub(at_unix))
}

// ── In-memory cache seeding ───────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
struct PersistedCaches {
    live: HashMap<String, CachedLive>,
    radar: HashMap<String, CachedRadar>,
}

// Seed-once guard. The first hit parses the compile-time seed snapshot into
// the in-memory maps (best-effort — a malformed seed just leaves them empty).
// Subsequent calls are a cheap atomic load.
static CACHE_LOADED: OnceCell<()> = OnceCell::const_new();

async fn ensure_caches_loaded() {
    CACHE_LOADED
        .get_or_init(|| async {
            let parsed: PersistedCaches = match serde_json::from_str(
                crate::foot_traffic_seed::SEED_JSON,
            ) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse foot-traffic seed; starting empty");
                    return;
                }
            };
            let live_n = parsed.live.len();
            let radar_n = parsed.radar.len();
            *live_cache().lock().await = parsed.live;
            *radar_cache().lock().await = parsed.radar;
            tracing::info!(
                live = live_n,
                radar = radar_n,
                "seeded foot-traffic cache from embedded snapshot"
            );
        })
        .await;
}

async fn fetch_one(
    client: &reqwest::Client,
    api_key: &str,
    key: &str,
    venue_name: &str,
    venue_address: &str,
) -> Result<FootTrafficCurrent, String> {
    ensure_caches_loaded().await;
    {
        let cache = live_cache().lock().await;
        if let Some(cached) = cache.get(key)
            && elapsed_since(cached.at_unix) < CACHE_TTL
        {
            return Ok(cached.value.clone());
        }
    }

    // BestTime accepts auth + venue identifiers as query parameters (not
    // JSON body) — that's how their docs / Python / curl examples send it.
    // POST with empty body, all data in the URL. We always send
    // venue_name + venue_address (built by the frontend from the store
    // record); BestTime geocodes them on each call.
    let query: Vec<(&str, String)> = vec![
        ("api_key_private", api_key.to_string()),
        ("venue_name", venue_name.to_string()),
        ("venue_address", venue_address.to_string()),
    ];

    let resp = client
        .post(BESTTIME_LIVE_URL)
        .query(&query)
        .send()
        .await
        .map_err(|e| format!("besttime: {}", scrub_api_key(&e.to_string(), api_key)))?;
    let status = resp.status();
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("besttime read: {}", scrub_api_key(&e.to_string(), api_key)))?;
    if !status.is_success() {
        return Err(format!(
            "besttime {}: {}",
            status,
            String::from_utf8_lossy(&body)
        ));
    }
    let parsed: BesttimeLiveResponse =
        serde_json::from_slice(&body).map_err(|e| format!("besttime parse: {e}"))?;

    let value = FootTrafficCurrent {
        key: key.to_string(),
        live_busyness: parsed.analysis.venue_live_busyness,
        forecast_busyness: parsed.analysis.venue_forecasted_busyness,
        delta: parsed.analysis.venue_live_forecasted_delta,
        venue_open: parsed.venue_info.venue_open.as_bool(),
    };

    {
        let mut cache = live_cache().lock().await;
        cache.insert(
            key.to_string(),
            CachedLive {
                at_unix: now_unix(),
                value: value.clone(),
            },
        );
    }
    Ok(value)
}

// ── Handler ──────────────────────────────────────────────────────────────

#[axum::debug_handler]
#[tracing::instrument(skip_all)]
pub async fn foot_traffic_current_batch(
    WorkspaceManagerReadOnly(workspace): WorkspaceManagerReadOnly,
    axum::Json(stores): axum::Json<Vec<FootTrafficRequest>>,
) -> Result<Json<Vec<FootTrafficCurrent>>, (StatusCode, String)> {
    let api_key = besttime_key(&workspace).await.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "besttime integration not configured for this workspace".to_string(),
    ))?;
    let client = oxy_shared::http::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Each store carries its own venue_name + venue_address (the frontend
    // builds them from the store record). Stores that arrive without both
    // are silently omitted — the frontend treats missing entries as "no
    // data available", same pattern as before.
    let futures = stores.into_iter().filter_map(|s| {
        if s.venue_name.trim().is_empty() || s.venue_address.trim().is_empty() {
            return None;
        }
        let api_key = api_key.clone();
        let client = client.clone();
        Some(async move {
            let id = s.key.clone();
            match fetch_one(&client, &api_key, &id, &s.venue_name, &s.venue_address).await {
                Ok(value) => Some(value),
                Err(e) => {
                    tracing::warn!(key = %id, error = %e, "foot-traffic fetch failed");
                    None
                }
            }
        })
    });
    let results = futures::future::join_all(futures).await;
    Ok(Json(results.into_iter().flatten().collect()))
}

// ── Radar — many-venue heatmap data ───────────────────────────────────────
//
// Powers the world-model Foot Traffic heatmap. Plotting only our 18 Poke
// House stores produces 18 isolated dots. The reference look the user
// pointed at (Hanoi-style) shows ~100 venues per metro packed together,
// blending into red hotspots. BestTime's `/v1/radar/filter` returns all
// indexed venues within a radius around a coord, with per-venue
// busyness — that's the right shape for the heatmap.
//
// Backend takes a list of `{key, lat, lon}` (each store's coord), calls
// the radar endpoint per coord, dedupes results by venue_id, returns a
// flat list. Cached process-wide for 10 minutes.

#[derive(Debug, Deserialize)]
pub struct RadarRequest {
    pub key: String,
    pub lat: f64,
    pub lon: f64,
    /// Search radius in meters. Default ~15 km (matches the reference URL
    /// the user shared).
    #[serde(default)]
    pub radius: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadarVenue {
    pub venue_id: String,
    pub venue_name: String,
    pub lat: f64,
    pub lon: f64,
    /// 0..100 if BestTime has a live signal for this venue. 0 if it
    /// only has a forecast — frontend should treat 0 as "no live data"
    /// and fall back to `forecast_busyness` for the heatmap weight.
    pub live_busyness: f64,
    pub forecast_busyness: f64,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedRadar {
    at_unix: u64,
    venues: Vec<RadarVenue>,
}

fn radar_cache() -> &'static Mutex<HashMap<String, CachedRadar>> {
    static C: OnceLock<Mutex<HashMap<String, CachedRadar>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const RADAR_TYPES: &str = "RESTAURANT,ASIAN_RESTAURANT,CHINESE_RESTAURANT,JAPANESE_RESTAURANT,SUSHI_RESTAURANT,BURGER_RESTAURANT,PIZZA_RESTAURANT,MEXICAN_RESTAURANT,THAI_RESTAURANT,SEAFOOD_RESTAURANT,VEGETARIAN_RESTAURANT,STEAKHOUSE,RAMEN_RESTAURANT";

async fn fetch_radar(
    client: &reqwest::Client,
    api_key: &str,
    lat: f64,
    lon: f64,
    radius_m: u32,
) -> Result<Vec<RadarVenue>, String> {
    let cache_key = format!("{lat:.2},{lon:.2},{radius_m}");
    ensure_caches_loaded().await;
    {
        let cache = radar_cache().lock().await;
        if let Some(cached) = cache.get(&cache_key)
            && elapsed_since(cached.at_unix) < CACHE_TTL
        {
            return Ok(cached.venues.clone());
        }
    }

    // Only the fields venues/filter actually accepts. The radar web UI
    // tacks on map_lat/map_lng/map_z but those are UI viewport params
    // and the JSON endpoint rejects them as "Unknown field".
    let query: [(&str, String); 6] = [
        ("api_key_private", api_key.to_string()),
        ("lat", lat.to_string()),
        ("lng", lon.to_string()),
        ("radius", radius_m.to_string()),
        ("limit", "10".to_string()),
        ("types", RADAR_TYPES.to_string()),
    ];
    let resp = client
        .get(BESTTIME_RADAR_URL)
        .query(&query)
        .send()
        .await
        .map_err(|e| format!("besttime radar: {}", scrub_api_key(&e.to_string(), api_key)))?;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = resp.bytes().await.map_err(|e| {
        format!(
            "besttime radar read: {}",
            scrub_api_key(&e.to_string(), api_key)
        )
    })?;
    if !status.is_success() {
        return Err(format!(
            "besttime radar {} (content-type={}): {}",
            status,
            content_type,
            String::from_utf8_lossy(&body)
        ));
    }
    tracing::debug!(
        status = %status,
        content_type = %content_type,
        body_len = body.len(),
        "besttime radar response"
    );

    // Permissive parse — BestTime doesn't publish the radar response
    // schema and the field names have varied between versions. Walk the
    // JSON manually instead of derive-deserializing.
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        // Surface the first chunk of raw body in the error so we can see
        // what BestTime actually returns (HTML error page, wrapped
        // error envelope, etc.) without echoing the api key.
        let raw = String::from_utf8_lossy(&body);
        let raw_trim = raw.chars().take(400).collect::<String>();
        format!(
            "besttime radar parse: {e}; content-type={content_type} body_len={} raw (first 400 chars): {}",
            body.len(),
            scrub_api_key(&raw_trim, api_key)
        )
    })?;
    let venues_arr = json
        .get("venues")
        .or_else(|| json.get("data"))
        .or_else(|| json.get("results"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            format!(
                "besttime radar: no `venues`/`data`/`results` array in response, top-level keys: {:?}",
                json.as_object().map(|o| o.keys().collect::<Vec<_>>())
            )
        })?;

    let venues: Vec<RadarVenue> = venues_arr
        .iter()
        .filter_map(|v| {
            let venue_id = v
                .get("venue_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)?;
            let venue_name = v
                .get("venue_name")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let lat = v.get("venue_lat").or_else(|| v.get("lat"))?.as_f64()?;
            let lon = v
                .get("venue_lng")
                .or_else(|| v.get("lon"))
                .or_else(|| v.get("lng"))?
                .as_f64()?;
            let live_busyness = v
                .get("venue_live_busyness")
                .or_else(|| v.get("live_busyness"))
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0);
            let forecast_busyness = v
                .get("venue_forecasted_busyness")
                .or_else(|| v.get("forecasted_busyness"))
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0);
            Some(RadarVenue {
                venue_id,
                venue_name,
                lat,
                lon,
                live_busyness,
                forecast_busyness,
            })
        })
        .collect();

    {
        let mut cache = radar_cache().lock().await;
        cache.insert(
            cache_key,
            CachedRadar {
                at_unix: now_unix(),
                venues: venues.clone(),
            },
        );
    }
    Ok(venues)
}

#[axum::debug_handler]
#[tracing::instrument(skip_all)]
pub async fn foot_traffic_radar_batch(
    WorkspaceManagerReadOnly(workspace): WorkspaceManagerReadOnly,
    axum::Json(stores): axum::Json<Vec<RadarRequest>>,
) -> Result<Json<Vec<RadarVenue>>, (StatusCode, String)> {
    let api_key = besttime_key(&workspace).await.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "besttime integration not configured for this workspace".to_string(),
    ))?;
    let client = oxy_shared::http::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let futures = stores.into_iter().map(|s| {
        let api_key = api_key.clone();
        let client = client.clone();
        async move {
            let radius = s.radius.unwrap_or(15_000);
            match fetch_radar(&client, &api_key, s.lat, s.lon, radius).await {
                Ok(venues) => venues,
                Err(e) => {
                    tracing::warn!(key = %s.key, lat = s.lat, lon = s.lon, error = %e, "radar fetch failed");
                    vec![]
                }
            }
        }
    });
    let results = futures::future::join_all(futures).await;

    // Dedupe by venue_id across all per-store calls. Overlapping store
    // radii in dense areas (Bay Area cluster) will return many of the
    // same venues — we want one heatmap point per real venue.
    let mut seen = HashMap::<String, RadarVenue>::new();
    for batch in results {
        for v in batch {
            seen.entry(v.venue_id.clone()).or_insert(v);
        }
    }
    Ok(Json(seen.into_values().collect()))
}
