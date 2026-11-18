import type { EChartsOption } from "echarts";
import {
  AlertTriangle,
  Bell,
  Camera,
  CheckCircle2,
  Clock,
  Cpu,
  Film,
  Loader2,
  type LucideIcon,
  ShieldAlert,
  XCircle
} from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { Echarts } from "@/components/Echarts";
import { Badge } from "@/components/ui/shadcn/badge";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import useCameraHealthSummary from "@/hooks/api/cameras/useCameraHealthSummary";
import useCameras from "@/hooks/api/cameras/useCameras";
import useDashboardRollup from "@/hooks/api/cameras/useDashboardRollup";
import useEdgeBoxes from "@/hooks/api/cameras/useEdgeBoxes";
import useRecentAlerts from "@/hooks/api/cameras/useRecentAlerts";
import useSites from "@/hooks/api/cameras/useSites";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { useSelectedSite } from "@/hooks/useSelectedSite";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import SetupNeededBanner from "./components/SetupNeededBanner";
import { RANGE_OPTIONS, rangeToSince } from "./timeline/format";

/**
 * Dashboard — fleet-wide overview. Volume-first per the answer to
 * "what's the dominant operator question?": tiles answer "how much
 * is happening?" up top, the activity chart visualizes "when," and
 * health rolls below as a "is anything broken?" backstop.
 *
 * Data flow is one rollup query per workspace (replaces the per-site
 * fan-out used in v0 — cost is now O(1) in site count). The
 * Recent alerts feed is also a single round-trip backed by
 * audit_events rows the alerter writes on every transition.
 *
 * Site picker: when set, all KPIs scope to that site server-side via
 * the rollup endpoint's `site_id` param; when "All sites" is selected,
 * the rollup covers the whole workspace.
 */
const DashboardPage: React.FC = () => {
  const { project } = useCurrentProjectBranch();
  const { workspace } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const workspaceId = workspace?.id ?? project.id;

  const { siteId: selectedSite } = useSelectedSite();
  const [range, setRange] = useState("24h");
  const since = useMemo(() => rangeToSince(range), [range]);

  const { data: allSites = [] } = useSites(workspaceId);
  const { data: edgeBoxes = [] } = useEdgeBoxes(workspaceId);
  const { data: cameras = [] } = useCameras(workspaceId);
  const { data: healthRows = [] } = useCameraHealthSummary(workspaceId);
  const { data: rollup, isLoading: rollupLoading } = useDashboardRollup(workspaceId, {
    siteId: selectedSite,
    since
  });
  const { data: recentAlerts = [], isLoading: alertsLoading } = useRecentAlerts(workspaceId, {
    siteId: selectedSite,
    limit: 10
  });

  // Cameras / boxes in scope — used for health bucket math and box
  // KPI tile. Independent of the rollup (rollup is Airhouse-driven;
  // these are Postgres-driven).
  const sitesInScope = useMemo(
    () => (selectedSite ? allSites.filter((s) => s.id === selectedSite) : allSites),
    [allSites, selectedSite]
  );
  const cameraIdsInScope = useMemo(() => {
    const allowedSite = new Set(sitesInScope.map((s) => s.id));
    return new Set(cameras.filter((c) => allowedSite.has(c.site_id)).map((c) => c.id));
  }, [cameras, sitesInScope]);
  const boxesInScope = useMemo(() => {
    const allowedSite = new Set(sitesInScope.map((s) => s.id));
    return edgeBoxes.filter((b) => allowedSite.has(b.site_id));
  }, [edgeBoxes, sitesInScope]);

  const totals = rollup?.totals;

  // Health KPIs from the per-camera health rollup. Scope to cameras
  // in scope so the dashboard counts match what the operator sees in
  // Devices when filtered to the same site.
  const healthBuckets = useMemo(() => {
    const scoped = healthRows.filter((h) => cameraIdsInScope.has(h.camera_id));
    const buckets = { ok: 0, degraded: 0, stale: 0, unknown: 0 };
    for (const h of scoped) {
      const key =
        (h.status as keyof typeof buckets) in buckets
          ? (h.status as keyof typeof buckets)
          : "unknown";
      buckets[key] += 1;
    }
    return buckets;
  }, [healthRows, cameraIdsInScope]);

  const boxesActive = boxesInScope.filter((b) => b.status === "active").length;

  // Privacy-safe default: cameras are created with
  // `analytics_consent = false`. This counts the scoped cameras
  // still awaiting operator review so the KPI row can surface a
  // "Setup needed" pill that links into review mode. Only counted
  // when active — inactive cameras aren't analytics candidates.
  const pendingReviewCount = useMemo(
    () =>
      cameras.filter((c) => cameraIdsInScope.has(c.id) && c.active && !c.analytics_consent).length,
    [cameras, cameraIdsInScope]
  );

  // Pre-compute camera-name lookup for the attention table (rollup
  // top-N tables already carry the name, but the health table doesn't).
  const cameraNameById = useMemo(() => {
    const m = new Map<string, string>();
    for (const c of cameras) m.set(c.id, c.name);
    return m;
  }, [cameras]);

  // Cameras needing attention — anything non-ok in the rollup. Sort
  // by status severity (stale > degraded) then name so worst-first
  // ordering is stable across refetches.
  const attentionCameras = useMemo(() => {
    const severityOrder = { stale: 0, degraded: 1, unknown: 2, ok: 3 };
    return healthRows
      .filter((h) => cameraIdsInScope.has(h.camera_id) && h.status !== "ok")
      .map((h) => ({
        ...h,
        cameraName: cameraNameById.get(h.camera_id) ?? h.camera_id.slice(0, 8)
      }))
      .sort((a, b) => {
        const ao = severityOrder[a.status as keyof typeof severityOrder] ?? 9;
        const bo = severityOrder[b.status as keyof typeof severityOrder] ?? 9;
        return ao - bo || a.cameraName.localeCompare(b.cameraName);
      })
      .slice(0, 8);
  }, [healthRows, cameraIdsInScope, cameraNameById]);

  const chartOptions = useTimeSeriesOptions(rollup?.hourly ?? []);

  const detections = ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.DETECTIONS;
  const playback = ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.PLAYBACK;
  const devices = ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.DEVICES;

  return (
    <div
      className='flex h-full min-h-0 flex-col gap-4 overflow-auto p-4'
      data-testid='edge-dashboard'
    >
      <SetupNeededBanner />
      <div className='flex shrink-0 items-center gap-3'>
        <span className='text-muted-foreground text-xs'>
          {selectedSite ? (sitesInScope[0]?.name ?? "Site") : "Fleet-wide"}
        </span>
        <span className='text-muted-foreground'>·</span>
        <Select value={range} onValueChange={setRange}>
          <SelectTrigger className='h-8 w-36 text-xs'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {RANGE_OPTIONS.map((o) => (
              <SelectItem key={o.value} value={o.value}>
                {o.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {rollupLoading && (
          <span className='flex items-center gap-1 text-muted-foreground text-xs'>
            <Loader2 className='h-3 w-3 animate-spin' /> Loading rollup…
          </span>
        )}
      </div>

      <div
        className={cn(
          "grid shrink-0 grid-cols-2 gap-3",
          // Stretch to 5 columns on large screens when the
          // Setup tile is present so the row stays one line;
          // 4-col layout is preserved when there's nothing to
          // review (the common steady state).
          pendingReviewCount > 0 ? "md:grid-cols-4 lg:grid-cols-5" : "md:grid-cols-4"
        )}
      >
        <KpiTile
          label='Detections'
          value={(totals?.detections ?? 0).toLocaleString()}
          sublabel={`${totals?.cameras_with_activity ?? 0} of ${cameraIdsInScope.size} cameras active`}
          icon={Film}
          tone='default'
          href={detections}
        />
        <KpiTile
          label='Violations'
          value={(totals?.violations ?? 0).toLocaleString()}
          sublabel={
            totals && totals.detections > 0
              ? `${((totals.violations / totals.detections) * 100).toFixed(0)}% of detections`
              : "—"
          }
          icon={ShieldAlert}
          tone={totals && totals.violations > 0 ? "destructive" : "default"}
          href={`${detections}?severity=critical`}
        />
        <KpiTile
          label='Cameras online'
          value={`${healthBuckets.ok} / ${cameraIdsInScope.size}`}
          sublabel={
            healthBuckets.degraded + healthBuckets.stale > 0
              ? `${healthBuckets.degraded + healthBuckets.stale} need attention`
              : "All healthy"
          }
          icon={Camera}
          tone={healthBuckets.degraded + healthBuckets.stale > 0 ? "warning" : "success"}
        />
        <KpiTile
          label='Edge boxes online'
          value={`${boxesActive} / ${boxesInScope.length}`}
          sublabel={boxesActive < boxesInScope.length ? "Some boxes offline" : "All boxes active"}
          icon={Cpu}
          tone={boxesActive < boxesInScope.length ? "warning" : "success"}
        />
        {/* Conditional Setup tile — only renders when there are
            cameras awaiting review. Matches the SetupNeededBanner
            above; the tile lets operators see the count in their
            usual KPI strip without scrolling. */}
        {pendingReviewCount > 0 && (
          <KpiTile
            label='Setup needed'
            value={pendingReviewCount.toLocaleString()}
            sublabel={`${pendingReviewCount === 1 ? "Camera" : "Cameras"} awaiting analytics review`}
            icon={Bell}
            tone='warning'
            href={`${devices}?review=1`}
          />
        )}
      </div>

      {/* Time-series chart — answers "when is activity happening?" */}
      <Card title='Activity over time' icon={Film}>
        {(rollup?.hourly?.length ?? 0) === 0 ? (
          <EmptyHint>
            {rollupLoading ? "Loading…" : "No detections in the selected range."}
          </EmptyHint>
        ) : (
          <Echarts options={chartOptions} isLoading={rollupLoading} />
        )}
      </Card>

      <div className='grid shrink-0 grid-cols-1 gap-3 md:grid-cols-2'>
        <Card title='Top cameras by detections' icon={Film} href={detections}>
          {(rollup?.top_by_detections.length ?? 0) === 0 ? (
            <EmptyHint>No detections in the selected range.</EmptyHint>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Camera</TableHead>
                  <TableHead className='text-right'>Detections</TableHead>
                  <TableHead className='text-right'>Violations</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rollup?.top_by_detections.map((s) => (
                  <TableRow key={s.camera_id} className='cursor-pointer hover:bg-muted/40'>
                    <TableCell className='font-medium text-sm'>
                      <Link
                        to={`${playback}?camera=${encodeURIComponent(s.camera_id)}`}
                        className='hover:underline'
                      >
                        {s.camera_name}
                      </Link>
                    </TableCell>
                    <TableCell className='text-right tabular-nums'>
                      {s.detections.toLocaleString()}
                    </TableCell>
                    <TableCell className='text-right'>
                      {s.violations > 0 ? (
                        <Badge variant='destructive'>{s.violations}</Badge>
                      ) : (
                        <span className='text-muted-foreground'>0</span>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </Card>

        <Card title='Top violations' icon={ShieldAlert} href={`${detections}?severity=critical`}>
          {(rollup?.top_by_violations.length ?? 0) === 0 ? (
            <EmptyHint>No violations in the selected range.</EmptyHint>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Camera</TableHead>
                  <TableHead className='text-right'>Violations</TableHead>
                  <TableHead className='text-right'>Of total</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rollup?.top_by_violations.map((s) => (
                  <TableRow key={s.camera_id} className='cursor-pointer hover:bg-muted/40'>
                    <TableCell className='font-medium text-sm'>
                      <Link
                        to={`${playback}?camera=${encodeURIComponent(s.camera_id)}`}
                        className='hover:underline'
                      >
                        {s.camera_name}
                      </Link>
                    </TableCell>
                    <TableCell className='text-right'>
                      <Badge variant='destructive'>{s.violations}</Badge>
                    </TableCell>
                    <TableCell className='text-right text-muted-foreground text-xs tabular-nums'>
                      {s.detections > 0
                        ? `${((s.violations / s.detections) * 100).toFixed(0)}%`
                        : "—"}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </Card>
      </div>

      <Card title='Recent alerts' icon={Bell}>
        {recentAlerts.length === 0 ? (
          <EmptyHint>
            {alertsLoading ? "Loading…" : "No camera transitions in recent history."}
          </EmptyHint>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>When</TableHead>
                <TableHead>Camera</TableHead>
                <TableHead>Site</TableHead>
                <TableHead>Transition</TableHead>
                <TableHead>Reason</TableHead>
                <TableHead className='text-right'>Slack</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {recentAlerts.map((row) => (
                <TableRow key={row.id} className='cursor-pointer hover:bg-muted/40'>
                  <TableCell className='text-muted-foreground text-xs'>
                    {new Date(row.created_at).toLocaleString()}
                  </TableCell>
                  <TableCell className='font-medium text-sm'>
                    {row.camera_id ? (
                      <Link
                        to={`${playback}?camera=${encodeURIComponent(row.camera_id)}`}
                        className='hover:underline'
                      >
                        {row.camera_name ?? row.camera_id.slice(0, 8)}
                      </Link>
                    ) : (
                      <span className='text-muted-foreground'>—</span>
                    )}
                  </TableCell>
                  <TableCell className='text-muted-foreground text-xs'>
                    {row.site_name ?? "—"}
                  </TableCell>
                  <TableCell>
                    <span className='inline-flex items-center gap-1 text-xs'>
                      <HealthChip
                        label={row.from || "?"}
                        count={undefined}
                        tone={statusTone(row.from)}
                      />
                      <span className='text-muted-foreground'>→</span>
                      <HealthChip
                        label={row.to || "?"}
                        count={undefined}
                        tone={statusTone(row.to)}
                      />
                    </span>
                  </TableCell>
                  <TableCell className='text-muted-foreground text-xs'>
                    {row.reason || "—"}
                  </TableCell>
                  <TableCell className='text-right text-xs'>
                    <DeliveryBadge delivered={row.delivered} />
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Card>

      <Card title='Health' icon={CheckCircle2}>
        <div className='mb-2 flex flex-wrap gap-1.5'>
          <HealthChip label='ok' count={healthBuckets.ok} tone='success' />
          <HealthChip label='degraded' count={healthBuckets.degraded} tone='warning' />
          <HealthChip label='stale' count={healthBuckets.stale} tone='destructive' />
          <HealthChip label='unknown' count={healthBuckets.unknown} tone='muted' />
        </div>
        {attentionCameras.length === 0 ? (
          <EmptyHint>Every camera is reporting clean.</EmptyHint>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Camera</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Reason</TableHead>
                <TableHead className='text-right'>Last frame</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {attentionCameras.map((row) => (
                <TableRow key={row.camera_id} className='cursor-pointer hover:bg-muted/40'>
                  <TableCell className='font-medium text-sm'>
                    <Link
                      to={`${playback}?camera=${encodeURIComponent(row.camera_id)}`}
                      className='hover:underline'
                    >
                      {row.cameraName}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <HealthChip
                      label={row.status}
                      count={undefined}
                      tone={statusTone(row.status)}
                    />
                  </TableCell>
                  <TableCell className='text-muted-foreground text-xs'>
                    {row.reason || "—"}
                  </TableCell>
                  <TableCell className='text-right text-muted-foreground text-xs'>
                    {row.last_frame_at ? new Date(row.last_frame_at).toLocaleString() : "Never"}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Card>
    </div>
  );
};

// ── Time-series chart options ──────────────────────────────────────────────

/**
 * Build the ECharts options for the hourly activity chart. Two
 * stacked-area-style line series: detections and violations.
 * Computed via `useMemo` to avoid rebuilding the (large) options
 * object on every render of the parent dashboard.
 */
function useTimeSeriesOptions(
  hourly: { hour: string; detections: number; violations: number }[]
): EChartsOption {
  return useMemo<EChartsOption>(() => {
    const x = hourly.map((b) => new Date(b.hour).toLocaleString());
    return {
      grid: { left: 50, right: 20, top: 30, bottom: 30 },
      tooltip: { trigger: "axis" },
      legend: { top: 0 },
      xAxis: {
        type: "category",
        data: x,
        axisLabel: { fontSize: 10 }
      },
      yAxis: { type: "value", minInterval: 1 },
      series: [
        {
          name: "Detections",
          type: "line",
          smooth: true,
          showSymbol: false,
          areaStyle: { opacity: 0.15 },
          data: hourly.map((b) => b.detections)
        },
        {
          name: "Violations",
          type: "line",
          smooth: true,
          showSymbol: false,
          areaStyle: { opacity: 0.15 },
          data: hourly.map((b) => b.violations)
        }
      ]
    };
  }, [hourly]);
}

// ── Small presentational helpers ────────────────────────────────────────────

const toneClass = (tone: "default" | "success" | "warning" | "destructive") => {
  switch (tone) {
    case "success":
      return "border-emerald-500/40 bg-emerald-500/5";
    case "warning":
      return "border-amber-500/40 bg-amber-500/5";
    case "destructive":
      return "border-destructive/40 bg-destructive/5";
    default:
      return "border-border bg-card";
  }
};

const KpiTile: React.FC<{
  label: string;
  value: string;
  sublabel: string;
  icon: LucideIcon;
  tone: "default" | "success" | "warning" | "destructive";
  href?: string;
}> = ({ label, value, sublabel, icon: Icon, tone, href }) => {
  const body = (
    <div
      className={cn(
        "flex flex-col gap-1 rounded-lg border p-3 transition-shadow",
        toneClass(tone),
        href && "hover:shadow-sm"
      )}
    >
      <div className='flex items-center gap-1.5 text-muted-foreground text-xs uppercase tracking-wide'>
        <Icon className='h-3 w-3' />
        {label}
      </div>
      <div className='font-semibold text-2xl tabular-nums'>{value}</div>
      <div className='text-muted-foreground text-xs'>{sublabel}</div>
    </div>
  );
  return href ? <Link to={href}>{body}</Link> : body;
};

const Card: React.FC<{
  title: string;
  icon: LucideIcon;
  href?: string;
  children: React.ReactNode;
}> = ({ title, icon: Icon, href, children }) => (
  <div className='flex shrink-0 flex-col overflow-hidden rounded-lg border border-border bg-card'>
    <div className='flex items-center justify-between border-border border-b px-3 py-2'>
      <h3 className='flex items-center gap-1.5 font-semibold text-sm'>
        <Icon className='h-3.5 w-3.5 text-muted-foreground' />
        {title}
      </h3>
      {href && (
        <Link to={href} className='text-muted-foreground text-xs hover:text-foreground'>
          See all →
        </Link>
      )}
    </div>
    <div className='p-2'>{children}</div>
  </div>
);

const EmptyHint: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div className='flex items-center justify-center py-6 text-muted-foreground text-sm'>
    {children}
  </div>
);

const statusTone = (status: string): "success" | "warning" | "destructive" | "muted" => {
  switch (status) {
    case "ok":
      return "success";
    case "degraded":
      return "warning";
    case "stale":
      return "destructive";
    default:
      return "muted";
  }
};

const HealthChip: React.FC<{
  label: string;
  count: number | undefined;
  tone: "success" | "warning" | "destructive" | "muted";
}> = ({ label, count, tone }) => {
  const Icon =
    tone === "success"
      ? CheckCircle2
      : tone === "warning"
        ? AlertTriangle
        : tone === "destructive"
          ? XCircle
          : Clock;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-md border px-2 py-0.5 text-xs",
        tone === "success" &&
          "border-emerald-500/40 bg-emerald-500/5 text-emerald-900 dark:text-emerald-300",
        tone === "warning" &&
          "border-amber-500/40 bg-amber-500/5 text-amber-900 dark:text-amber-300",
        tone === "destructive" && "border-destructive/40 bg-destructive/5 text-destructive",
        tone === "muted" && "border-border bg-muted/30 text-muted-foreground"
      )}
    >
      <Icon className='h-3 w-3' />
      {label}
      {count !== undefined && <span className='tabular-nums'>· {count}</span>}
    </span>
  );
};

/**
 * Slack delivery indicator on the Recent alerts row. Three states
 * map to three reads:
 *   `true`  → "sent"           (Slack accepted the message)
 *   `false` → "no Slack"       (workspace hasn't connected Slack)
 *   `null`  → "send failed"    (notify call errored — see server logs)
 */
const DeliveryBadge: React.FC<{ delivered: boolean | null }> = ({ delivered }) => {
  if (delivered === true) {
    return (
      <Badge variant='secondary' className='font-normal'>
        sent
      </Badge>
    );
  }
  if (delivered === false) {
    return <span className='text-muted-foreground'>no Slack</span>;
  }
  return <Badge variant='destructive'>failed</Badge>;
};

export default DashboardPage;
