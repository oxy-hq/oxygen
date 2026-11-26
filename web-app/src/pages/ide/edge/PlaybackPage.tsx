import { AlertTriangle, Camera as CameraIcon, Film, Info, Radio } from "lucide-react";
import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger
} from "@/components/ui/shadcn/tooltip";
import useCameras from "@/hooks/api/cameras/useCameras";
import useComplianceReports from "@/hooks/api/cameras/useComplianceReports";
import useRecordingSegments from "@/hooks/api/cameras/useRecordingSegments";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { useSelectedSite } from "@/hooks/useSelectedSite";
import { cn } from "@/libs/shadcn/utils";
import type {
  ComplianceReport,
  PpeDetectionsEnvelope,
  RecordingSegment
} from "@/services/api/cameras";
import CameraSidebar from "./timeline/CameraSidebar";
import ClipPlayer from "./timeline/ClipPlayer";
import {
  eventSeverity,
  formatReceivedAt,
  isComplianceFailure,
  RANGE_OPTIONS,
  rangeToSince,
  reportSnippet,
  type Severity,
  segmentDurationSeconds,
  severityDotClass,
  toStructuredVerdict
} from "./timeline/format";
import LiveView from "./timeline/LiveView";

type PlayerMode = "live" | "playback";

/** Default clip length, in seconds, when the operator scrubs to an
 *  arbitrary point on the timeline (not anchored to a compliance
 *  report). MediaMTX's `/get` accepts up to 600s; 30s is a sweet spot
 *  for "what just happened here?" without burning bandwidth on tail
 *  footage the operator almost never watches. */
const SCRUB_CLIP_DURATION_S = 30;

/** Snap a free-scrub click onto a real recorded segment. Behavior:
 *   - If the click lands inside a segment, play from there (capped at
 *     `SCRUB_CLIP_DURATION_S`).
 *   - If the click lands in a gap, jump to the closest segment edge
 *     (the start of the next segment if the click was before it, or
 *     the last `SCRUB_CLIP_DURATION_S` of the previous segment if the
 *     click was after it).
 *   - Returns `null` when there are no segments at all — caller
 *     decides what to render.
 */
function snapToSegment(
  wallClockMs: number,
  segments: RecordingSegment[]
): { startIso: string; durationSeconds: number } | null {
  if (segments.length === 0) return null;
  // Defensive: copy + sort so we tolerate unsorted server payloads.
  const sorted = [...segments].sort(
    (a, b) => new Date(a.start).getTime() - new Date(b.start).getTime()
  );

  let best: { startIso: string; durationSeconds: number } | null = null;
  let bestDelta = Number.POSITIVE_INFINITY;
  for (const seg of sorted) {
    const segStart = new Date(seg.start).getTime();
    const segEnd = segStart + seg.duration_seconds * 1000;
    if (wallClockMs >= segStart && wallClockMs <= segEnd) {
      // Inside this segment — play from the click, but don't overrun.
      const offsetMs = wallClockMs - segStart;
      const remainingS = Math.max(1, seg.duration_seconds - offsetMs / 1000);
      return {
        startIso: new Date(wallClockMs).toISOString(),
        durationSeconds: Math.min(SCRUB_CLIP_DURATION_S, remainingS)
      };
    }
    // Click before this segment → candidate is the segment's start.
    if (wallClockMs < segStart) {
      const delta = segStart - wallClockMs;
      if (delta < bestDelta) {
        bestDelta = delta;
        best = {
          startIso: seg.start,
          durationSeconds: Math.min(SCRUB_CLIP_DURATION_S, seg.duration_seconds)
        };
      }
    }
    // Click after this segment → candidate is the tail of this segment.
    else {
      const delta = wallClockMs - segEnd;
      if (delta < bestDelta) {
        bestDelta = delta;
        const tailStartMs = Math.max(segStart, segEnd - SCRUB_CLIP_DURATION_S * 1000);
        best = {
          startIso: new Date(tailStartMs).toISOString(),
          durationSeconds: Math.min(SCRUB_CLIP_DURATION_S, (segEnd - tailStartMs) / 1000)
        };
      }
    }
  }
  return best;
}

type PlaybackTarget = {
  /** ISO-8601 wall-clock start time. */
  start: string;
  /** Clip length in seconds. */
  durationSeconds: number;
  /** Set when the playback was triggered by clicking a compliance
   *  report marker; lets the player chrome show a severity badge. */
  reportId?: string;
};

/**
 * Playback — live + recorded video viewer.
 *
 * Layout: left rail = camera sidebar (grouped by site), right pane =
 * focused-camera player + scrubber. Replaces the older Grid/Single
 * mode toggle — operators kept ping-ponging between Grid (to pick a
 * camera) and Single (to actually watch), so the picker lives next
 * to the player now.
 *
 * Two playback modes:
 *  - **Live** — HLS proxy via `LiveView`. Default. "Live" button is
 *    a hard reset of the playback target so re-clicking the same
 *    detection marker after going Live actually re-loads the clip.
 *  - **Playback** — `ClipPlayer` against a `{start, duration}` pair.
 *    Set by either (a) clicking a detection marker on the scrubber
 *    (start = `segment_start`, duration = segment length) or (b)
 *    clicking anywhere on the bare timeline (start = wall-clock of
 *    click, duration = `SCRUB_CLIP_DURATION_S`). The latter relies
 *    on MediaMTX's playback `/get` accepting arbitrary timestamps,
 *    so the operator can scrub footage between detections.
 *
 * URL params:
 *  - `?camera=<id>` — initial focused camera (cross-link from
 *    Detections / Topology). Doesn't filter the sidebar — operators
 *    need to be able to navigate to other cameras from a deep link.
 *  - `?box=<id>` — sidebar narrowed to one edge box.
 *  - `?event=<id>` — auto-play this compliance report's clip on
 *    arrival, then clear the param.
 *
 * The global site picker scopes the sidebar; URL filters compose
 * with it (URL wins for the focused camera, site picker still
 * narrows the picker list).
 */
const PlaybackPage: React.FC = () => {
  const { project } = useCurrentProjectBranch();
  const workspaceId = project.id;
  const { data: cameras = [], isLoading: camerasLoading } = useCameras(workspaceId);
  const { siteId: selectedSite } = useSelectedSite();

  const [searchParams, setSearchParams] = useSearchParams();
  const initialCamera = searchParams.get("camera") ?? undefined;
  const scopeBox = searchParams.get("box") ?? undefined;
  const focusEventId = searchParams.get("event") ?? undefined;

  // Cameras the sidebar shows. URL `?box=` and the global site
  // picker compose; an `?camera=` deep link does NOT collapse the
  // sidebar to that one camera (operators must be able to navigate
  // away from a deep-linked starting point).
  const sidebarCameras = useMemo(() => {
    let list = cameras;
    if (selectedSite) list = list.filter((c) => c.site_id === selectedSite);
    if (scopeBox) list = list.filter((c) => c.edge_box_id === scopeBox);
    return list;
  }, [cameras, selectedSite, scopeBox]);

  // Focused camera = the one in the right-pane player. Initialized
  // from `?camera=` when present, else the first sidebar entry.
  const [singleCameraId, setSingleCameraId] = useState<string | undefined>(initialCamera);
  useEffect(() => {
    if (initialCamera && initialCamera !== singleCameraId) {
      setSingleCameraId(initialCamera);
      return;
    }
    if (!singleCameraId && sidebarCameras[0]) {
      setSingleCameraId(sidebarCameras[0].id);
    }
  }, [initialCamera, sidebarCameras, singleCameraId]);

  const focusCamera = (cameraId: string) => {
    setSingleCameraId(cameraId);
    // Persist the focus in the URL so refresh / share-link round-trips.
    const sp = new URLSearchParams(searchParams);
    sp.set("camera", cameraId);
    setSearchParams(sp, { replace: true });
  };

  const [range, setRange] = useState("24h");
  const since = useMemo(() => rangeToSince(range), [range]);

  const { data: reports = [] } = useComplianceReports({
    workspaceId,
    cameraId: singleCameraId,
    since,
    limit: 200
  });
  // Available footage windows on disk for the focused camera. Drives
  // the scrubber's shaded bands + the snap-to-segment behavior when
  // the operator scrubs onto a gap. Hook tolerates undefined cameraId.
  const { data: recordingSegments = [] } = useRecordingSegments(workspaceId, singleCameraId);

  const taggedReports = useMemo(
    () =>
      reports.map((r) => ({
        report: r,
        severity: eventSeverity(r.structured_json, r.received_at)
      })),
    [reports]
  );

  const selectedCamera = useMemo(
    () => cameras.find((c) => c.id === singleCameraId),
    [cameras, singleCameraId]
  );

  // ── Playback target + player mode ──────────────────────────────
  const [playerMode, setPlayerMode] = useState<PlayerMode>("live");
  const [playback, setPlayback] = useState<PlaybackTarget | null>(null);

  // Re-focusing a camera or changing the range invalidates any
  // playback target — clip starts that came from the previous
  // camera's reports are meaningless for the new one. Drop back
  // to Live, which is the resting state.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset-on-change pattern
  useEffect(() => {
    setPlayback(null);
    setPlayerMode("live");
  }, [singleCameraId, range]);

  const selectReport = (reportId: string) => {
    const tagged = taggedReports.find((t) => t.report.report_id === reportId);
    if (!tagged) return;
    setPlayback({
      reportId,
      start: tagged.report.segment_start,
      durationSeconds: segmentDurationSeconds(
        tagged.report.segment_start,
        tagged.report.segment_end
      )
    });
    setPlayerMode("playback");
  };

  // Free-scrub: clicking anywhere on the timeline plays a clip
  // starting at that wall-clock instant. If the click landed in a
  // gap (no MTX recording at that instant), snap to the nearest
  // available segment so the operator gets footage instead of a 503.
  // Drops `reportId` since the playback is no longer tied to a
  // verdict.
  const seekToWallClock = (wallClockMs: number) => {
    const snapped = snapToSegment(wallClockMs, recordingSegments);
    if (!snapped) {
      // No recordings at all for this camera — nothing to play, but
      // still surface a playback target so the ClipPlayer's friendly
      // empty-state can render its "Back to Live" CTA.
      setPlayback({
        start: new Date(wallClockMs).toISOString(),
        durationSeconds: SCRUB_CLIP_DURATION_S,
        reportId: undefined
      });
      setPlayerMode("playback");
      return;
    }
    setPlayback({
      start: snapped.startIso,
      durationSeconds: snapped.durationSeconds,
      reportId: undefined
    });
    setPlayerMode("playback");
  };

  // Live button must hard-reset the playback target so that
  // re-clicking the SAME detection after going Live actually
  // re-fires the effect that loads the clip (a stale target +
  // mode flip alone left the player on a black frame).
  const goLive = () => {
    setPlayback(null);
    setPlayerMode("live");
  };

  // Cross-link from Detections — find the report and auto-play.
  // We watch `singleCameraId` so a `?camera=X&event=Y` link works
  // even when the first effect tick races ahead of the reports
  // query for camera Y.
  // biome-ignore lint/correctness/useExhaustiveDependencies: cross-link consume-once pattern (selectReport closure reads taggedReports)
  useEffect(() => {
    if (!focusEventId) return;
    const tagged = taggedReports.find((t) => t.report.report_id === focusEventId);
    if (!tagged) return;
    selectReport(focusEventId);
    const sp = new URLSearchParams(searchParams);
    sp.delete("event");
    setSearchParams(sp, { replace: true });
  }, [focusEventId, taggedReports, searchParams, setSearchParams]);

  const selectedTagged = playback?.reportId
    ? taggedReports.find((t) => t.report.report_id === playback.reportId)
    : undefined;
  // P3 (Option C) — PPE-YOLO bbox overlay. Only rendered when the
  // playback is anchored to a known compliance report (free-scrub
  // clips have no detections payload) AND `?debug=1` is set. The
  // debug-mode toggle lets us ship the plumbing dark and only
  // surface to reviewers / pilots until VLM agreement is measured.
  const debugMode = searchParams.get("debug") === "1";
  const detectionsEnvelope =
    debugMode && selectedTagged?.report.detections_json
      ? selectedTagged.report.detections_json
      : undefined;

  // ── Playhead state — wall-clock ms, set by ClipPlayer onProgress ─
  const [playheadMs, setPlayheadMs] = useState<number | undefined>();
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset-on-target-change pattern
  useEffect(() => {
    setPlayheadMs(undefined);
  }, [playback?.start, playback?.durationSeconds]);
  const handleClipProgress = (seconds: number) => {
    if (!playback) return;
    setPlayheadMs(new Date(playback.start).getTime() + seconds * 1000);
  };

  return (
    <div className='flex h-full min-h-0 min-w-0 flex-col gap-3 p-4' data-testid='edge-playback'>
      <div className='flex flex-wrap items-center gap-3'>
        <Label htmlFor='playback-range' className='text-muted-foreground'>
          Range
        </Label>
        <Select value={range} onValueChange={setRange}>
          <SelectTrigger id='playback-range' className='w-44'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            {RANGE_OPTIONS.map((o) => (
              <SelectItem key={o.value} value={o.value}>
                {o.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {/* Sidebar (left) + player (right). min-h-0 on both so the
          flex children don't blow out of the page height — without
          it the long sidebar would push the player off the bottom
          on small viewports. */}
      <div className='flex min-h-0 min-w-0 flex-1 gap-3'>
        <aside className='w-72 shrink-0 overflow-hidden border border-border bg-card'>
          <CameraSidebar
            workspaceId={workspaceId}
            cameras={sidebarCameras}
            since={since}
            activeCameraId={singleCameraId}
            onSelect={focusCamera}
          />
        </aside>

        <section className='flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden border bg-card'>
          {camerasLoading ? (
            <PlayerEmpty>Loading cameras…</PlayerEmpty>
          ) : sidebarCameras.length === 0 ? (
            <PlayerEmpty>No cameras in this scope. Adjust the site picker.</PlayerEmpty>
          ) : (
            <SingleCameraPlayer
              workspaceId={workspaceId}
              cameraId={singleCameraId}
              cameraName={selectedCamera?.name}
              since={since}
              playerMode={playerMode}
              playback={playback}
              selectedSeverity={selectedTagged?.severity}
              selectedReport={selectedTagged?.report}
              taggedReports={taggedReports}
              recordingSegments={recordingSegments}
              detectionsEnvelope={detectionsEnvelope}
              onGoLive={goLive}
              onSelectReport={selectReport}
              onSeekWallClock={seekToWallClock}
              onClipProgress={handleClipProgress}
              playheadMs={playheadMs}
            />
          )}
        </section>
      </div>
    </div>
  );
};

const PlayerEmpty: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div className='flex h-full items-center justify-center p-8 text-center text-muted-foreground text-sm'>
    {children}
  </div>
);

// ── Single-camera player + scrubber ─────────────────────────────────────────

type SingleCameraPlayerProps = {
  workspaceId: string;
  cameraId: string | undefined;
  cameraName: string | undefined;
  since: string | undefined;
  playerMode: PlayerMode;
  playback: PlaybackTarget | null;
  selectedSeverity: Severity | undefined;
  /** Currently-selected compliance report. Drives the violation
   *  popover in the player header so the operator can read the
   *  VLM verdict (missing items, confidence, notes) inline without
   *  bouncing to the Detections page. */
  selectedReport: ComplianceReport | undefined;
  taggedReports: {
    report: ComplianceReport;
    severity: Severity;
  }[];
  recordingSegments: RecordingSegment[];
  /** P3 (Option C) — PPE-YOLO bbox envelope to overlay on the
   *  playing clip. Already gated on `?debug=1` in the parent. */
  detectionsEnvelope?: PpeDetectionsEnvelope;
  /** Hard-reset back to live: clears the playback target AND flips
   *  mode. Used by the floating LIVE pill on the player and the
   *  ClipPlayer's "no footage here" error state. */
  onGoLive: () => void;
  onSelectReport: (id: string) => void;
  onSeekWallClock: (wallClockMs: number) => void;
  onClipProgress: (seconds: number) => void;
  playheadMs: number | undefined;
};

const SingleCameraPlayer: React.FC<SingleCameraPlayerProps> = ({
  workspaceId,
  cameraId,
  cameraName,
  since,
  playerMode,
  playback,
  selectedSeverity,
  selectedReport,
  taggedReports,
  recordingSegments,
  detectionsEnvelope,
  onGoLive,
  onSelectReport,
  onSeekWallClock,
  onClipProgress,
  playheadMs
}) => {
  return (
    <div className='flex h-full min-h-0 flex-row gap-3 p-3'>
      {/* Player card occupies the bulk of the right pane. Header is
          `shrink-0` so the video area's `flex-1 min-h-0` can't squeeze
          it out (the old layout's `aspect-video` on ClipPlayer could
          push the header off the top of an overflow-hidden parent). */}
      <div className='flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden border border-border bg-card'>
        <div className='flex shrink-0 items-center justify-between gap-2 border-border border-b px-3 py-2'>
          <div className='flex min-w-0 items-center gap-2'>
            <CameraIcon className='h-3.5 w-3.5 shrink-0 text-muted-foreground' />
            <span className='truncate font-medium text-sm'>{cameraName ?? "—"}</span>
            {playerMode === "playback" && playback && (
              <span className='shrink-0 text-muted-foreground text-xs'>
                · {formatReceivedAt(playback.start)}
              </span>
            )}
          </div>
          <div className='flex items-center gap-2'>
            {detectionsEnvelope && (
              <Badge variant='outline' className='gap-1 font-mono text-[10px]'>
                YOLO debug
              </Badge>
            )}
            {/* Always rendered so the operator has a single,
                predictable place to look for the current event's
                verdict. With no report selected (Live, or
                scrubbing between detections) it stays muted; clicking
                a detection marker / event in the rail populates it. */}
            <ViolationIndicator report={selectedReport} severity={selectedSeverity} />

            {/* State indicator only — no toggle. Going Live happens
                via the floating pill in the player corner; going to
                Playback happens implicitly by clicking the scrubber
                or a detection marker. The old dual-button toggle was
                redundant (Playback only re-entered a state the
                scrubber already provided). */}
            <span
              className={cn(
                "inline-flex items-center gap-1 border px-2 py-0.5 text-xs",
                playerMode === "live"
                  ? "border-destructive/40 bg-destructive/5 text-destructive"
                  : "border-border bg-muted/30 text-muted-foreground"
              )}
            >
              {playerMode === "live" ? (
                <>
                  <Radio className='h-3 w-3' /> Live
                </>
              ) : (
                <>
                  <Film className='h-3 w-3' /> Playback
                </>
              )}
            </span>
          </div>
        </div>
        <div className='relative flex min-h-0 flex-1 items-center justify-center bg-black'>
          {playerMode === "live" && cameraId ? (
            <LiveView cameraId={cameraId} cameraName={cameraName} />
          ) : playerMode === "playback" && playback && cameraId ? (
            <ClipPlayer
              workspaceId={workspaceId}
              cameraId={cameraId}
              start={playback.start}
              durationSeconds={playback.durationSeconds}
              onProgress={onClipProgress}
              onGoLive={onGoLive}
              detections={detectionsEnvelope}
            />
          ) : (
            <div className='flex flex-col items-center gap-2 p-8 text-center text-muted-foreground text-sm'>
              <Film className='h-8 w-8' />
              Click a detection marker below or anywhere on the timeline to play recorded footage.
            </div>
          )}
          {/* Floating Go-Live affordance — always visible during
              playback so the operator never has to hunt for the way
              back to the live feed (the header toggle is too easy to
              overlook with eyes on the player). */}
          {playerMode === "playback" && (
            <button
              type='button'
              onClick={onGoLive}
              className='absolute top-3 right-3 z-10 inline-flex items-center gap-1.5 rounded-full bg-destructive/90 px-3 py-1 font-semibold text-[11px] text-destructive-foreground shadow-md backdrop-blur transition-colors hover:bg-destructive'
              data-testid='player-go-live'
              aria-label='Go to live feed'
            >
              <span className='relative inline-flex h-2 w-2'>
                <span className='absolute inset-0 animate-ping rounded-full bg-destructive-foreground/60' />
                <span className='relative h-2 w-2 rounded-full bg-destructive-foreground' />
              </span>
              LIVE
            </button>
          )}
        </div>
      </div>

      {/* Vertical timeline right rail. Replaces the old horizontal
          scrubber under the player — operators were finding it hard
          to scan tall detection bursts horizontally; vertical gives
          time tickers more breathing room and matches the "scroll
          back in time" mental model of activity logs. Newest at the
          top (NOW), older below. */}
      <VerticalDetectionScrubber
        since={since}
        events={taggedReports.map((t) => ({
          id: t.report.report_id,
          ts: t.report.received_at,
          severity: t.severity
        }))}
        recordingSegments={recordingSegments}
        activeId={playback?.reportId}
        onSelectMarker={onSelectReport}
        onSeek={onSeekWallClock}
        playheadMs={playheadMs}
      />
    </div>
  );
};

// ── Vertical detection timeline ────────────────────────────────────────────
//
// Right-rail timeline replacement for the old horizontal strip. Newest
// at top (NOW); time flows downward into the past. Layout:
//
//   ┌────────────────────────────┐
//   │ ● NOW                      │  ← top-pinned label
//   ├────┬───────────────────────┤
//   │ 11:│30                     │  ← time-ticker column
//   │    │█ event                │  ← event markers + recording bands
//   │ 11:│00 ━━ playhead         │
//   │    │█ event                │
//   │ 10:│30                     │
//   │    ▼                       │
//
// Click-to-seek hits a target wall-clock; markers stop-propagation
// to fire the marker-select handler instead.
const TICKER_WIDTH_PX = 44;
const RAIL_WIDTH_CLASS = "w-32";

const VerticalDetectionScrubber: React.FC<{
  since: string | undefined;
  events: { id: string; ts: string; severity: Severity }[];
  recordingSegments: RecordingSegment[];
  activeId: string | undefined;
  onSelectMarker: (id: string) => void;
  onSeek: (wallClockMs: number) => void;
  playheadMs?: number;
}> = ({ since, events, recordingSegments, activeId, onSelectMarker, onSeek, playheadMs }) => {
  const lowerMs = since ? new Date(since).getTime() : Date.now() - 24 * 60 * 60 * 1000;
  const upperMs = Date.now();
  const span = Math.max(upperMs - lowerMs, 1);

  // pctFromTop: 0 at NOW (top), 100 at the lower bound (bottom).
  const pctFromTop = (ms: number) => ((upperMs - ms) / span) * 100;

  type Band = { key: string; topPct: number; heightPct: number };
  const bands = useMemo<Band[]>(
    () =>
      recordingSegments
        .map((s): Band | null => {
          const start = new Date(s.start).getTime();
          const end = start + s.duration_seconds * 1000;
          const clippedStart = Math.max(start, lowerMs);
          const clippedEnd = Math.min(end, upperMs);
          if (clippedEnd <= clippedStart) return null;
          return {
            key: s.start,
            topPct: pctFromTop(clippedEnd),
            heightPct: ((clippedEnd - clippedStart) / span) * 100
          };
        })
        .filter((b): b is Band => b !== null),
    // `pctFromTop` is stable per render (closes over lowerMs/upperMs/span)
    // and excluded from deps on purpose.
    // biome-ignore lint/correctness/useExhaustiveDependencies: see comment above
    [recordingSegments, lowerMs, upperMs, span, pctFromTop]
  );

  // Tick labels — choose granularity from the visible span. Goal:
  // ≤ ~12 labels so they breathe; never under 1min apart.
  type Tick = { key: string; topPct: number; label: string };
  const ticks = useMemo<Tick[]>(() => {
    const hours = span / 3_600_000;
    const stepMs =
      hours > 36
        ? 4 * 3_600_000
        : hours > 12
          ? 2 * 3_600_000
          : hours > 4
            ? 60 * 60_000
            : hours > 1
              ? 30 * 60_000
              : 10 * 60_000;
    const fmt = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
    const out: Tick[] = [];
    // Snap the first tick down to a clean step boundary so the
    // labels read "11:00, 10:30, 10:00…" not "11:07, 10:37, 10:07…".
    const firstMs = Math.floor(upperMs / stepMs) * stepMs;
    for (let t = firstMs; t >= lowerMs; t -= stepMs) {
      out.push({
        key: String(t),
        topPct: pctFromTop(t),
        label: fmt.format(new Date(t))
      });
    }
    return out;
    // biome-ignore lint/correctness/useExhaustiveDependencies: pctFromTop is render-stable
  }, [lowerMs, upperMs, span, pctFromTop]);

  const handleRailClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const pct = (e.clientY - rect.top) / Math.max(rect.height, 1);
    // Top of rail = NOW, bottom = lowerMs.
    const targetMs = upperMs - pct * span;
    onSeek(Math.min(Math.max(targetMs, lowerMs), upperMs - 1000));
  };
  const handleRailKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (events.length === 0) return;
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown" && e.key !== "Home" && e.key !== "End") return;
    e.preventDefault();
    // Sort newest-first to match the visual top-to-bottom order.
    const sorted = [...events].sort((a, b) => new Date(b.ts).getTime() - new Date(a.ts).getTime());
    const currentIdx = activeId ? sorted.findIndex((ev) => ev.id === activeId) : -1;
    let nextIdx = currentIdx;
    if (e.key === "ArrowUp") nextIdx = Math.max(0, currentIdx <= 0 ? 0 : currentIdx - 1);
    else if (e.key === "ArrowDown")
      nextIdx = currentIdx < 0 ? 0 : Math.min(sorted.length - 1, currentIdx + 1);
    else if (e.key === "Home") nextIdx = 0;
    else if (e.key === "End") nextIdx = sorted.length - 1;
    onSelectMarker(sorted[nextIdx].id);
  };

  const playheadPctTop =
    playheadMs !== undefined ? Math.min(100, Math.max(0, pctFromTop(playheadMs))) : undefined;

  return (
    <aside
      className={cn(
        "relative flex shrink-0 flex-col overflow-hidden border border-border bg-card",
        RAIL_WIDTH_CLASS
      )}
      data-testid='vertical-detection-scrubber'
    >
      {/* NOW pill pinned at the top — mirrors the position of the
          first ticker so the operator's eyes always know which end
          is "live." */}
      <div className='flex shrink-0 items-center justify-center gap-1.5 border-border border-b px-2 py-1.5 font-semibold text-[10px] text-destructive tabular-nums'>
        <span className='relative inline-flex h-1.5 w-1.5'>
          <span className='absolute inset-0 animate-ping rounded-full bg-destructive/70' />
          <span className='relative h-1.5 w-1.5 rounded-full bg-destructive' />
        </span>
        NOW
      </div>

      <div
        className='relative min-h-0 flex-1 cursor-pointer'
        onClick={handleRailClick}
        onKeyDown={handleRailKeyDown}
        role='slider'
        tabIndex={0}
        aria-label='Detection timeline'
        aria-orientation='vertical'
        aria-valuemin={lowerMs}
        aria-valuemax={upperMs}
        aria-valuenow={playheadMs ?? upperMs}
      >
        {/* Time tickers on the left column. Lines reach across so the
            operator can read across to the markers at the same depth. */}
        {ticks.map((t) => (
          <div
            key={t.key}
            className='pointer-events-none absolute right-0 left-0 flex items-center gap-1.5'
            style={{ top: `${t.topPct}%` }}
            aria-hidden
          >
            <span
              className='inline-block shrink-0 pl-2 text-[10px] text-muted-foreground tabular-nums'
              style={{ width: `${TICKER_WIDTH_PX}px` }}
            >
              {t.label}
            </span>
            <span className='block h-px flex-1 bg-border/60' />
          </div>
        ))}

        {/* Track column on the right (where markers + bands live). */}
        <div
          className='absolute top-0 bottom-0'
          style={{ left: `${TICKER_WIDTH_PX + 12}px`, right: "12px" }}
        >
          <div className='relative h-full w-px bg-border'>
            {/* Recording bands — vertical equivalent of the original
                horizontal shaded bands. */}
            {bands.map((b) => (
              <div
                key={b.key}
                className='pointer-events-none absolute left-1/2 w-3 -translate-x-1/2 bg-primary/15'
                style={{ top: `${b.topPct}%`, height: `${b.heightPct}%` }}
                aria-hidden
              />
            ))}
            {/* Detection markers — horizontal dashes at their time
                position. Severity drives the dash width (more severe
                = wider so it grabs the eye). */}
            {events.map((e) => {
              const t = new Date(e.ts).getTime();
              const pct = pctFromTop(t);
              if (pct < 0 || pct > 100) return null;
              const isActive = e.id === activeId;
              const widthClass =
                e.severity === "critical" ? "w-5" : e.severity === "warning" ? "w-4" : "w-3";
              return (
                <button
                  type='button'
                  key={e.id}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    onSelectMarker(e.id);
                  }}
                  style={{ top: `${pct}%` }}
                  className={cn(
                    "absolute left-1/2 h-1 -translate-x-1/2 -translate-y-1/2 transition-all",
                    widthClass,
                    severityDotClass(e.severity),
                    isActive && "h-1.5 w-6 ring-2 ring-primary"
                  )}
                  aria-label={`Detection ${e.id}`}
                  title={`${new Date(e.ts).toLocaleString()} · ${e.severity}`}
                />
              );
            })}
            {playheadPctTop !== undefined && (
              <div
                className='pointer-events-none absolute left-1/2 h-0.5 w-8 -translate-x-1/2 -translate-y-1/2 bg-primary shadow-[0_0_4px_var(--primary)]'
                style={{ top: `${playheadPctTop}%` }}
                aria-hidden
              />
            )}
          </div>
        </div>
      </div>

      {/* Lower-bound label pinned at the bottom — symmetric with NOW. */}
      <div className='shrink-0 border-border border-t px-2 py-1 text-center text-[10px] text-muted-foreground tabular-nums'>
        {new Date(lowerMs).toLocaleString(undefined, {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit"
        })}
      </div>
    </aside>
  );
};

// ── Violation indicator ─────────────────────────────────────────────────────

/**
 * Player-header badge that surfaces the selected report's verdict
 * details on hover. Replaces the bare severity badge — the operator
 * can now read the VLM's structured findings (missing items,
 * confidence, free-text notes) without leaving the playback view.
 *
 * Click-target stays the badge for keyboard accessibility; hover +
 * focus both open the tooltip.
 */
const ViolationIndicator: React.FC<{
  report: ComplianceReport | undefined;
  severity: Severity | undefined;
}> = ({ report, severity }) => {
  if (!report) {
    return (
      <Badge variant='outline' className='gap-1 text-[10px] text-muted-foreground'>
        <Info className='h-3 w-3' />
        no event
      </Badge>
    );
  }
  const verdict = toStructuredVerdict(report.structured_json);
  const failure = isComplianceFailure(verdict);
  const items = verdict.missing_items?.filter((s) => s.trim().length > 0) ?? [];
  const variant = failure ? "destructive" : severity === "warning" ? "secondary" : "outline";
  const Icon = failure ? AlertTriangle : Info;
  const label = failure ? "violation" : severity === "warning" ? "warning" : "ok";

  return (
    <TooltipProvider delayDuration={150}>
      <Tooltip>
        <TooltipTrigger asChild>
          <Badge
            variant={variant}
            className='cursor-help gap-1 text-[10px]'
            data-testid='violation-indicator'
          >
            <Icon className='h-3 w-3' />
            {label}
          </Badge>
        </TooltipTrigger>
        <TooltipContent
          className={`${DESIGN_REFERENCE} max-w-sm space-y-1.5 p-3 text-left`}
          side='bottom'
          align='end'
        >
          <div className='flex items-center justify-between gap-2 text-[11px]'>
            <span className='font-semibold uppercase tracking-wide'>VLM verdict</span>
            {typeof verdict.confidence === "number" && (
              <span className='text-muted-foreground'>
                confidence {verdict.confidence.toFixed(2)}
              </span>
            )}
          </div>
          {items.length > 0 && (
            <div className='text-xs'>
              <span className='text-muted-foreground'>Missing: </span>
              <span className='font-medium'>{items.join(", ")}</span>
            </div>
          )}
          {verdict.notes && (
            <div className='text-muted-foreground text-xs italic'>"{verdict.notes}"</div>
          )}
          {items.length === 0 && !verdict.notes && (
            <div className='break-words text-muted-foreground text-xs'>
              {reportSnippet(report.report_text, 280)}
            </div>
          )}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
};

export default PlaybackPage;
