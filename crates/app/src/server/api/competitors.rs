//! Nearby competitors for the world-model — OpenStreetMap venues via Overpass.
//!
//! The world-model app's competitor layer needs nearby restaurants / fast-food
//! / cafes around each Poke House store. It used to hit overpass-api.de
//! directly from the browser, but Overpass serves its error / throttle
//! responses (429/504/406) WITHOUT an `Access-Control-Allow-Origin` header, so
//! a failed call surfaced as a misleading "blocked by CORS policy" error in
//! production. We do the lookup here instead: same-origin for the browser, real
//! upstream statuses on failure, and the Overpass QL stays server-side.
//!
//! This returns every named venue near the anchors; deciding which are
//! competitors (i.e. filtering out Poke House's own locations) is the
//! world-model app's job.
//!
//! Override the upstream with the `OVERPASS_URL` env var (e.g. a paid mirror
//! such as `https://overpass.kumi.systems/api/interpreter`).

use axum::Json;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const OVERPASS_DEFAULT_URL: &str = "https://overpass-api.de/api/interpreter";

/// Overpass instances run behind Apache + mod_security, which returns
/// `406 Not Acceptable` for requests without a `User-Agent`. reqwest sends no
/// User-Agent by default, so we must set one — and Overpass's usage policy asks
/// callers to identify themselves anyway.
const OVERPASS_USER_AGENT: &str = concat!("oxy-world-model/", env!("CARGO_PKG_VERSION"));

/// One store coordinate to search around.
#[derive(Debug, Deserialize)]
pub struct Anchor {
    pub lat: f64,
    pub lon: f64,
}

/// `POST /world-model/competitors` body. The frontend passes one anchor per
/// store (a single anchor for the per-store panel, all stores for the map
/// layer) plus the search radius.
#[derive(Debug, Deserialize)]
pub struct CompetitorsRequest {
    pub anchors: Vec<Anchor>,
    /// Search radius around each anchor, in meters. Defaults to 1 km.
    #[serde(default = "default_radius_m")]
    pub radius_m: u32,
    /// Include OSM ways/relations (polygons) in addition to nodes. The
    /// per-store panel wants the extra coverage; the chain-wide map layer
    /// stays node-only for speed.
    #[serde(default)]
    pub include_ways: bool,
}

fn default_radius_m() -> u32 {
    1000
}

/// A nearby third-party venue. Field names are camelCased on the wire to match
/// the world-model app's `OsmVenue` type verbatim.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Competitor {
    /// OSM element ref, e.g. `node/123` — unique across nodes/ways/relations.
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// `restaurant` / `fast_food` / `cafe`.
    pub amenity: String,
    /// `cuisine` tag value; `null` when untagged.
    pub cuisine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opening_hours: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// `POST /api/{workspace_id}/world-model/competitors`
///
/// Returns nearby food venues around the requested store anchors. The
/// world-model app filters out Poke House's own locations client-side. See the
/// module docs for why this lives server-side rather than calling Overpass from
/// the browser.
#[axum::debug_handler]
#[tracing::instrument(skip_all)]
pub async fn get_competitors(
    Json(req): Json<CompetitorsRequest>,
) -> Result<Json<Vec<Competitor>>, (StatusCode, String)> {
    if req.anchors.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let query = build_overpass_query(&req);
    let json = run_overpass(&query).await?;
    Ok(Json(parse_competitors(&json)))
}

/// Build a single Overpass QL program unioning an `around:` filter per anchor.
/// Per-anchor `around:` clauses each use the spatial index independently, so N
/// small radius scans finish far faster than one chain-wide bbox (which
/// Overpass times out on for chains spanning multiple metros).
fn build_overpass_query(req: &CompetitorsRequest) -> String {
    let kinds: &[&str] = if req.include_ways {
        &["node", "way", "relation"]
    } else {
        &["node"]
    };
    let amenity = r#"["amenity"~"^(restaurant|fast_food|cafe)$"]"#;
    let mut filters = String::new();
    for a in &req.anchors {
        for kind in kinds {
            filters.push_str(&format!(
                "{kind}{amenity}(around:{},{},{});\n",
                req.radius_m, a.lat, a.lon
            ));
        }
    }
    // More anchors → allow Overpass more wall-clock; a single store is quick.
    let timeout = if req.anchors.len() > 1 { 60 } else { 25 };
    // `out center tags` collapses ways/relations to a single coordinate and
    // still emits node lat/lon, so the parser treats every element uniformly.
    format!("[out:json][timeout:{timeout}];\n(\n{filters});\nout center tags;")
}

/// POST the query to Overpass and return the parsed JSON body, mapping
/// transport / status / decode failures to real HTTP errors (never a fake CORS
/// error, which is what a direct browser call would surface instead).
async fn run_overpass(query: &str) -> Result<Value, (StatusCode, String)> {
    let upstream =
        std::env::var("OVERPASS_URL").unwrap_or_else(|_| OVERPASS_DEFAULT_URL.to_string());
    let client = oxy_shared::http::builder()
        .timeout(Duration::from_secs(60))
        // Overpass returns 406 Not Acceptable without a User-Agent; reqwest
        // omits one by default. See OVERPASS_USER_AGENT.
        .user_agent(OVERPASS_USER_AGENT)
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let res = client
        .post(&upstream)
        // reqwest form-encodes to `data=<urlencoded query>` and sets the
        // `application/x-www-form-urlencoded` content type Overpass expects.
        .form(&[("data", query)])
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("overpass request failed: {e}"),
            )
        })?;

    let status = res.status();
    if !status.is_success() {
        let body: String = res
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();
        return Err((
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            format!("overpass {status}: {body}"),
        ));
    }

    let json = res.json::<Value>().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("overpass returned non-JSON: {e}"),
        )
    })?;

    // Overpass replies 200 even when its query runtime times out — the failure
    // shows up as `elements: []` plus a `remark` field. Surface it as an error
    // so the frontend's React Query retries instead of caching an empty list.
    if let Some(remark) = json.get("remark").and_then(Value::as_str)
        && (remark.contains("timed out") || remark.contains("runtime error"))
    {
        return Err((StatusCode::GATEWAY_TIMEOUT, format!("overpass: {remark}")));
    }
    Ok(json)
}

/// Normalize the Overpass `elements` array into venues: skip unnamed /
/// coordinate-less elements and dedupe by OSM id. Owned-vs-competitor
/// filtering is the world-model app's job — this returns every named venue.
fn parse_competitors(json: &Value) -> Vec<Competitor> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let Some(elements) = json.get("elements").and_then(Value::as_array) else {
        return out;
    };
    for el in elements {
        let Some(c) = competitor_from_element(el) else {
            continue;
        };
        if seen.insert(c.id.clone()) {
            out.push(c);
        }
    }
    out
}

/// Map one Overpass element to a `Competitor`, or `None` if it lacks a name or
/// a resolvable coordinate (nodes carry lat/lon; ways/relations carry `center`).
fn competitor_from_element(el: &Value) -> Option<Competitor> {
    let tags = el.get("tags")?;
    let name = tags.get("name")?.as_str()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let lat = el
        .get("lat")
        .or_else(|| el.get("center").and_then(|c| c.get("lat")))?
        .as_f64()?;
    let lon = el
        .get("lon")
        .or_else(|| el.get("center").and_then(|c| c.get("lon")))?
        .as_f64()?;
    let kind = el.get("type").and_then(Value::as_str).unwrap_or("node");
    let id_num = el.get("id").and_then(Value::as_i64).unwrap_or(0);

    let tag = |k: &str| tags.get(k).and_then(Value::as_str).map(str::to_string);
    Some(Competitor {
        id: format!("{kind}/{id_num}"),
        name,
        lat,
        lon,
        amenity: tag("amenity").unwrap_or_else(|| "restaurant".to_string()),
        cuisine: tag("cuisine"),
        website: tag("website").or_else(|| tag("contact:website")),
        phone: tag("phone").or_else(|| tag("contact:phone")),
        opening_hours: tag("opening_hours"),
        address: build_address(tags),
    })
}

/// One-line address assembled from OSM `addr:*` tags, when present.
fn build_address(tags: &Value) -> Option<String> {
    let t = |k: &str| tags.get(k).and_then(Value::as_str).unwrap_or("");
    let street = [t("addr:housenumber"), t("addr:street")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let parts: Vec<&str> = [street.as_str(), t("addr:city")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}
