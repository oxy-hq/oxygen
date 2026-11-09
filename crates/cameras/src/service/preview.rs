//! Workspace-scoped snapshot proxy.
//!
//! ## Flow
//!
//! ```
//! Browser ─HTTPS─→ Oxy /{wid}/cameras/{cam_id}/preview/snapshot.jpg
//!                 ↓ workspace ownership check + edge lookup
//!                 ↓ tailscale_ip + PREVIEW_PORT
//! Oxy backend ──HTTP──→ Edge worker /preview/cameras/{cam_id}/snapshot.jpg
//!                                       ↓ aiohttp server (preview_server.py)
//!                                       ↓ CameraReader.latest_jpeg()
//!                                       ↓ cv2.imencode
//!                       ←── JPEG bytes ──
//! Oxy backend ←──
//! Browser ←── JPEG bytes
//! ```
//!
//! Tailscale terminates between Oxy and the edge: the edge's preview
//! server is only reachable from Oxy's identity. The browser only
//! talks to Oxy over HTTPS with its session cookie — the device
//! bearer and the edge URL never leave the backend.
//!
//! ## Why a proxy and not direct
//!
//! - The browser doesn't know (and shouldn't know) the edge's
//!   Tailscale IP. That's an infrastructure detail.
//! - The browser has a session cookie, not a device bearer — and the
//!   device bearer's plaintext was thrown away after `register`.
//! - The Oxy proxy gates on workspace ownership; talking directly
//!   would skip that.

use std::sync::OnceLock;
use std::time::Duration;

use bytes::Bytes;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::entities::{cameras, edge_boxes, sites};

use super::{ServiceError, ServiceResult};

/// Default port the edge worker's aiohttp preview server listens on.
/// Overridable via `OXY_CAMERAS_EDGE_PREVIEW_PORT` so a deployment
/// can pick a different port without redeploying Oxy.
const DEFAULT_EDGE_PREVIEW_PORT: u16 = 8090;

/// Default port for MediaMTX's HLS endpoint on the edge box.
/// Overridable via `OXY_CAMERAS_EDGE_HLS_PORT`.
const DEFAULT_EDGE_HLS_PORT: u16 = 8888;

/// Default port for MediaMTX's recording playback endpoint on the
/// edge box (`:9996` by default per mediamtx.yml). Overridable via
/// `OXY_CAMERAS_EDGE_PLAYBACK_PORT`.
const DEFAULT_EDGE_PLAYBACK_PORT: u16 = 9996;

/// Process-wide reqwest client with a shared cookie jar.
///
/// MediaMTX's HLS server does a one-time `cookieCheck=1` redirect on
/// the FIRST request to any path. With a per-call reqwest client the
/// cookie was fresh on every request, meaning every single playlist
/// + variant + segment fetch incurred the redirect roundtrip. With
/// LL-HLS sending parts every ~200ms that's a lot of wasted RTTs
/// (and a real risk of MTX rejecting the spam as a cookie loop).
///
/// One shared client per Oxy process amortizes the cookie set across
/// all subsequent fetches. Cookies are scoped to the upstream host —
/// `Cookie: cookieCheck=1` for `localhost:8888` (or whatever the
/// edge resolves to) — so multiple edge boxes still get their own
/// cookies independently within the same jar.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        oxy_shared::http::builder()
            .timeout(SNAPSHOT_TIMEOUT)
            .cookie_store(true)
            .build()
            .expect("build shared HTTP client for preview proxies")
    })
}

/// Per-call snapshot timeout. Short because the edge is supposed to
/// return cached bytes — anything slower is a sign the edge is
/// struggling and we'd rather fail fast than tie up an Oxy thread.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);

/// Build the base URL Oxy should dial for HLS / snapshot / clip
/// requests against this edge box.
///
/// Preference order:
///   1. **Funnel hostname**: `https://<funnel>` — single public hostname
///      that the box's nginx gateway demultiplexes by path. This is
///      what lets a cloud-hosted Oxy reach a tailnet-only box without
///      sharing a tailnet itself; the box's Tailscale Funnel publishes
///      port 443 publicly, nginx routes /preview/, /list, /get,
///      /cam-X/{whep,index.m3u8,…} to the correct upstream.
///   2. **Direct tailnet dial**: `http://<tailscale_ip>:<port>` — the
///      original shape, kept as a fallback for boxes that haven't
///      enabled Funnel (dev compose, prod-tailnet deployments where
///      Oxy is on the same tailnet anyway).
///
/// Returns `Unavailable` only when *both* are absent, since one or
/// the other is required for any preview/playback round-trip.
fn upstream_base(edge_box: &edge_boxes::Model, fallback_port: u16) -> ServiceResult<String> {
    if let Some(funnel) = edge_box.funnel_hostname.as_deref()
        && !funnel.trim().is_empty()
    {
        return Ok(format!("https://{funnel}"));
    }
    let tailscale_ip = edge_box.tailscale_ip.as_deref().ok_or_else(|| {
        ServiceError::Unavailable(
            "edge box has no reachable address (neither funnel_hostname nor tailscale_ip set)",
        )
    })?;
    Ok(format!("http://{tailscale_ip}:{fallback_port}"))
}

/// Result of a successful snapshot fetch. The body bytes are JPEG;
/// the content-type is whatever the upstream sent (almost always
/// `image/jpeg`).
pub struct SnapshotBytes {
    pub bytes: Bytes,
    pub content_type: String,
}

/// Fetch a JPEG snapshot for `camera_id` in `workspace_id`.
///
/// Failure modes (in order of likelihood):
/// - 404 NotFound      — camera doesn't exist in this workspace
/// - 403 Forbidden     — camera exists but in another workspace
/// - 503 Unavailable   — camera not bound to an edge box, or edge has
///                       no `tailscale_ip`, or upstream returned 503
///                       (camera still starting up)
/// - 502 Upstream      — network failure / timeout / upstream 5xx
pub async fn snapshot(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    camera_id: Uuid,
) -> ServiceResult<SnapshotBytes> {
    let cam = cameras::Entity::find_by_id(camera_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;

    let site = sites::Entity::find_by_id(cam.site_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    if site.workspace_id != workspace_id {
        return Err(ServiceError::Forbidden(
            "camera belongs to another workspace",
        ));
    }

    let edge_box_id = cam
        .edge_box_id
        .ok_or(ServiceError::Unavailable("camera not bound to an edge box"))?;
    let edge_box = edge_boxes::Entity::find_by_id(edge_box_id)
        .one(db)
        .await?
        .ok_or(ServiceError::Unavailable(
            "camera's edge box no longer exists",
        ))?;
    let port: u16 = std::env::var("OXY_CAMERAS_EDGE_PREVIEW_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_EDGE_PREVIEW_PORT);

    let base = upstream_base(&edge_box, port)?;
    let url = format!("{base}/preview/cameras/{camera_id}/snapshot.jpg");

    let resp = http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| ServiceError::Upstream(format!("GET {url}: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(ServiceError::NotFound);
    }
    if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        // Edge knows the camera but hasn't decoded a frame yet (just
        // started / reconnecting). Pass that signal through as-is so
        // the UI can show "waiting for first frame" rather than
        // collapsing it into a generic upstream error.
        return Err(ServiceError::Unavailable(
            "edge has no frame for this camera yet",
        ));
    }
    if !status.is_success() {
        return Err(ServiceError::Upstream(format!(
            "edge returned status {status}"
        )));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ServiceError::Upstream(format!("read body: {e}")))?;

    Ok(SnapshotBytes {
        bytes,
        content_type,
    })
}

// ── HLS pass-through ────────────────────────────────────────────────────────

/// One HLS asset (playlist `.m3u8` or segment `.ts` / `.m4s`) proxied
/// from the edge MediaMTX over Tailscale. The browser fetches the
/// playlist via Oxy, then fetches segments via Oxy — same URL prefix
/// so relative segment paths in the playlist resolve through us
/// without rewriting.
pub struct HlsAsset {
    pub bytes: Bytes,
    pub content_type: String,
}

/// Fetch one HLS asset (`tail` is the suffix after `/preview/hls/`,
/// e.g. `index.m3u8` or `seg000.ts`).
///
/// MediaMTX exposes `http://<edge>:8888/<path-name>/<file>`; we use
/// the camera id's simple form (no hyphens) as `<path-name>` so the
/// per-camera MTX config is `paths: cam-{simple}: source: <rtsp_url>`.
/// That config step is currently manual — a followup will populate
/// MTX paths from the `cameras` table via MTX's HTTP API.
///
/// Same guard chain as [`snapshot`]: workspace ownership →
/// `cam.edge_box_id` → `edge_box.tailscale_ip`.
pub async fn hls_fetch(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    camera_id: Uuid,
    tail: &str,
) -> ServiceResult<HlsAsset> {
    let cam = cameras::Entity::find_by_id(camera_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;

    let site = sites::Entity::find_by_id(cam.site_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    if site.workspace_id != workspace_id {
        return Err(ServiceError::Forbidden(
            "camera belongs to another workspace",
        ));
    }

    let edge_box_id = cam
        .edge_box_id
        .ok_or(ServiceError::Unavailable("camera not bound to an edge box"))?;
    let edge_box = edge_boxes::Entity::find_by_id(edge_box_id)
        .one(db)
        .await?
        .ok_or(ServiceError::Unavailable(
            "camera's edge box no longer exists",
        ))?;
    // Defense-in-depth: reject any `..` or scheme injection in `tail`.
    // The path is parsed from the URL by axum's wildcard extractor, so
    // it shouldn't contain these — but a tightly-scoped check costs
    // nothing and rules out a class of mistakes if the route shape
    // ever changes.
    if tail.contains("..") || tail.contains("://") {
        return Err(ServiceError::InvalidInput(
            "invalid path traversal in hls asset path".into(),
        ));
    }

    let port: u16 = std::env::var("OXY_CAMERAS_EDGE_HLS_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_EDGE_HLS_PORT);

    // MediaMTX path-name convention: `cam-<camera_id-simple>`. The
    // operator (or, later, the auto-config followup) is responsible
    // for registering this path in MediaMTX so the camera's RTSP
    // republishes as HLS.
    let mtx_path = format!("cam-{}", camera_id.simple());
    let base = upstream_base(&edge_box, port)?;
    let url = format!("{base}/{mtx_path}/{tail}");

    // Use the shared process-wide client so the MTX `cookieCheck=1`
    // cookie persists across playlist + variant + segment fetches.
    // See `http_client()` for why this matters.
    let resp = http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| ServiceError::Upstream(format!("GET {url}: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        // MediaMTX returns 404 when the path isn't configured yet
        // ("stream not configured for this camera"). Surface as 503
        // unavailable so the UI can show the same "preview not ready"
        // placeholder it shows for the unbound-camera case — a 404
        // would imply "this camera doesn't exist," which is wrong.
        return Err(ServiceError::Unavailable(
            "stream not configured on edge MediaMTX for this camera",
        ));
    }
    if !status.is_success() {
        return Err(ServiceError::Upstream(format!(
            "edge returned status {status}"
        )));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ServiceError::Upstream(format!("read body: {e}")))?;

    Ok(HlsAsset {
        bytes,
        content_type,
    })
}

// ── Recording playback (clip for compliance reports) ────────────────────────

/// One recording clip from MediaMTX's playback server. Body is the
/// concatenated fMP4 / MP4 of the requested time range; content-type
/// is whatever MTX returns (typically `video/mp4`).
pub struct RecordingClip {
    pub bytes: Bytes,
    pub content_type: String,
}

/// Fetch a recorded clip for `camera_id` starting at `start` (ISO-8601
/// or any string MTX accepts in its `?start=` query param) and
/// running for `duration_s` seconds.
///
/// MediaMTX's playback server (`playback: yes`, `playbackAddress :9996`
/// — see `mediamtx.yml`) serves recorded segments via
/// `GET /get?path=<name>&start=<start>&duration=<duration>` which
/// returns the concatenated bytes as a single MP4. We proxy that
/// through Oxy with the same workspace-ownership chain as live HLS
/// so an operator's browser never talks to MTX directly.
///
/// Same guard chain as [`snapshot`] / [`hls_fetch`]: workspace
/// ownership → `cam.edge_box_id` → `edge_box.tailscale_ip`.
pub async fn recording_clip(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    camera_id: Uuid,
    start: &str,
    duration_s: f64,
) -> ServiceResult<RecordingClip> {
    // Validate caller-supplied parameters before any DB I/O. A bad
    // request should never depend on whether the camera happens to
    // exist or be bound to an edge box — and `start` is being
    // interpolated into a URL below, so it gets a defense-in-depth
    // sniff for the obvious injection vectors (real MTX accepts
    // ISO-8601 + a handful of aliases; we don't try to validate
    // exactly here).
    if start.contains('&') || start.contains('?') || start.contains('\n') {
        return Err(ServiceError::InvalidInput(
            "invalid `start` value for recording playback".into(),
        ));
    }
    if !(0.1..=600.0).contains(&duration_s) {
        return Err(ServiceError::InvalidInput(
            "duration_s must be between 0.1 and 600 seconds".into(),
        ));
    }

    let cam = cameras::Entity::find_by_id(camera_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;

    let site = sites::Entity::find_by_id(cam.site_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    if site.workspace_id != workspace_id {
        return Err(ServiceError::Forbidden(
            "camera belongs to another workspace",
        ));
    }

    let edge_box_id = cam
        .edge_box_id
        .ok_or(ServiceError::Unavailable("camera not bound to an edge box"))?;
    let edge_box = edge_boxes::Entity::find_by_id(edge_box_id)
        .one(db)
        .await?
        .ok_or(ServiceError::Unavailable(
            "camera's edge box no longer exists",
        ))?;
    let port: u16 = std::env::var("OXY_CAMERAS_EDGE_PLAYBACK_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_EDGE_PLAYBACK_PORT);

    let mtx_path = format!("cam-{}", camera_id.simple());
    // MTX playback returns the concatenated MP4 directly via `/get`.
    // The `format=fmp4` query keeps the output streamable without
    // muxing through ffmpeg again.
    //
    // Build the query via `.query(&[...])` rather than `format!()` so
    // reqwest URL-encodes the timestamp (`:` → `%3A`, etc.). Some MTX
    // versions reject the raw `:` in `start=`, and other operators
    // upstream of us (Tailscale Funnel, intermediate proxies) treat
    // unencoded reserved chars in query strings inconsistently.
    let base = format!("{}/get", upstream_base(&edge_box, port)?);
    let duration_str = format!("{duration_s}");
    // MTX 1.18+ playback server: a credential MUST be present
    // (Basic Auth or `?user=X&pass=Y`) before MTX will even call
    // the external auth callback. Without one MTX 401s immediately
    // with `WWW-Authenticate: Basic realm="mediamtx"` and no log
    // entry. The values themselves don't matter — our callback at
    // `/control/mtx-auth` approves any non-WebRTC action — but
    // they have to be there. `oxy:any` is the convention used
    // across other Oxy → MTX call sites.
    let resp = http_client()
        .get(&base)
        .basic_auth("oxy", Some("any"))
        .query(&[
            ("path", mtx_path.as_str()),
            ("start", start),
            ("duration", duration_str.as_str()),
            ("format", "fmp4"),
        ])
        .send()
        .await
        .map_err(|e| ServiceError::Upstream(format!("GET {base}: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        // MTX returns 404 when no recording exists for the requested
        // window (camera wasn't being recorded then, or the clip
        // expired past `recordDeleteAfter`).
        return Err(ServiceError::Unavailable(
            "no recording for the requested window (expired or never captured)",
        ));
    }
    if !status.is_success() {
        // Pull the MTX response body so we surface MTX's actual
        // diagnostic instead of a generic 502. Truncate to 256B in
        // case MTX returns an HTML error page.
        let body = resp.text().await.unwrap_or_default();
        let snippet = body.chars().take(256).collect::<String>();
        tracing::warn!(
            camera_id = %camera_id,
            edge_box_id = %edge_box_id,
            mtx_url = %base,
            mtx_path = %mtx_path,
            start = %start,
            duration_s = duration_s,
            status = %status,
            body = %snippet,
            "MTX playback returned non-success"
        );
        return Err(ServiceError::Upstream(format!(
            "playback returned status {status}: {snippet}"
        )));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("video/mp4")
        .to_string();

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ServiceError::Upstream(format!("read body: {e}")))?;

    Ok(RecordingClip {
        bytes,
        content_type,
    })
}

// ── Recording inventory (which windows have footage) ───────────────────────

/// One contiguous recorded segment as MTX reports it. MTX records in
/// fixed-duration parts (typically minutes-long); contiguous parts get
/// rolled up into one entry. Gaps appear as separate entries.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecordingSegment {
    /// ISO-8601, in MTX's local time-as-UTC convention (mirrors the
    /// `recording_clip` path's accepted `start` shapes).
    pub start: String,
    /// Length of this contiguous block, in seconds.
    pub duration_seconds: f64,
}

/// MTX `/list` returns durations as Go's `time.Duration` strings
/// (e.g. `3600.000000s`, `1h0m0s`). Parse into seconds for the UI.
fn parse_mtx_duration(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    // Plain numeric ("3600", "3600.0", "3600.0s") — fast path.
    if let Some(stripped) = trimmed.strip_suffix('s')
        && let Ok(n) = stripped.parse::<f64>()
    {
        return Some(n);
    }
    if let Ok(n) = trimmed.parse::<f64>() {
        return Some(n);
    }
    // Compound form like `1h2m3.4s`. Walk the string once.
    let mut total = 0.0_f64;
    let mut cur = String::new();
    for ch in trimmed.chars() {
        match ch {
            '0'..='9' | '.' => cur.push(ch),
            'h' => {
                total += cur.parse::<f64>().ok()? * 3600.0;
                cur.clear();
            }
            'm' => {
                total += cur.parse::<f64>().ok()? * 60.0;
                cur.clear();
            }
            's' => {
                total += cur.parse::<f64>().ok()?;
                cur.clear();
            }
            _ => return None,
        }
    }
    Some(total)
}

/// List the contiguous recorded segments MTX has on disk for `camera_id`.
/// Used by the operator UI to (a) shade the timeline scrubber so it's
/// obvious where footage exists, and (b) snap free-scrub clicks onto
/// a valid segment instead of erroring with 503.
///
/// Same workspace-ownership guard chain as `recording_clip`. Returns
/// an empty Vec (not an error) when:
///   - MTX reports the path has no recordings at all (camera was just
///     bound, or `recordDeleteAfter` cleared everything), OR
///   - MTX returns a 404 for the path (no recording config yet).
pub async fn list_recordings(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    camera_id: Uuid,
) -> ServiceResult<Vec<RecordingSegment>> {
    let cam = cameras::Entity::find_by_id(camera_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    let site = sites::Entity::find_by_id(cam.site_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    if site.workspace_id != workspace_id {
        return Err(ServiceError::Forbidden(
            "camera belongs to another workspace",
        ));
    }

    let edge_box_id = cam
        .edge_box_id
        .ok_or(ServiceError::Unavailable("camera not bound to an edge box"))?;
    let edge_box = edge_boxes::Entity::find_by_id(edge_box_id)
        .one(db)
        .await?
        .ok_or(ServiceError::Unavailable(
            "camera's edge box no longer exists",
        ))?;
    let port: u16 = std::env::var("OXY_CAMERAS_EDGE_PLAYBACK_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_EDGE_PLAYBACK_PORT);
    let mtx_path = format!("cam-{}", camera_id.simple());
    let base = format!("{}/list", upstream_base(&edge_box, port)?);

    let resp = http_client()
        .get(&base)
        .basic_auth("oxy", Some("any"))
        .query(&[("path", mtx_path.as_str())])
        .send()
        .await
        .map_err(|e| ServiceError::Upstream(format!("GET {base}: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        // MTX returns 404 when the path has no recordings (yet). The
        // UI distinguishes this from a hard error by treating an
        // empty list as "no footage to seek into."
        return Ok(Vec::new());
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let snippet = body.chars().take(256).collect::<String>();
        tracing::warn!(
            camera_id = %camera_id,
            mtx_url = %base,
            status = %status,
            body = %snippet,
            "MTX /list returned non-success"
        );
        return Err(ServiceError::Upstream(format!(
            "/list returned status {status}: {snippet}"
        )));
    }

    // MTX `/list` returns an array of objects. The shape MTX 1.x uses:
    //   [{"start":"2026-06-01T10:00:00Z","duration":"3600.000000s"}, …]
    // The `duration` is sometimes numeric, sometimes a `time.Duration`
    // string; we tolerate both via `parse_mtx_duration`.
    #[derive(serde::Deserialize)]
    struct MtxEntry {
        start: String,
        duration: serde_json::Value,
    }
    let entries: Vec<MtxEntry> = resp
        .json()
        .await
        .map_err(|e| ServiceError::Upstream(format!("parse /list: {e}")))?;
    Ok(entries
        .into_iter()
        .filter_map(|e| {
            let secs = match e.duration {
                serde_json::Value::Number(n) => n.as_f64(),
                serde_json::Value::String(s) => parse_mtx_duration(&s),
                _ => None,
            }?;
            Some(RecordingSegment {
                start: e.start,
                duration_seconds: secs,
            })
        })
        .collect())
}

#[cfg(test)]
mod recording_inventory_tests {
    use super::parse_mtx_duration;

    #[test]
    fn parses_plain_seconds() {
        assert_eq!(parse_mtx_duration("3600"), Some(3600.0));
        assert_eq!(parse_mtx_duration("3600.5"), Some(3600.5));
        assert_eq!(parse_mtx_duration("3600s"), Some(3600.0));
        assert_eq!(parse_mtx_duration("3600.000000s"), Some(3600.0));
    }

    #[test]
    fn parses_go_duration_compound() {
        assert_eq!(parse_mtx_duration("1h0m0s"), Some(3600.0));
        assert_eq!(parse_mtx_duration("1h2m3s"), Some(3723.0));
        assert_eq!(parse_mtx_duration("0h0m1.5s"), Some(1.5));
        assert_eq!(parse_mtx_duration("2m"), Some(120.0));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_mtx_duration("nope"), None);
        assert_eq!(parse_mtx_duration("1z"), None);
    }
}
