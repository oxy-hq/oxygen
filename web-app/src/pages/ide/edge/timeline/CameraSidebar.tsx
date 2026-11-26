import { Camera as CameraIcon, MapPin } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import useFleetComplianceSummary from "@/hooks/api/cameras/useFleetComplianceSummary";
import useSites from "@/hooks/api/cameras/useSites";
import { cn } from "@/libs/shadcn/utils";
import type { Camera, CameraComplianceSummary } from "@/services/api/cameras";
import CameraThumbnail from "../components/CameraThumbnail";

type Props = {
  workspaceId: string;
  cameras: Camera[];
  /** ISO timestamp passed to the rollup query so badge counts reflect the active range. */
  since: string | undefined;
  activeCameraId: string | undefined;
  onSelect: (cameraId: string) => void;
};

/**
 * Playback's left-rail camera picker. Replaces the standalone Grid
 * view — operators kept needing to bounce between Grid (to pick a
 * camera) and Single (to actually watch); collapsing the picker
 * into a sidebar means one screen handles both.
 *
 * Cameras are grouped by site (sticky headers so the section label
 * stays visible while scrolling a long fleet), and each item shows
 * a small thumbnail + violation count badge so the operator can
 * eyeball "where's the action" without entering each camera.
 */
const CameraSidebar: React.FC<Props> = ({
  workspaceId,
  cameras,
  since,
  activeCameraId,
  onSelect
}) => {
  const { data: allSites = [] } = useSites(workspaceId);
  // Same scope-aware narrow as the old Grid view: when every visible
  // camera shares one site, push the narrow into the backend; else
  // the rollup is workspace-wide.
  const siteIds = useMemo(() => Array.from(new Set(cameras.map((c) => c.site_id))), [cameras]);
  const narrowSiteId = siteIds.length === 1 ? siteIds[0] : null;
  const { data: fleetSummary = [] } = useFleetComplianceSummary(workspaceId, {
    siteId: narrowSiteId,
    since
  });

  const visibleCameraIds = useMemo(() => new Set(cameras.map((c) => c.id)), [cameras]);
  const summaryByCamera = useMemo(() => {
    const m = new Map<string, CameraComplianceSummary>();
    for (const row of fleetSummary) {
      if (visibleCameraIds.has(row.camera_id)) m.set(row.camera_id, row);
    }
    return m;
  }, [fleetSummary, visibleCameraIds]);

  const grouped = useMemo(() => {
    const nameById = new Map(allSites.map((s) => [s.id, s.name] as const));
    const bySite = new Map<string, Camera[]>();
    for (const c of cameras) {
      const list = bySite.get(c.site_id) ?? [];
      list.push(c);
      bySite.set(c.site_id, list);
    }
    const groups = Array.from(bySite.entries()).map(([siteId, cams]) => ({
      siteId,
      siteName: nameById.get(siteId) ?? "Unknown site",
      cameras: [...cams].sort((a, b) => a.name.localeCompare(b.name))
    }));
    return groups.sort((a, b) => a.siteName.localeCompare(b.siteName));
  }, [cameras, allSites]);

  if (cameras.length === 0) {
    return (
      <div className='flex h-full flex-col items-center justify-center gap-2 p-4 text-center text-muted-foreground'>
        <CameraIcon className='h-6 w-6' />
        <p className='text-xs'>No cameras in this scope.</p>
      </div>
    );
  }

  return (
    <div className='flex h-full flex-col overflow-y-auto' data-testid='camera-sidebar'>
      {grouped.map((g) => (
        <section key={g.siteId} className='flex flex-col'>
          <div className='sticky top-0 z-10 flex items-center justify-between gap-2 border-border border-b bg-card px-3 py-1.5'>
            <h3 className='flex items-center gap-1.5 font-semibold text-[10px] text-muted-foreground uppercase tracking-wider'>
              <MapPin className='h-3 w-3' /> {g.siteName}
            </h3>
            <span className='text-[10px] text-muted-foreground'>
              {g.cameras.length} camera{g.cameras.length === 1 ? "" : "s"}
            </span>
          </div>
          <ul className='flex flex-col gap-1 p-2'>
            {g.cameras.map((c) => {
              const summary = summaryByCamera.get(c.id);
              const violations = summary?.violations ?? 0;
              const isActive = c.id === activeCameraId;
              return (
                <li key={c.id}>
                  <button
                    type='button'
                    onClick={() => onSelect(c.id)}
                    className={cn(
                      "flex w-full gap-2 border border-transparent p-1.5 text-left transition-colors hover:bg-muted/40",
                      isActive && "border-primary bg-primary/5"
                    )}
                    data-testid={`camera-sidebar-item-${c.id}`}
                  >
                    <div className='relative h-14 w-24 shrink-0 overflow-hidden bg-black'>
                      <CameraThumbnail
                        cameraId={c.id}
                        className='h-full w-full rounded-none border-0'
                      />
                      {violations > 0 && (
                        <Badge
                          variant='destructive'
                          className='absolute top-1 left-1 h-4 px-1 text-[9px]'
                        >
                          {violations}
                        </Badge>
                      )}
                    </div>
                    <div className='flex min-w-0 flex-1 flex-col justify-center'>
                      <p className='truncate font-medium text-xs'>{c.name}</p>
                      <p className='truncate text-[10px] text-muted-foreground'>
                        {c.role ?? "unassigned"}
                      </p>
                      <p className='text-[10px] text-muted-foreground tabular-nums'>
                        {summary?.total_reports ?? 0} detections
                      </p>
                    </div>
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
};

export default CameraSidebar;
