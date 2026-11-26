import { ChevronRight, Film, Filter, PlayCircle } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Spinner } from "@/components/ui/shadcn/spinner";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger
} from "@/components/ui/shadcn/tooltip";
import {
  useArbitration,
  useDeleteArbitration,
  usePutArbitration
} from "@/hooks/api/cameras/useArbitration";
import useCameras from "@/hooks/api/cameras/useCameras";
import useFleetComplianceReports from "@/hooks/api/cameras/useFleetComplianceReports";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { useSelectedSite } from "@/hooks/useSelectedSite";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import type { ComplianceReport } from "@/services/api/cameras";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import {
  eventSeverity,
  formatReceivedAt,
  isComplianceFailure,
  RANGE_OPTIONS,
  rangeToSince,
  reportSnippet,
  type Severity,
  severityDotClass,
  severityLabel,
  toStructuredVerdict
} from "./timeline/format";

type SeverityFilter = "all" | Severity;

/**
 * Detections — analyst-flow browser for VLM compliance verdicts.
 *
 * Successor to the events sidebar of the legacy Timeline tab, with
 * additional filters operators asked for: confidence threshold +
 * trigger-type narrow + severity chips.
 *
 * Data flow is one fleet-wide query via `useFleetComplianceReports`
 * (site narrow lives in the backend `site_id` param) — independent
 * of camera count. Per-page Camera filter is then a client-side
 * narrow on the already-loaded set so the dropdown is instant.
 *
 * Site picker scopes the camera set; a per-page Camera filter
 * narrows further (or "All cameras in scope").
 *
 * Row click → cross-links to Playback with `?camera=X&event=Y` so
 * the natural "what was that?" flow opens the clip with one click.
 */
const DetectionsPage: React.FC = () => {
  const { project } = useCurrentProjectBranch();
  const { workspace } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const navigate = useNavigate();
  const workspaceId = workspace?.id ?? project.id;

  const { data: cameras = [], isLoading: camerasLoading } = useCameras(workspaceId);
  const { siteId: selectedSite } = useSelectedSite();

  // Cameras the dropdown can narrow to — global site picker scopes
  // them. The actual report fetch is fleet-wide; the per-page camera
  // filter is applied client-side after the data lands.
  const [cameraFilter, setCameraFilter] = useState<string>("all");
  const scopedCameras = useMemo(
    () => (selectedSite ? cameras.filter((c) => c.site_id === selectedSite) : cameras),
    [cameras, selectedSite]
  );

  const [range, setRange] = useState("24h");
  const since = useMemo(() => rangeToSince(range), [range]);

  // Single fleet-wide query — independent of camera count.
  const { data: fleetReports = [], isLoading: reportsLoading } = useFleetComplianceReports(
    workspaceId,
    { siteId: selectedSite, since, limit: 500 }
  );
  const loading = reportsLoading || camerasLoading;
  const allReports = useMemo<ComplianceReport[]>(() => {
    if (cameraFilter === "all") return fleetReports;
    return fleetReports.filter((r) => r.camera_id === cameraFilter);
  }, [fleetReports, cameraFilter]);

  // Camera-name lookup for the table.
  const cameraNameById = useMemo(() => {
    const m = new Map<string, string>();
    for (const c of cameras) m.set(c.id, c.name);
    return m;
  }, [cameras]);

  // Available trigger types — derived from the loaded set so the
  // dropdown only ever shows values that actually exist.
  const triggerTypes = useMemo(() => {
    const set = new Set<string>();
    for (const r of allReports) set.add(r.trigger_type);
    return Array.from(set).sort();
  }, [allReports]);

  const [severityFilter, setSeverityFilter] = useState<SeverityFilter>("all");
  const [triggerFilter, setTriggerFilter] = useState<string>("all");
  const [minConfidence, setMinConfidence] = useState<number>(0);
  // Phase 1 disagreement utilization — operators flip this to surface
  // the review queue. Default "all" so the page behaves as it does
  // today; the value here drives both the table filter and the
  // banner below the filter strip.
  const [agreementFilter, setAgreementFilter] = useState<"all" | "disagree" | "agree">("all");

  // Tag once with severity + parsed verdict for filter + render.
  const tagged = useMemo(
    () =>
      allReports.map((r) => {
        const verdict = toStructuredVerdict(r.structured_json);
        return {
          report: r,
          severity: eventSeverity(r.structured_json, r.received_at),
          confidence: verdict.confidence ?? 0,
          isFailure: isComplianceFailure(verdict)
        };
      }),
    [allReports]
  );

  const filtered = useMemo(
    () =>
      tagged.filter((t) => {
        if (severityFilter !== "all" && t.severity !== severityFilter) return false;
        if (triggerFilter !== "all" && t.report.trigger_type !== triggerFilter) return false;
        if (minConfidence > 0 && (t.confidence ?? 0) < minConfidence) return false;
        if (agreementFilter !== "all" && t.report.agreement_status !== agreementFilter) {
          return false;
        }
        return true;
      }),
    [tagged, severityFilter, triggerFilter, minConfidence, agreementFilter]
  );

  // Pre-compute the disagreement count for the filter button label —
  // surfaces the size of the review queue without an extra round-trip.
  const disagreeCount = useMemo(
    () => tagged.filter((t) => t.report.agreement_status === "disagree").length,
    [tagged]
  );

  const openInPlayback = (r: ComplianceReport) => {
    const playback = ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.PLAYBACK;
    navigate(
      `${playback}?camera=${encodeURIComponent(r.camera_id)}&event=${encodeURIComponent(r.report_id)}`
    );
  };

  return (
    <div className='flex h-full min-h-0 flex-col gap-3 p-4' data-testid='edge-detections'>
      {/* Filter strip */}
      <div className='flex flex-wrap items-center gap-3'>
        <Label htmlFor='det-camera' className='text-muted-foreground'>
          Camera
        </Label>
        <Select value={cameraFilter} onValueChange={setCameraFilter}>
          <SelectTrigger id='det-camera' className='w-56'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            <SelectItem value='all'>All cameras{selectedSite ? " in site" : ""}</SelectItem>
            {scopedCameras.map((c) => (
              <SelectItem key={c.id} value={c.id}>
                {c.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <Label htmlFor='det-range' className='text-muted-foreground'>
          Range
        </Label>
        <Select value={range} onValueChange={setRange}>
          <SelectTrigger id='det-range' className='w-44'>
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

        <Label htmlFor='det-trigger' className='text-muted-foreground'>
          Trigger
        </Label>
        <Select value={triggerFilter} onValueChange={setTriggerFilter}>
          <SelectTrigger id='det-trigger' className='w-44'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            <SelectItem value='all'>All triggers</SelectItem>
            {triggerTypes.map((t) => (
              <SelectItem key={t} value={t}>
                {t}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <div className='flex items-center gap-1.5'>
          <Label htmlFor='det-conf' className='text-muted-foreground'>
            Min confidence
          </Label>
          <Input
            id='det-conf'
            type='number'
            min={0}
            max={1}
            step={0.05}
            value={minConfidence}
            onChange={(e) => setMinConfidence(Number(e.target.value))}
            className='h-8 w-20 text-xs'
          />
        </div>

        <div className='flex items-center gap-1.5'>
          <Filter className='h-3.5 w-3.5 text-muted-foreground' />
          {(["all", "critical", "warning", "info"] as SeverityFilter[]).map((s) => (
            <Button
              key={s}
              variant={severityFilter === s ? "default" : "outline"}
              size='sm'
              onClick={() => setSeverityFilter(s)}
              className='h-7 text-xs capitalize'
            >
              {s}
            </Button>
          ))}
        </div>

        {/* Review queue toggle — disagreements between VLM and YOLO.
            Button label includes the count so operators can size the
            queue without flipping the filter on. */}
        <div className='flex items-center gap-1.5'>
          <Button
            variant={agreementFilter === "disagree" ? "default" : "outline"}
            size='sm'
            onClick={() => setAgreementFilter((f) => (f === "disagree" ? "all" : "disagree"))}
            className='h-7 text-xs'
            title='Show only events where VLM and YOLO disagreed — these are the highest-value review candidates.'
          >
            Disagreements
            {disagreeCount > 0 && (
              <Badge variant='secondary' className='ml-1.5 h-4 px-1 text-[10px]'>
                {disagreeCount}
              </Badge>
            )}
          </Button>
        </div>

        <span className='ml-auto text-muted-foreground text-xs'>
          {loading ? "Loading…" : `${filtered.length} of ${tagged.length}`}
        </span>
      </div>

      {/* Results table */}
      <div className='min-h-0 flex-1 overflow-auto border bg-card'>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className='w-32'>Severity</TableHead>
              <TableHead>When</TableHead>
              <TableHead>Camera</TableHead>
              <TableHead>Trigger</TableHead>
              <TableHead>Confidence</TableHead>
              <TableHead>Report</TableHead>
              <TableHead className='w-44'>VLM ↔ YOLO</TableHead>
              <TableHead className='w-16 text-right' aria-label='Open' />
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow>
                <TableCell colSpan={8} className='py-8'>
                  <div className='flex justify-center'>
                    <Spinner />
                  </div>
                </TableCell>
              </TableRow>
            ) : filtered.length === 0 ? (
              <TableRow>
                <TableCell colSpan={8} className='py-8 text-center text-muted-foreground text-sm'>
                  <Film className='mx-auto mb-2 h-6 w-6' />
                  {tagged.length === 0
                    ? "No detections in this range."
                    : "No detections match the current filters."}
                </TableCell>
              </TableRow>
            ) : (
              filtered.map(({ report: r, severity, confidence, isFailure }) => (
                <TableRow
                  key={r.report_id}
                  className='cursor-pointer hover:bg-muted/40'
                  onClick={() => openInPlayback(r)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      openInPlayback(r);
                    }
                  }}
                  role='link'
                  tabIndex={0}
                  data-testid={`detection-row-${r.report_id}`}
                >
                  <TableCell>
                    <span className='inline-flex items-center gap-1.5 font-medium text-xs capitalize'>
                      <span
                        className={cn("h-1.5 w-1.5 rounded-full", severityDotClass(severity))}
                      />
                      {severityLabel(severity)}
                      {isFailure && (
                        <Badge variant='destructive' className='ml-1 text-[10px]'>
                          violation
                        </Badge>
                      )}
                    </span>
                  </TableCell>
                  <TableCell className='whitespace-nowrap font-medium text-xs tabular-nums'>
                    {formatReceivedAt(r.received_at)}
                  </TableCell>
                  <TableCell className='text-xs'>
                    {cameraNameById.get(r.camera_id) ?? r.camera_id.slice(0, 8)}
                  </TableCell>
                  <TableCell className='text-muted-foreground text-xs'>{r.trigger_type}</TableCell>
                  <TableCell className='text-muted-foreground text-xs tabular-nums'>
                    {confidence > 0 ? confidence.toFixed(2) : "—"}
                  </TableCell>
                  {/* TableCell max-width is unreliable without
                      `table-layout: fixed` on the parent — wrap in
                      a constrained inner div so a single-line
                      JSON-shaped report_text (``` ```json {... ```
                      from VLMs that ignored the natural-language
                      instruction) can't burst the row.
                      `break-all` is the strong form: breaks anywhere,
                      including mid-token, so a payload with no
                      whitespace at all still wraps. `line-clamp-3`
                      caps the cell at three lines; full text lives
                      on the detail page once that exists. */}
                  <TableCell className='text-foreground text-sm'>
                    <TooltipProvider delayDuration={150}>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          {/* Truncated preview is the click-target;
                              the tooltip below carries the full,
                              untruncated text for hover reads
                              without leaving the table. */}
                          <div className='line-clamp-3 max-w-md cursor-help break-all'>
                            {reportSnippet(r.report_text, 220)}
                          </div>
                        </TooltipTrigger>
                        <TooltipContent
                          side='bottom'
                          align='start'
                          className={`${DESIGN_REFERENCE} max-w-xl whitespace-pre-wrap break-all p-3 text-left font-mono text-xs leading-relaxed`}
                        >
                          {r.report_text}
                        </TooltipContent>
                      </Tooltip>
                    </TooltipProvider>
                  </TableCell>
                  <TableCell>
                    <ArbitrationCell reportId={r.report_id} agreementStatus={r.agreement_status} />
                  </TableCell>
                  <TableCell className='text-right'>
                    <Button
                      size='sm'
                      variant='ghost'
                      onClick={(e) => {
                        e.stopPropagation();
                        openInPlayback(r);
                      }}
                      aria-label='Open in Playback'
                    >
                      <PlayCircle className='h-4 w-4' />
                      <ChevronRight className='h-3.5 w-3.5' />
                    </Button>
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  );
};

// ── Arbitration cell ────────────────────────────────────────────────────────

/**
 * Per-row affordance for VLM↔YOLO arbitration. Renders an agreement
 * pill plus three quick-action buttons for `disagree` rows. The
 * arbitration mutation upserts on the server — clicking the same
 * verdict twice is a no-op, clicking a different verdict overwrites.
 *
 * Click handling stops propagation so the row's "open in Playback"
 * navigation doesn't fire when the operator labels.
 */
const ArbitrationCell: React.FC<{
  reportId: string;
  agreementStatus: ComplianceReport["agreement_status"];
}> = ({ reportId, agreementStatus }) => {
  const { workspace } = useCurrentWorkspace();
  const { data: arbitration } = useArbitration(workspace?.id, reportId);
  const putMutation = usePutArbitration(workspace?.id);
  const deleteMutation = useDeleteArbitration(workspace?.id);

  // No agreement signal at all (legacy rows) — render nothing so the
  // column stays clean. Inconclusive gets a quiet "—" so the table
  // alignment doesn't shift.
  if (agreementStatus == null) {
    return <span className='text-muted-foreground text-xs'>—</span>;
  }
  if (agreementStatus === "inconclusive") {
    return (
      <Badge
        variant='outline'
        className='border-border bg-muted/30 text-[10px] text-muted-foreground'
      >
        no signal
      </Badge>
    );
  }
  if (agreementStatus === "agree") {
    return (
      <Badge variant='outline' className='border-dot-success/40 bg-dot-success/5 text-[10px]'>
        agree
      </Badge>
    );
  }

  // agreement_status === "disagree" — the high-value review case.
  const arbVerdict = arbitration?.verdict;
  // Single handler used on both `onClick` and `onKeyDown`. Typed
  // permissively as the shared `SyntheticEvent` base so TS accepts
  // it in both prop slots; only `.stopPropagation()` is invoked.
  const stop = (e: React.SyntheticEvent) => e.stopPropagation();
  const setVerdict = (verdict: "vlm_right" | "yolo_right" | "neither") => async () => {
    if (arbVerdict === verdict) {
      await deleteMutation.mutateAsync({ reportId });
    } else {
      await putMutation.mutateAsync({ reportId, verdict });
    }
  };

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: this wrapper only stops event propagation so the parent <TableRow role=link> doesn't intercept button clicks; the interactive elements inside (<Button>) carry their own handlers.
    <div className='flex flex-col gap-1' onClick={stop} onKeyDown={stop}>
      <Badge variant='destructive' className='w-fit text-[10px]'>
        disagree
      </Badge>
      <div className='flex flex-wrap gap-1'>
        <Button
          size='sm'
          variant={arbVerdict === "vlm_right" ? "default" : "outline"}
          onClick={setVerdict("vlm_right")}
          disabled={putMutation.isPending || deleteMutation.isPending}
          className='h-5 px-1.5 text-[10px]'
          title='Mark VLM as the correct verdict — captures this frame as a training signal.'
        >
          VLM
        </Button>
        <Button
          size='sm'
          variant={arbVerdict === "yolo_right" ? "default" : "outline"}
          onClick={setVerdict("yolo_right")}
          disabled={putMutation.isPending || deleteMutation.isPending}
          className='h-5 px-1.5 text-[10px]'
        >
          YOLO
        </Button>
        <Button
          size='sm'
          variant={arbVerdict === "neither" ? "default" : "outline"}
          onClick={setVerdict("neither")}
          disabled={putMutation.isPending || deleteMutation.isPending}
          className='h-5 px-1.5 text-[10px]'
        >
          neither
        </Button>
      </div>
    </div>
  );
};

export default DetectionsPage;
