import { apiClient } from "./axios";

// ── Response types (mirror crates/cameras/src/routes/dto.rs) ────────────────

export type Site = {
  id: string;
  workspace_id: string;
  name: string;
  timezone: string;
  region: string | null;
  /** `manual` (operator-created) or `unifi` (imported via UniFi onboarding). */
  source: string;
  /**
   * WAN-facing IP for pre-edge-box compliance — set on the site detail
   * UI when the customer hasn't received hardware yet. Drives the bulk
   * RTSP rewrite endpoint that swaps LAN UniFi URLs onto the public IP
   * + WAN-forwarded RTSP port.
   */
  public_ip: string | null;
  created_at: string;
  updated_at: string;
};

/**
 * One result row from `POST /cameras/sites/:id/bulk-rtsp-rewrite`.
 * `outcome` discriminates which fields are populated:
 *   - `rewritten`     → camera_id / camera_name / old_url / new_url
 *   - `no_match`      → camera_name + error
 *   - `parse_error`   → error
 *   - `invalid_url`   → camera_name (if parseable) + old_url + error
 *   - `ambiguous_name`→ camera_name + error
 */
type BulkRewriteOutcome =
  | "rewritten"
  | "no_match"
  | "parse_error"
  | "invalid_url"
  | "ambiguous_name";

export type BulkRewriteItem = {
  line: string;
  outcome: BulkRewriteOutcome;
  camera_id: string | null;
  camera_name: string | null;
  old_url: string | null;
  new_url: string | null;
  error: string | null;
};

type BulkRewriteSummary = {
  rewritten: number;
  no_match: number;
  parse_error: number;
  invalid_url: number;
  ambiguous_name: number;
};

export type BulkRewriteResult = {
  items: BulkRewriteItem[];
  summary: BulkRewriteSummary;
};

/** Result of `POST /cameras/sites/:id/resync-public-ip`. */
export type ResyncPublicIpResult = {
  site_id: string;
  old_public_ip: string | null;
  new_public_ip: string | null;
  changed: boolean;
};

/**
 * Status of the workspace's stored UniFi API key. Drives the
 * "configured ✓" badge in the settings UI and lets the EditSiteDialog
 * skip its inline API-key prompt when a key is already on file. Never
 * carries the secret itself.
 */
export type UnifiCredentialStatus = {
  configured: boolean;
  set_at: string | null;
  updated_at: string | null;
  set_by_user_id: string | null;
};

export type EdgeBox = {
  id: string;
  site_id: string;
  hardware_model: string;
  image_tag: string;
  cohort: string;
  tailscale_ip: string | null;
  status: string;
  unifi_console_id: string | null;
  unifi_public_ip: string | null;
  unifi_rtsp_reachable: boolean;
  registered_at: string;
  last_seen_at: string | null;
  /**
   * MTX outbound-bytes delta over the last reporter window (~5
   * min). Stamped by `POST /control/boxes/{id}/bandwidth`. Used as
   * an early warning before the per-tailnet Funnel quota bites.
   */
  bandwidth_5min_bytes: number | null;
  bandwidth_reported_at: string | null;
  /**
   * IoT Phase 2 — software-update channel. Operator sets target;
   * device reports current via the health endpoint. UI shows a
   * "pending update" indicator when target ≠ current.
   */
  target_image_tag: string | null;
  current_image_tag: string | null;
  held_until: string | null;
  /** One of `applied` | `reverted` | `in_progress`, or null. */
  last_update_result: string | null;
  last_update_at: string | null;
  /**
   * CI #3 — last compatibility manifest the worker reported. Shape:
   * `{edge_version, git_sha, built_at, protocol_version, min_server_version}`.
   * `null` when the box runs an older worker that doesn't ship this
   * manifest yet — UI falls back to dash in that case.
   */
  edge_compatibility_json: {
    edge_version?: string;
    git_sha?: string;
    built_at?: string;
    protocol_version?: number;
    min_server_version?: string;
  } | null;
  /**
   * P1 #3 — non-empty when the worker's posted protocol_version is
   * below the server's minimum (OXY_MIN_EDGE_PROTOCOL_VERSION).
   * UI surfaces an amber "needs upgrade" badge with this as tooltip.
   */
  incompatible_reason: string | null;
  updated_at: string;
};

/** List shape (the summary DTO flattens `site_name` next to the edge box fields). */
export type EdgeBoxSummary = EdgeBox & {
  site_name: string;
};

export type Camera = {
  id: string;
  site_id: string;
  edge_box_id: string | null;
  name: string;
  rtsp_url: string | null;
  substream_url: string | null;
  credentials_ref: string | null;
  zones_json: unknown;
  lines_json: unknown;
  analytics_consent: boolean;
  active: boolean;
  protect_camera_id: string | null;
  mac_address: string | null;
  model: string | null;
  online: boolean | null;
  /**
   * Role tag the worker uses to pick a VLM prompt. One of
   * `"kitchen" | "prep" | "dining" | "other"`, or `null` for
   * unset (the worker falls back to the general whole-frame prompt).
   * Validated against the same set on the backend.
   */
  role: string | null;
  /**
   * Operator zone-approval automation: pending proposal slot.
   * `null` = no proposal. Distinct from an empty array — that
   * would CLEAR live zones on approval, not "no change."
   */
  proposed_zones_json: unknown | null;
  proposed_lines_json: unknown | null;
  proposed_at: string | null;
  created_at: string;
  updated_at: string;
};

export type CameraSummary = Camera & {
  site_name: string;
  edge_box_name: string | null;
  /**
   * One of `pending` | `active` | `offline` | `retired`, or `null`
   * for cameras not bound to an edge box yet. UI surfaces `offline`
   * as a warning even when the camera itself reports `online: true`,
   * since the snapshot / HLS / WebRTC proxies all dial the box back.
   */
  edge_box_status: string | null;
};

// ── Compliance reports (Airhouse-backed) ────────────────────────────────────

/**
 * One row of the Compliance tab. Mirrors `ComplianceReportDto` in
 * crates/cameras/src/routes/dto.rs. Timestamps arrive as ISO-8601
 * strings (chrono::DateTime<Utc>); the UI parses them on demand.
 */
export type ComplianceReport = {
  report_id: string;
  camera_id: string;
  segment_start: string;
  segment_end: string;
  trigger_type: string;
  trigger_track_id: string | null;
  vlm_model: string;
  report_text: string;
  /** Already-parsed JSON; the server decoded the VARCHAR before sending. */
  structured_json: unknown;
  frame_uri: string | null;
  tokens_used: number | null;
  /**
   * Tier B #6 — bucket-relative S3 key for the archived
   * dwell-window clip. NULL when the worker didn't upload
   * (no violation, OXY_CAMERAS_CLIPS_S3_BUCKET unset, or upload failed).
   * The UI surfaces a "View clip" affordance when set.
   */
  evidence_s3_key: string | null;
  /**
   * P3 (Option C) — PPE-YOLO detections captured by the edge worker
   * on the same frame the VLM scored. The VLM verdict stays
   * authoritative; this drives the optional bbox overlay on the
   * Playback page. `null` = edge worker pre-dates Option C; the
   * envelope's inner `detections` array being empty means "YOLO ran,
   * saw nothing." See `crates/cameras/src/service/compliance.rs`
   * `CompliancePayload::detections_json` for the contract.
   */
  detections_json: PpeDetectionsEnvelope | null;
  /**
   * VLM ↔ YOLO agreement signal computed at ingest. `null` on rows
   * ingested before Phase 1 of disagreement utilization landed. The
   * UI filters on this for the review queue.
   */
  agreement_status: AgreementStatus | null;
  /**
   * Per-class breakdown — see the contract on `agreement_detail_json`
   * in the Rust ComplianceReportRow. `null` on legacy rows.
   */
  agreement_detail_json: AgreementDetail | null;
  received_at: string;
};

type AgreementStatus = "agree" | "disagree" | "inconclusive";

type ClassAgreement = "agree" | "vlm_only_missing" | "yolo_only_missing";

type AgreementDetail = {
  per_class: Record<string, ClassAgreement>;
};

// ── Arbitrations ─────────────────────────────────────────────────────────
// Operator verdicts on VLM ↔ YOLO disagreements. One row per report;
// PUT-with-same-report_id upserts.

export type ArbitrationVerdict = "vlm_right" | "yolo_right" | "neither";

export type ArbitrationRow = {
  id: string;
  workspace_id: string;
  report_id: string;
  arbiter_user_id: string | null;
  verdict: ArbitrationVerdict;
  notes: string | null;
  created_at: string;
  updated_at: string;
};

export type PpeDetectionsEnvelope = {
  model: string;
  /** Ms into `segment_start` when the YOLO snapshot was taken. */
  frame_offset_ms: number;
  /** Pixel dims of the source frame YOLO ran on (for sanity / debug). */
  frame_width: number;
  frame_height: number;
  detections: PpeDetection[];
};

type PpeDetection = {
  /** YOLO class label — e.g. "person", "hat", "hairnet", "apron", "glove", "mask". */
  class: string;
  /** Model confidence in [0,1]. */
  confidence: number;
  /** Normalized [0,1] coords, top-left origin. */
  bbox: { x: number; y: number; width: number; height: number };
};

export type ComplianceListParams = {
  /** ISO-8601; omit to get the most recent rows regardless of age. */
  since?: string;
  /** Default 50, server caps at 500. */
  limit?: number;
};

/**
 * One row of the site-rollup endpoint: per-camera incident counts.
 * Mirrors `CameraComplianceSummaryDto` in crates/cameras/src/routes/dto.rs.
 *
 * Cameras with zero reports still appear (with `total_reports: 0` /
 * `violations: 0` / `last_received_at: null`) so the UI can show
 * configured-but-clean cameras as a distinct state from
 * not-configured-yet.
 */
/**
 * Per-session WebRTC configuration. Mirrors `WebrtcSessionDto` in
 * crates/cameras/src/routes/dto.rs. The browser uses `whep_url` to
 * POST its SDP offer and `ice_servers` on the `RTCPeerConnection`
 * constructor for NAT traversal / TURN-over-TLS via Tailscale
 * Funnel.
 */
export type WebrtcSession = {
  whep_url: string;
  ice_servers: {
    urls: string[];
    username: string;
    credential: string;
  }[];
  expires_at: string;
};

/**
 * Canary rollout plan for an OTA target_image_tag. Mirrors
 * `oxy_cameras::entities::rollout_plans::Model`. State machine
 * lives server-side (see `service::rollouts`); the UI just
 * displays + creates + aborts.
 */
export type RolloutPlan = {
  id: string;
  workspace_id: string;
  target_image_tag: string;
  canary_cohort: string;
  canary_min_minutes: number;
  failure_threshold_pct: number;
  /** "pending" | "canary" | "promoting" | "complete" | "aborted". */
  status: string;
  canary_started_at: string | null;
  promotion_started_at: string | null;
  completed_at: string | null;
  aborted_at: string | null;
  abort_reason: string | null;
  actor_user_id: string | null;
  created_at: string;
  updated_at: string;
};

export type CreateRolloutBody = {
  target_image_tag: string;
  canary_cohort: string;
  /** Default 1440 (24h). */
  canary_min_minutes?: number;
  /** Default 5.0%. */
  failure_threshold_pct?: number;
};

/**
 * Per-box row used by the rollout detail page. `convergence_status`
 * is the coarse signal:
 *   - `converged`  — box reports `current_image_tag == plan target`
 *   - `failed`     — box reported a failure result during this rollout
 *   - `pending`    — apply went out, no signal yet
 *   - `untargeted` — apply hasn't reached this box (promoting phase
 *                    hasn't started + box is not in canary cohort)
 */
export type RolloutConvergenceRow = {
  edge_box_id: string;
  hardware_model: string;
  cohort: string;
  site_name: string;
  current_image_tag: string | null;
  last_update_result: string | null;
  last_update_at: string | null;
  in_canary: boolean;
  convergence_status: "converged" | "failed" | "pending" | "untargeted";
};

export type CameraComplianceSummary = {
  camera_id: string;
  camera_name: string;
  edge_box_name: string | null;
  total_reports: number;
  violations: number;
  last_received_at: string | null;
};

// ── Dashboard rollup DTOs ──────────────────────────────────────────────────
// Mirrors `oxy_cameras::service::dashboard::DashboardRollup`. Returned
// by `GET /{wid}/fleet/dashboard?since=&site_id=?` — single round-trip
// replacement for the per-site fan-out the dashboard used in v0.

type DashboardTotals = {
  detections: number;
  violations: number;
  cameras_total: number;
  cameras_with_activity: number;
};

type HourlyBucket = {
  hour: string;
  detections: number;
  violations: number;
};

type DashboardCameraStat = {
  camera_id: string;
  camera_name: string;
  detections: number;
  violations: number;
};

export type DashboardRollup = {
  totals: DashboardTotals;
  hourly: HourlyBucket[];
  top_by_detections: DashboardCameraStat[];
  top_by_violations: DashboardCameraStat[];
};

export type RecordingSegment = {
  start: string;
  duration_seconds: number;
};

export type RecentAlertRow = {
  id: string;
  created_at: string;
  camera_id: string | null;
  camera_name: string | null;
  site_id: string | null;
  site_name: string | null;
  from: string;
  to: string;
  reason: string;
  delivered: boolean | null;
};

// ── Onboarding (UniFi) DTOs ─────────────────────────────────────────────────
// Mirrors `oxy_cameras::service::onboarding::SitePreview`. The Rust
// struct is the source of truth — these field names are what serde
// actually serializes, NOT a guess (an earlier guess shipped with
// `host_id` / `host_name` / `cameras` which 404'd every cell at
// render time).

type UnifiPreviewSite = {
  unifi_console_id: string;
  name: string;
  public_ip: string | null;
  camera_count: number;
  online_camera_count: number;
  hardware_model: string | null;
};

export type UnifiPreviewResult = {
  sites: UnifiPreviewSite[];
  total_sites: number;
  total_cameras: number;
};

export type UnifiImportResult = {
  sites_upserted: number;
  edge_boxes_upserted: number;
  cameras_upserted: number;
  skipped_no_workspace: number;
};

// ── Service ─────────────────────────────────────────────────────────────────

export const CameraService = {
  async listSites(workspaceId: string): Promise<Site[]> {
    const response = await apiClient.get(`/${workspaceId}/cameras/sites`);
    return response.data;
  },

  async getSite(workspaceId: string, siteId: string): Promise<Site> {
    const response = await apiClient.get(`/${workspaceId}/cameras/sites/${siteId}`);
    return response.data;
  },

  async listEdgeBoxes(workspaceId: string): Promise<EdgeBoxSummary[]> {
    const response = await apiClient.get(`/${workspaceId}/cameras/edge-boxes`);
    return response.data;
  },

  async getEdgeBox(workspaceId: string, boxId: string): Promise<EdgeBox> {
    const response = await apiClient.get(`/${workspaceId}/cameras/edge-boxes/${boxId}`);
    return response.data;
  },

  async listCameras(workspaceId: string): Promise<CameraSummary[]> {
    const response = await apiClient.get(`/${workspaceId}/cameras`);
    return response.data;
  },

  async getCamera(workspaceId: string, cameraId: string): Promise<Camera> {
    const response = await apiClient.get(`/${workspaceId}/cameras/${cameraId}`);
    return response.data;
  },

  // ── UniFi onboarding ─────────────────────────────────────────────────────

  async unifiPreview(workspaceId: string, apiKey?: string): Promise<UnifiPreviewResult> {
    // Omitting `api_key` falls back to the stored workspace credential
    // server-side (see `service::unifi_credentials::resolve_api_key`).
    // Used by the "Scan sites" action that re-checks the customer's
    // UniFi account for new sites after the initial onboarding.
    const response = await apiClient.post(`/${workspaceId}/integrations/unifi/preview`, {
      api_key: apiKey
    });
    return response.data;
  },

  async unifiImport(
    workspaceId: string,
    apiKey?: string,
    consoleIds?: string[]
  ): Promise<UnifiImportResult> {
    const response = await apiClient.post(`/${workspaceId}/integrations/unifi/import`, {
      api_key: apiKey,
      console_ids: consoleIds
    });
    return response.data;
  },

  // ── Compliance ───────────────────────────────────────────────────────────

  async listComplianceReports(
    workspaceId: string,
    cameraId: string,
    params: ComplianceListParams = {}
  ): Promise<ComplianceReport[]> {
    const response = await apiClient.get(`/${workspaceId}/cameras/${cameraId}/compliance-reports`, {
      params
    });
    return response.data;
  },

  async listSiteComplianceSummary(
    workspaceId: string,
    siteId: string,
    params: { since?: string } = {}
  ): Promise<CameraComplianceSummary[]> {
    const response = await apiClient.get(
      `/${workspaceId}/cameras/sites/${siteId}/compliance-summary`,
      { params }
    );
    return response.data;
  },

  /**
   * Fleet-wide compliance reports — newest first, across the workspace
   * (or one site). Replaces per-camera fan-out from the Detections
   * page. One Airhouse round-trip regardless of camera count.
   */
  async listFleetComplianceReports(
    workspaceId: string,
    params: { site_id?: string | null; since?: string; limit?: number } = {}
  ): Promise<ComplianceReport[]> {
    const response = await apiClient.get<ComplianceReport[]>(
      `/${workspaceId}/fleet/compliance-reports`,
      {
        params: {
          site_id: params.site_id ?? undefined,
          since: params.since,
          limit: params.limit
        }
      }
    );
    return response.data;
  },

  /**
   * Per-camera totals across the workspace (or one site). Replaces
   * per-site fan-out from the Playback Grid view. One Airhouse
   * round-trip regardless of site count.
   */
  async listFleetComplianceSummary(
    workspaceId: string,
    params: { site_id?: string | null; since?: string } = {}
  ): Promise<CameraComplianceSummary[]> {
    const response = await apiClient.get<CameraComplianceSummary[]>(
      `/${workspaceId}/fleet/compliance-summary`,
      {
        params: {
          site_id: params.site_id ?? undefined,
          since: params.since
        }
      }
    );
    return response.data;
  },

  /**
   * Fetch the recorded clip as a Blob.
   *
   * We can't just set the endpoint URL as `<video src>` directly:
   * - Native `<video>` has no hook to inject Authorization headers, so
   *   we'd need `crossOrigin="use-credentials"` to ride cookies.
   * - But the dev server returns `Access-Control-Allow-Origin: *`,
   *   which the browser refuses to combine with credentials mode.
   *
   * Pulling the bytes via `apiClient` (which already carries the
   * bearer + uses the Vite dev proxy in development) and wrapping
   * the response in a blob URL sidesteps the whole CORS dance —
   * the resulting `blob:` URL is same-origin from the browser's
   * perspective.
   *
   * Clip sizes are bounded (5–60s of ~200 kbps fMP4 ≈ 1–12 MB) so
   * the upfront download cost is negligible.
   */
  /**
   * Which time windows MTX still has recordings for. Drives the
   * scrubber's shaded bands + the bare-scrub "snap to footage"
   * behavior so the operator can't click into a gap and get a 503.
   * Empty list = no footage at all (camera just bound, or retention
   * cleared everything).
   */
  async listRecordingSegments(workspaceId: string, cameraId: string): Promise<RecordingSegment[]> {
    const response = await apiClient.get<RecordingSegment[]>(
      `/${workspaceId}/cameras/${cameraId}/preview/recording-segments`
    );
    return response.data;
  },

  async fetchRecordingClip(
    workspaceId: string,
    cameraId: string,
    start: string,
    durationSeconds: number
  ): Promise<Blob> {
    try {
      const response = await apiClient.get(
        `/${workspaceId}/cameras/${cameraId}/preview/recording`,
        {
          params: { start, duration: durationSeconds },
          responseType: "blob"
        }
      );
      return response.data;
    } catch (err) {
      // `responseType: "blob"` applies to both success and error bodies,
      // so axios hands us the backend's JSON error envelope as a Blob.
      // Re-parse it into `error.response.data` so `apiErrorMessage`
      // picks up the real "no recording for the requested window…"
      // diagnostic instead of falling back to axios's generic
      // "Request failed with status code 503".
      if (err && typeof err === "object" && "response" in err) {
        const e = err as { response?: { data?: unknown } };
        if (e.response?.data instanceof Blob) {
          try {
            const text = await e.response.data.text();
            e.response.data = JSON.parse(text);
          } catch {
            // Body wasn't JSON — leave the Blob in place; apiErrorMessage
            // falls back to e.message which is fine for non-API errors.
          }
        }
      }
      throw err;
    }
  },

  /**
   * Mint a WebRTC live-preview session. Returns the per-box Funnel
   * URL the browser POSTs its SDP offer at + an ephemeral TURN
   * credential (~5min TTL).
   *
   * Throws on any failure shape (503 box-has-no-funnel-hostname,
   * 503 secret-not-configured, 403 cross-workspace, etc.). The
   * caller catches and falls back to HLS — there's no recovering
   * a WebRTC session at this layer.
   */
  async requestWebrtcSession(workspaceId: string, cameraId: string): Promise<WebrtcSession> {
    const response = await apiClient.post<WebrtcSession>(
      `/${workspaceId}/cameras/${cameraId}/preview/webrtc/session`
    );
    return response.data;
  },

  // ── Domain packs ─────────────────────────────────────────────────────────

  async getActivePack(workspaceId: string): Promise<DomainPack> {
    const response = await apiClient.get<DomainPack>(`/${workspaceId}/camera-packs/active`);
    return response.data;
  },

  async updatePackBody(
    workspaceId: string,
    packKey: string,
    body: DomainPackBody
  ): Promise<DomainPack> {
    const response = await apiClient.put<DomainPack>(
      `/${workspaceId}/camera-packs/${packKey}/body`,
      { body }
    );
    return response.data;
  },

  async listPacks(workspaceId: string): Promise<DomainPackSummary[]> {
    const response = await apiClient.get<DomainPackSummary[]>(`/${workspaceId}/camera-packs`);
    return response.data;
  },

  async listStarters(workspaceId: string): Promise<StarterPackSummary[]> {
    const response = await apiClient.get<StarterPackSummary[]>(
      `/${workspaceId}/camera-packs/starters`
    );
    return response.data;
  },

  async forkPack(workspaceId: string, starterKey: string): Promise<DomainPack> {
    const response = await apiClient.post<DomainPack>(`/${workspaceId}/camera-packs`, {
      starter_key: starterKey
    });
    return response.data;
  },

  async activatePack(workspaceId: string, packKey: string): Promise<DomainPack> {
    const response = await apiClient.post<DomainPack>(
      `/${workspaceId}/camera-packs/${packKey}/activate`
    );
    return response.data;
  },

  async listStarterUpdates(workspaceId: string): Promise<PackStarterUpdate[]> {
    const response = await apiClient.get<PackStarterUpdate[]>(
      `/${workspaceId}/camera-packs/starter-updates`
    );
    return response.data;
  },

  async applyStarterUpdate(
    workspaceId: string,
    packKey: string,
    actions: Record<string, UpdateAction>
  ): Promise<DomainPack> {
    const response = await apiClient.post<DomainPack>(
      `/${workspaceId}/camera-packs/${packKey}/apply-update`,
      { actions }
    );
    return response.data;
  },

  /**
   * IoT Phase 2 — set the target image tag (and optional hold
   * window) for an edge box. The device's update agent picks the
   * target up on its next poll and pulls + restarts the worker
   * container. Pass `target_image_tag: null` to clear (disable
   * automated updates).
   */
  async patchEdgeBoxCohort(workspaceId: string, boxId: string, cohort: string): Promise<EdgeBox> {
    const response = await apiClient.patch<EdgeBox>(
      `/${workspaceId}/cameras/edge-boxes/${boxId}/cohort`,
      { cohort }
    );
    return response.data;
  },

  async patchEdgeBoxTarget(
    workspaceId: string,
    boxId: string,
    body: { target_image_tag: string | null; held_until?: string | null }
  ): Promise<EdgeBox> {
    const response = await apiClient.patch<EdgeBox>(
      `/${workspaceId}/cameras/edge-boxes/${boxId}/target`,
      body
    );
    return response.data;
  },

  /**
   * Bulk variant of `patchEdgeBoxTarget`. The backend validates
   * that every `box_ids` entry belongs to the workspace before
   * touching the DB — a single cross-workspace id fails the whole
   * call with 403. Empty list returns `{updated_count: 0}` without
   * error.
   */
  async bulkPatchEdgeBoxTarget(
    workspaceId: string,
    body: {
      box_ids: string[];
      target_image_tag: string | null;
      held_until?: string | null;
    }
  ): Promise<{ updated_count: number }> {
    const response = await apiClient.patch<{ updated_count: number }>(
      `/${workspaceId}/cameras/edge-boxes/bulk/target`,
      body
    );
    return response.data;
  },

  // ── Manual provisioning ──────────────────────────────────────────────────

  async createSite(
    workspaceId: string,
    body: { name: string; timezone?: string; region?: string | null }
  ): Promise<Site> {
    const response = await apiClient.post<Site>(`/${workspaceId}/cameras/sites`, body);
    return response.data;
  },

  /**
   * Update a site. Each optional field follows the
   * `undefined`-means-no-change / `null`-means-clear convention the
   * Rust route enforces via serde's double-option pattern.
   */
  async updateSite(
    workspaceId: string,
    siteId: string,
    body: {
      public_ip?: string | null;
      region?: string | null;
      timezone?: string;
    }
  ): Promise<Site> {
    const response = await apiClient.patch<Site>(`/${workspaceId}/cameras/sites/${siteId}`, body);
    return response.data;
  },

  /**
   * Bulk-rewrite each camera's RTSP URL onto the site's `public_ip` +
   * UniFi WAN port 7447. Lines look like
   * `Camera Name, rtsps://<lan>:7441/<stream_id>`; the parser pulls
   * the stream_id, drops it onto `rtsp://<public_ip>:7447/<stream>`,
   * matches the camera by name, updates the row. Result enumerates
   * per-line outcomes so the UI can flag mismatches.
   */
  async bulkRewriteSiteCameraRtsp(
    workspaceId: string,
    siteId: string,
    lines: string[]
  ): Promise<BulkRewriteResult> {
    const response = await apiClient.post<BulkRewriteResult>(
      `/${workspaceId}/cameras/sites/${siteId}/bulk-rtsp-rewrite`,
      { lines }
    );
    return response.data;
  },

  /**
   * Re-fetch the UniFi controller's current WAN IP for one site and
   * update `sites.public_ip` in place. Operator hands in the UniFi
   * API key each time today — credentials aren't persisted yet.
   * Returns the diff so the UI can flash "still X" vs "updated to Y".
   */
  async resyncSitePublicIp(
    workspaceId: string,
    siteId: string,
    /** Omit when the workspace has a stored UniFi credential — the
     *  backend's `resolve_api_key` falls back to storage. */
    apiKey?: string | null
  ): Promise<ResyncPublicIpResult> {
    const body = apiKey ? { api_key: apiKey } : {};
    const response = await apiClient.post<ResyncPublicIpResult>(
      `/${workspaceId}/cameras/sites/${siteId}/resync-public-ip`,
      body
    );
    return response.data;
  },

  // ── UniFi credential storage ─────────────────────────────────────────────

  async getUnifiCredentialStatus(workspaceId: string): Promise<UnifiCredentialStatus> {
    const response = await apiClient.get<UnifiCredentialStatus>(
      `/${workspaceId}/cameras/unifi-credentials/status`
    );
    return response.data;
  },

  async setUnifiCredential(workspaceId: string, apiKey: string): Promise<UnifiCredentialStatus> {
    const response = await apiClient.put<UnifiCredentialStatus>(
      `/${workspaceId}/cameras/unifi-credentials`,
      { api_key: apiKey }
    );
    return response.data;
  },

  async deleteUnifiCredential(workspaceId: string): Promise<void> {
    await apiClient.delete(`/${workspaceId}/cameras/unifi-credentials`);
  },

  /**
   * Upsert an operator arbitration for a compliance report. PUTting
   * the same report_id replaces the existing verdict — the table
   * holds the operator's current belief, not a history.
   */
  async putArbitration(
    workspaceId: string,
    body: { report_id: string; verdict: ArbitrationVerdict; notes?: string | null }
  ): Promise<ArbitrationRow> {
    const response = await apiClient.put<ArbitrationRow>(
      `/${workspaceId}/cameras/arbitrations`,
      body
    );
    return response.data;
  },

  /** Fetch one arbitration by report_id. Returns null on 404. */
  async getArbitration(workspaceId: string, reportId: string): Promise<ArbitrationRow | null> {
    try {
      const response = await apiClient.get<ArbitrationRow>(
        `/${workspaceId}/cameras/arbitrations/${encodeURIComponent(reportId)}`
      );
      return response.data;
    } catch (err) {
      if (err && typeof err === "object" && "response" in err) {
        const r = (err as { response?: { status?: number } }).response;
        if (r?.status === 404) return null;
      }
      throw err;
    }
  },

  async deleteArbitration(workspaceId: string, reportId: string): Promise<void> {
    await apiClient.delete(`/${workspaceId}/cameras/arbitrations/${encodeURIComponent(reportId)}`);
  },

  async createCamera(
    workspaceId: string,
    body: {
      site_id: string;
      name: string;
      rtsp_url: string;
      substream_url?: string | null;
      credentials_ref?: string | null;
    }
  ): Promise<Camera> {
    const response = await apiClient.post<Camera>(`/${workspaceId}/cameras`, body);
    return response.data;
  },

  // ── VLM cost rollups (IoT Phase 4a) ──────────────────────────────────────

  async getEdgeBoxCost(
    workspaceId: string,
    boxId: string,
    range: CostRange = "24h"
  ): Promise<EdgeBoxCost> {
    const response = await apiClient.get<EdgeBoxCost>(
      `/${workspaceId}/cameras/edge-boxes/${boxId}/cost`,
      { params: { range } }
    );
    return response.data;
  },

  async getWorkspaceCost(workspaceId: string, range: CostRange = "24h"): Promise<WorkspaceCost> {
    const response = await apiClient.get<WorkspaceCost>(`/${workspaceId}/cameras/cost`, {
      params: { range }
    });
    return response.data;
  },

  // ── Device logs (IoT Phase 4b) ───────────────────────────────────────────

  async getEdgeBoxLogs(
    workspaceId: string,
    boxId: string,
    filter: LogFilter = {}
  ): Promise<LogRow[]> {
    const response = await apiClient.get<LogRow[]>(
      `/${workspaceId}/cameras/edge-boxes/${boxId}/logs`,
      { params: cleanLogParams(filter) }
    );
    return response.data;
  },

  // ── VLM budget (IoT Phase 4c) ────────────────────────────────────────────

  async getBudgetStatus(workspaceId: string): Promise<BudgetStatus> {
    const response = await apiClient.get<BudgetStatus>(`/${workspaceId}/cameras/budget`);
    return response.data;
  },

  async setBudget(
    workspaceId: string,
    monthlyBudgetMicros: number | null
  ): Promise<{ monthly_budget_micros: number | null }> {
    const response = await apiClient.patch<{ monthly_budget_micros: number | null }>(
      `/${workspaceId}/cameras/budget`,
      { monthly_budget_micros: monthlyBudgetMicros }
    );
    return response.data;
  },

  // ── Fleet (IoT Phase 5) ──────────────────────────────────────────────────

  async listFleetDevices(workspaceId: string): Promise<FleetDeviceRow[]> {
    const response = await apiClient.get<FleetDeviceRow[]>(`/${workspaceId}/fleet/devices`);
    return response.data;
  },

  async bindExistingDevice(
    workspaceId: string,
    body: { device_id: string; edge_box_id: string }
  ): Promise<{ claim_id: string; device_id: string; status: string }> {
    const response = await apiClient.post<{
      claim_id: string;
      device_id: string;
      status: string;
    }>(`/${workspaceId}/fleet/claims/bind-existing`, body);
    return response.data;
  },

  async prepareDevice(
    workspaceId: string,
    body: {
      site_id: string;
      hardware_label?: string;
      /** Optional Tailscale auth-key. When provided, the backend
       *  bakes `--ts-authkey <key>` into the install command so the
       *  new box joins the tailnet on first boot. Pass-through only;
       *  not persisted server-side. */
      ts_authkey?: string;
    }
  ): Promise<PreparedDeviceResponse> {
    const response = await apiClient.post<PreparedDeviceResponse>(
      `/${workspaceId}/fleet/prepared-devices`,
      body
    );
    return response.data;
  },

  async listRolloutPlans(workspaceId: string): Promise<RolloutPlan[]> {
    const response = await apiClient.get<RolloutPlan[]>(`/${workspaceId}/fleet/rollouts`);
    return response.data;
  },

  async createRolloutPlan(workspaceId: string, body: CreateRolloutBody): Promise<RolloutPlan> {
    const response = await apiClient.post<RolloutPlan>(`/${workspaceId}/fleet/rollouts`, body);
    return response.data;
  },

  async getRolloutConvergence(
    workspaceId: string,
    planId: string
  ): Promise<RolloutConvergenceRow[]> {
    const response = await apiClient.get<RolloutConvergenceRow[]>(
      `/${workspaceId}/fleet/rollouts/${planId}/convergence`
    );
    return response.data;
  },

  async abortRolloutPlan(
    workspaceId: string,
    planId: string,
    reason?: string
  ): Promise<RolloutPlan> {
    const response = await apiClient.post<RolloutPlan>(
      `/${workspaceId}/fleet/rollouts/${planId}/abort`,
      { reason }
    );
    return response.data;
  },

  async getDashboardRollup(
    workspaceId: string,
    params: { since?: string; site_id?: string | null } = {}
  ): Promise<DashboardRollup> {
    const response = await apiClient.get<DashboardRollup>(`/${workspaceId}/fleet/dashboard`, {
      params: {
        since: params.since,
        site_id: params.site_id ?? undefined
      }
    });
    return response.data;
  },

  async listRecentAlerts(
    workspaceId: string,
    params: { site_id?: string | null; limit?: number } = {}
  ): Promise<RecentAlertRow[]> {
    const response = await apiClient.get<RecentAlertRow[]>(`/${workspaceId}/fleet/alerts`, {
      params: {
        site_id: params.site_id ?? undefined,
        limit: params.limit
      }
    });
    return response.data;
  },

  async listFleetMembers(workspaceId: string): Promise<FleetMemberRow[]> {
    const response = await apiClient.get<FleetMemberRow[]>(`/${workspaceId}/cameras/fleet`);
    return response.data;
  },

  async removeFleetMember(
    workspaceId: string,
    body: { edge_box_id?: string | null; device_id?: string | null }
  ): Promise<RemoveFleetMemberResult> {
    const response = await apiClient.delete<RemoveFleetMemberResult>(
      `/${workspaceId}/cameras/fleet`,
      { data: body }
    );
    return response.data;
  },

  // ── Audit log (Tier B #7) ────────────────────────────────────────────────

  async listAuditEvents(workspaceId: string, filter: AuditFilter = {}): Promise<AuditEventRow[]> {
    const response = await apiClient.get<AuditEventRow[]>(`/${workspaceId}/cameras/audit-events`, {
      params: cleanAuditParams(filter)
    });
    return response.data;
  },

  // ── Clip playback (Tier B #6 follow-up) ──────────────────────────────────

  async getClipPlaybackUrl(
    workspaceId: string,
    reportId: string,
    key: string
  ): Promise<{ presigned_get_url: string; expires_in_secs: number }> {
    const response = await apiClient.get<{
      presigned_get_url: string;
      expires_in_secs: number;
    }>(`/${workspaceId}/cameras/clips/${reportId}/url`, {
      params: { key }
    });
    return response.data;
  },

  // ── Camera health alerting ───────────────────────────────────────────────

  async getCameraHealthSummary(workspaceId: string): Promise<CameraHealthRow[]> {
    const response = await apiClient.get<CameraHealthRow[]>(
      `/${workspaceId}/cameras/health-summary`
    );
    return response.data;
  },

  // ── Operator zone approval automation ──────────────────────────────────────

  async proposeCameraGeometry(
    workspaceId: string,
    cameraId: string,
    body: { zones_json?: unknown; lines_json?: unknown }
  ): Promise<Camera> {
    const response = await apiClient.put<Camera>(
      `/${workspaceId}/cameras/${cameraId}/proposed-geometry`,
      body
    );
    return response.data;
  },

  async approveCameraProposal(workspaceId: string, cameraId: string): Promise<Camera> {
    const response = await apiClient.post<Camera>(
      `/${workspaceId}/cameras/${cameraId}/proposed-geometry/approve`
    );
    return response.data;
  },

  async rejectCameraProposal(workspaceId: string, cameraId: string): Promise<Camera> {
    const response = await apiClient.post<Camera>(
      `/${workspaceId}/cameras/${cameraId}/proposed-geometry/reject`
    );
    return response.data;
  }
};

// ── Camera health (mirror crates/cameras/src/service/camera_health.rs) ──────

type CameraHealthStatus = "ok" | "degraded" | "stale" | "unknown";

export type CameraHealthRow = {
  camera_id: string;
  status: CameraHealthStatus | string;
  fps: number | null;
  decoder_errors: number | null;
  reconnect_count: number | null;
  last_frame_at: string | null;
  last_reported_at: string | null;
  /** Human-readable reason the status isn't `ok`. Empty otherwise. */
  reason: string;
};

/** Range param accepted by the cost endpoints. */
export type CostRange = "24h" | "7d" | "30d";

type CostByModel = {
  model: string;
  reports: number;
  tokens_used: number;
  /** Null for unknown/unpriced models — UI renders these as n/a. */
  cost_usd_micros: number | null;
};

export type EdgeBoxCost = {
  edge_box_id: string;
  range: string;
  reports: number;
  tokens_used: number;
  cost_usd_micros: number;
  by_model: CostByModel[];
};

export type WorkspaceCost = {
  range: string;
  reports: number;
  tokens_used: number;
  cost_usd_micros: number;
  /** Per-box breakdown, sorted heaviest-first. */
  by_box: EdgeBoxCost[];
};

/** Format USD micros as a human-friendly string. Sub-cent values
 *  render as "$0.00" (the operator-friendly read for "negligible"),
 *  mirroring the backend's `pricing::format_micros_as_usd`. */
export function formatCostUsd(micros: number | null | undefined): string {
  if (micros == null) return "—";
  return `$${(micros / 1_000_000).toFixed(2)}`;
}

// ── Device logs (mirror crates/cameras/src/service/logs.rs) ─────────────────

export type LogSeverity = "debug" | "info" | "warn" | "warning" | "error";

/** One row as returned by `GET /{wid}/cameras/edge-boxes/{id}/logs`. */
export type LogRow = {
  box_id: string;
  ts: string;
  severity: string;
  event: string;
  /** Pre-encoded JSON object — UI parses lazily for display. */
  fields_json: string;
  received_at: string;
};

export type LogFilter = {
  min_severity?: LogSeverity;
  /** Substring match on the canonical snake_case event name. */
  event_contains?: string;
  /** RFC-3339 cutoff; only logs with `received_at >= since` returned. */
  since?: string;
  /** Default 200, max 1000 server-side. */
  limit?: number;
};

function cleanLogParams(f: LogFilter): Record<string, string | number> {
  const out: Record<string, string | number> = {};
  if (f.min_severity) out.min_severity = f.min_severity;
  if (f.event_contains) out.event_contains = f.event_contains;
  if (f.since) out.since = f.since;
  if (f.limit) out.limit = f.limit;
  return out;
}

// ── Audit log (mirror crates/cameras/src/service/audit.rs) ──────────────────

export type AuditEventRow = {
  id: string;
  actor_user_id: string | null;
  /** Canonical snake_case action key, e.g. `edge_box.set_target`. */
  action: string;
  target_id: string | null;
  /** Free-shape JSON; UI renders as key/value pairs. */
  details_json: Record<string, unknown>;
  created_at: string;
};

export type AuditFilter = {
  /** Trailing-wildcard match on the action column. */
  action_prefix?: string;
  /** Only events whose `target_id` matches this UUID. */
  target_id?: string;
  /** Default 50, max 500 server-side. */
  limit?: number;
  offset?: number;
};

function cleanAuditParams(f: AuditFilter): Record<string, string | number> {
  const out: Record<string, string | number> = {};
  if (f.action_prefix) out.action_prefix = f.action_prefix;
  if (f.target_id) out.target_id = f.target_id;
  if (f.limit) out.limit = f.limit;
  if (f.offset) out.offset = f.offset;
  return out;
}

// ── VLM budget (mirror crates/cameras/src/service/budget.rs) ────────────────

// ── Fleet (mirror crates/cameras/src/service/fleet.rs) ──────────────────────

type FleetClaimStatus = "pending_claim" | "claimed" | "revoked";

/** Returned from `DELETE /{wid}/cameras/fleet`. The UI uses
 *  these flags to render a toast summarizing what changed. */
export type RemoveFleetMemberResult = {
  edge_box_retired: boolean;
  claim_revoked: boolean;
  target_id: string;
};

/** One row in the merged "physical boxes" view. Either
 *  edge_box is non-null (operational box) or the identity
 *  columns are populated (pending claim) — for a fully
 *  claimed-and-bootstrapped box, both are. */
export type FleetMemberRow = {
  edge_box: EdgeBox | null;
  site_id: string;
  site_name: string;
  device_id: string | null;
  /** `legacy` (no identity layer) | `pending_claim` | `claimed`. */
  claim_status: "legacy" | "pending_claim" | "claimed" | string;
  claim_code: string | null;
  sku: string | null;
  hardware_revision: string | null;
  last_announce_at: string | null;
};

/** Returned from `POST /{wid}/fleet/prepared-devices`. The
 *  `device_secret_b64` is shown ONCE — the server never returns
 *  it again. Operators must copy the install snippet into the
 *  new box's terminal before closing the dialog. */
export type PreparedDeviceResponse = {
  device_id: string;
  device_secret_b64: string;
  claim_code: string;
  claim_id: string;
  sku: string;
  hardware_revision: string;
  /** One-line `curl | sudo bash` — preferred. Sets up docker,
   *  identity, compose, and the systemd unit in one command. */
  install_command: string;
  /** Verbose JSON-into-device.json snippet — fallback for boxes
   *  that can't reach the install script (airgapped, etc.). */
  install_snippet: string;
};

export type FleetDeviceRow = {
  device_id: string;
  claim_code: string;
  sku: string;
  hardware_revision: string;
  status: FleetClaimStatus | string;
  edge_box_id: string | null;
  site_id: string | null;
  site_name: string | null;
  last_announce_at: string | null;
  claimed_at: string | null;
};

export type BudgetStatus = {
  /** Null when the operator hasn't set a ceiling — banner hides. */
  budget_micros: number | null;
  month_to_date_micros: number;
  /** Straight-line month-end projection. */
  projected_month_micros: number;
  day_of_month: number;
  days_in_month: number;
  /** `month_to_date / budget` in [0, ∞); null when budget is null. */
  percent_consumed: number | null;
  /** `projected / budget` in [0, ∞); null when budget is null. */
  percent_projected: number | null;
};

// ── Domain pack types (mirror crates/cameras/src/routes/dto.rs) ─────────────

/**
 * One role within a domain pack. `prompt` is a Jinja2 template
 * rendered with `track_id`, `dwell_seconds`, and `variables`.
 * Backend renders against a dummy context at PUT time so a typo
 * here returns 400 before it ever ships to a worker.
 */
type DomainPackRole = {
  role_key: string;
  label: string;
  hint: string;
  prompt: string;
};

/**
 * Domain pack body. Lives inside `DomainPack.body` and is what
 * the operator edits through the pack editor. Forward-compatible:
 * unknown keys round-trip through the API as-is.
 */
export type DomainPackBody = {
  schema_version: number;
  output_schema: unknown;
  roles: DomainPackRole[];
  variables: Record<string, unknown>;
};

/**
 * Operator view of a domain pack. Mirrors `DomainPackDto` server-
 * side. `body` is `unknown` at the wire level so callers narrow
 * to `DomainPackBody` only when they need the structured fields.
 */
export type DomainPack = {
  id: string;
  pack_key: string;
  name: string;
  description: string | null;
  starter_key: string;
  starter_version: string;
  locally_modified: boolean;
  is_active: boolean;
  body: DomainPackBody;
  created_at: string;
  updated_at: string;
};

/**
 * Compact pack row for the list endpoint — omits `body`. Open the
 * pack via `getActivePack` (or future per-key endpoint) to fetch
 * the body when an editor needs it.
 */
export type DomainPackSummary = Omit<DomainPack, "body">;

/** One entry in Oxy's curated starter library. */
export type StarterPackSummary = {
  key: string;
  name: string;
  description: string;
  version: string;
};

/**
 * Per-role diff status returned by `GET /camera-packs/starter-updates`.
 * Mirrors `RoleDiffStatus` server-side.
 */
export type RoleDiffStatus = "unchanged" | "differs" | "only_in_upstream" | "only_in_current";

export type RoleDiff = {
  role_key: string;
  status: RoleDiffStatus;
  current: DomainPackRole | null;
  upstream: DomainPackRole | null;
};

/**
 * One pending update against a workspace pack. Empty array
 * response from the endpoint means all packs are at the latest
 * starter version.
 */
export type PackStarterUpdate = {
  pack_key: string;
  starter_key: string;
  current_version: string;
  available_version: string;
  locally_modified: boolean;
  role_diffs: RoleDiff[];
  variables_differ: boolean;
  current_variables: Record<string, unknown>;
  upstream_variables: Record<string, unknown>;
};

/**
 * Per-key decision sent with `POST /camera-packs/{key}/apply-update`.
 * The special key `"$variables"` controls the variables map.
 */
export type UpdateAction = "take_upstream" | "keep_mine";

/** Sentinel key matching `crate::service::packs::updates::VARIABLES_ACTION_KEY`. */
export const VARIABLES_ACTION_KEY = "$variables";
