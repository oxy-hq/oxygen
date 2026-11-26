import { Plus, ShieldAlert, ShieldCheck, X } from "lucide-react";
import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Switch } from "@/components/ui/shadcn/switch";
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
import useEdgeBoxes from "@/hooks/api/cameras/useEdgeBoxes";
import usePatchCameraConsent from "@/hooks/api/cameras/usePatchCameraConsent";
import usePatchCameraRole from "@/hooks/api/cameras/usePatchCameraRole";
import { useSelectedSite } from "@/hooks/useSelectedSite";
import type { CameraHealthRow, CameraSummary } from "@/services/api";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import AddCameraDialog from "./AddCameraDialog";
import CameraPlayerDialog from "./CameraPlayerDialog";
import CameraRolePicker from "./CameraRolePicker";
import CameraThumbnail from "./CameraThumbnail";
import ZoneEditorDialog from "./ZoneEditorDialog";

/** Special value the Select uses for "no filter — show everything." */
const ALL_EDGE_BOXES = "__all__";
/** Special value for "cameras not bound to any edge box yet." */
const UNBOUND_EDGE_BOX = "__unbound__";

const CamerasTable: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const { data: cameras = [], isLoading, error, refetch } = useCameras(workspace?.id);
  const { data: edgeBoxes = [] } = useEdgeBoxes(workspace?.id);
  const { data: healthRows = [] } = useCameraHealthSummary(workspace?.id);

  // Index health rows by camera_id so the per-row lookup is O(1)
  // — the table renders up to ~50 cameras per workspace, so this
  // matters less than for the cost rollup but keeps the same
  // shape and avoids an N²-find on every render.
  const healthByCameraId = useMemo(() => {
    const m = new Map<string, CameraHealthRow>();
    for (const h of healthRows) m.set(h.camera_id, h);
    return m;
  }, [healthRows]);

  // First edge box id, or null when none are registered yet. Pulled
  // out so the useEffect deps below are stable + safe — the prior
  // version used `edgeBoxes[0].id` directly in deps which crashes
  // on empty arrays.
  const firstEdgeBoxId = edgeBoxes[0]?.id ?? null;

  // Filter cameras by edge box. Default to the first registered edge
  // box: in restaurant-scale deployments (1-2 boxes per workspace)
  // that's what the operator wants to see first anyway, and it caps
  // the number of cameras polling thumbnails at one site's worth.
  const [edgeBoxFilter, setEdgeBoxFilter] = useState<string>(
    () => firstEdgeBoxId ?? ALL_EDGE_BOXES
  );

  // Once edgeBoxes loads (it's async), promote to its first entry if
  // we mounted before the query settled. The `=== ALL_EDGE_BOXES`
  // guard prevents clobbering a user-chosen filter.
  useEffect(() => {
    if (edgeBoxFilter === ALL_EDGE_BOXES && firstEdgeBoxId) {
      setEdgeBoxFilter(firstEdgeBoxId);
    }
  }, [firstEdgeBoxId, edgeBoxFilter]);

  // Review mode — driven by `?review=1` in the URL. The
  // SetupNeededBanner on Dashboard + Devices deep-links here when
  // there are cameras with `analytics_consent = false`. In this
  // mode the table is filtered to those cameras, the bulk-enable
  // action surfaces, and a soft amber banner reminds the operator
  // why they're here. The URL param IS the mode toggle so a
  // page reload or a deep-link reproduces the state.
  const [searchParams, setSearchParams] = useSearchParams();
  const reviewMode = searchParams.get("review") === "1";
  // Global site picker (URL-backed). Same source the rest of the
  // Edge tab uses — see `useSelectedSite`. Filtering here keeps the
  // table consistent with the banner that deep-links into it and
  // prevents bulk actions (consent flip, etc.) from reaching
  // cameras outside the active site's scope.
  const { siteId: selectedSiteId } = useSelectedSite();
  const exitReviewMode = () => {
    setSearchParams(
      (prev) => {
        const params = new URLSearchParams(prev);
        params.delete("review");
        return params;
      },
      { replace: true }
    );
  };

  const filtered = useMemo(() => {
    let list: CameraSummary[];
    if (edgeBoxFilter === ALL_EDGE_BOXES) list = cameras;
    else if (edgeBoxFilter === UNBOUND_EDGE_BOX)
      list = cameras.filter((c) => c.edge_box_id == null);
    else list = cameras.filter((c) => c.edge_box_id === edgeBoxFilter);
    // Global site filter — applies in every mode, not just review.
    // The banner counts pending cameras per site; bulk-enable from
    // here must not reach beyond that scope.
    if (selectedSiteId) list = list.filter((c) => c.site_id === selectedSiteId);
    // Review-mode narrow: only show cameras still awaiting
    // operator review (consent off, active true — inactive
    // cameras aren't candidates for analytics anyway).
    if (reviewMode) list = list.filter((c) => c.active && !c.analytics_consent);
    return list;
  }, [cameras, edgeBoxFilter, reviewMode, selectedSiteId]);

  const [playerCamera, setPlayerCamera] = useState<CameraSummary | null>(null);
  const [zoneEditorCamera, setZoneEditorCamera] = useState<CameraSummary | null>(null);
  const [addCameraOpen, setAddCameraOpen] = useState(false);
  const patchConsent = usePatchCameraConsent(workspace?.id);
  const patchRole = usePatchCameraRole(workspace?.id);

  const enableAllVisible = async () => {
    if (filtered.length === 0) return;
    // Fire mutations in parallel — bounded by `filtered.length`
    // which is workspace-cap (~50 max in PoC) so we don't need
    // a chunked queue. Promise.allSettled so a single 5xx on one
    // camera doesn't abort the batch.
    const results = await Promise.allSettled(
      filtered.map((c) => patchConsent.mutateAsync({ cameraId: c.id, consent: true }))
    );
    const ok = results.filter((r) => r.status === "fulfilled").length;
    const failed = results.length - ok;
    if (failed === 0) {
      toast.success(`Enabled analytics on ${ok} ${ok === 1 ? "camera" : "cameras"}.`);
      exitReviewMode();
    } else {
      toast.error(
        `Enabled ${ok} of ${results.length}; ${failed} failed — leaving review mode on for the rest.`
      );
    }
  };

  return (
    <div className='flex flex-col gap-3'>
      {reviewMode && (
        <div
          className='flex items-center justify-between gap-3 border border-dot-warning/40 bg-dot-warning/5 px-3 py-2 text-status-warning'
          data-testid='cameras-review-banner'
        >
          <div className='flex min-w-0 items-center gap-2'>
            <ShieldAlert className='h-4 w-4 shrink-0' />
            <div className='flex min-w-0 flex-col text-xs'>
              <span className='font-medium'>Review mode</span>
              <span className='text-status-warning/80'>
                {filtered.length === 0
                  ? "Nothing left to review in this filter. Exit to see the full list."
                  : `${filtered.length} ${
                      filtered.length === 1 ? "camera" : "cameras"
                    } awaiting setup. Set zones first, then enable analytics — individually or with the bulk action.`}
              </span>
            </div>
          </div>
          <div className='flex shrink-0 items-center gap-2'>
            {filtered.length > 0 && (
              <Button
                size='sm'
                onClick={enableAllVisible}
                disabled={patchConsent.isPending}
                className='gap-1.5'
              >
                <ShieldCheck className='h-3.5 w-3.5' />
                Enable analytics on {filtered.length}
              </Button>
            )}
            <Button
              size='sm'
              variant='ghost'
              onClick={exitReviewMode}
              className='gap-1'
              aria-label='Exit review mode'
            >
              <X className='h-3.5 w-3.5' />
              Exit
            </Button>
          </div>
        </div>
      )}
      <div className='flex items-center gap-3'>
        <Label htmlFor='edge-box-filter' className='text-muted-foreground'>
          Edge box
        </Label>
        <Select value={edgeBoxFilter} onValueChange={setEdgeBoxFilter}>
          <SelectTrigger id='edge-box-filter' className='w-72'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            {edgeBoxes.map((eb) => (
              <SelectItem key={eb.id} value={eb.id}>
                {eb.hardware_model} @ {eb.site_name}
              </SelectItem>
            ))}
            <SelectItem value={UNBOUND_EDGE_BOX}>Unbound cameras</SelectItem>
            <SelectItem value={ALL_EDGE_BOXES}>All cameras in workspace</SelectItem>
          </SelectContent>
        </Select>
        <span className='text-muted-foreground text-xs'>
          {filtered.length} of {cameras.length} cameras
        </span>
        <Button size='sm' className='ml-auto' onClick={() => setAddCameraOpen(true)}>
          <Plus className='mr-1.5 h-4 w-4' />
          Add camera
        </Button>
      </div>

      <TableWrapper>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Preview</TableHead>
              <TableHead>Name</TableHead>
              <TableHead>Site</TableHead>
              <TableHead>Edge box</TableHead>
              <TableHead>Online</TableHead>
              <TableHead>Health</TableHead>
              <TableHead>Role</TableHead>
              <TableHead>Triggers</TableHead>
              <TableHead>Active</TableHead>
              <TableHead>Analytics</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableContentWrapper
              isEmpty={filtered.length === 0}
              loading={isLoading}
              colSpan={10}
              noFoundTitle={
                cameras.length === 0 ? "No cameras yet" : "No cameras for this edge box"
              }
              noFoundDescription={
                cameras.length === 0
                  ? "Import cameras from UniFi or add an edge box that publishes them."
                  : "Switch the Edge box filter above to see other cameras."
              }
              error={error?.message}
              onRetry={() => refetch()}
            >
              {filtered.map((cam) => (
                <TableRow
                  key={cam.id}
                  className='cursor-pointer'
                  onClick={() => setPlayerCamera(cam)}
                >
                  <TableCell>
                    <CameraThumbnail cameraId={cam.id} />
                  </TableCell>
                  <TableCell className='font-medium'>
                    <div className='flex flex-col'>
                      <span>{cam.name}</span>
                      {cam.model && (
                        <span className='text-muted-foreground text-xs'>{cam.model}</span>
                      )}
                    </div>
                  </TableCell>
                  <TableCell className='text-muted-foreground'>{cam.site_name}</TableCell>
                  <TableCell className='text-muted-foreground'>
                    <div className='flex flex-col'>
                      <span>{cam.edge_box_name ?? "—"}</span>
                      {/* `offline` from stale_checker (90s no /control/* call).
                          Surfacing here even when cam.online is "yes" because
                          the camera being live at UniFi doesn't mean we can
                          reach it for preview — the snapshot / HLS / WebRTC
                          proxies all need the edge box reachable. */}
                      {cam.edge_box_status === "offline" && (
                        <span className='text-destructive text-xs'>edge offline</span>
                      )}
                      {cam.edge_box_status === "pending" && (
                        <span className='text-muted-foreground text-xs italic'>
                          waiting for first check-in
                        </span>
                      )}
                    </div>
                  </TableCell>
                  <TableCell>
                    {cam.online == null ? (
                      <span className='text-muted-foreground'>?</span>
                    ) : (
                      <Badge variant={cam.online ? "default" : "destructive"}>
                        {cam.online ? "yes" : "no"}
                      </Badge>
                    )}
                  </TableCell>
                  <TableCell>
                    <HealthBadge row={healthByCameraId.get(cam.id)} />
                  </TableCell>
                  <TableCell>
                    <CameraRolePicker
                      value={cam.role}
                      disabled={patchRole.isPending}
                      onChange={(role) => patchRole.mutate({ cameraId: cam.id, role })}
                      ariaLabel={`Role for ${cam.name}`}
                    />
                  </TableCell>
                  <TableCell>
                    {/* Triggers cell: zone + line count, edit button.
                        Click-trap mirrors the consent cell pattern —
                        the button is the interactive element, we don't
                        want it to also open the live-preview dialog. */}
                    {/* biome-ignore lint/a11y/noStaticElementInteractions: wrapper exists only to trap row-click propagation */}
                    <div
                      role='presentation'
                      onClick={(e) => e.stopPropagation()}
                      onKeyDown={(e) => e.stopPropagation()}
                      className='flex items-center gap-2'
                    >
                      <span className='text-muted-foreground text-xs'>
                        {(cam.zones_json as unknown[] | null)?.length ?? 0}z ·{" "}
                        {(cam.lines_json as unknown[] | null)?.length ?? 0}L
                      </span>
                      {(cam.proposed_zones_json != null || cam.proposed_lines_json != null) && (
                        <Badge variant='outline' className='text-xs'>
                          proposal
                        </Badge>
                      )}
                      <Button size='sm' variant='outline' onClick={() => setZoneEditorCamera(cam)}>
                        Edit
                      </Button>
                    </div>
                  </TableCell>
                  <TableCell>
                    <Badge variant={cam.active ? "default" : "secondary"}>
                      {cam.active ? "yes" : "no"}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    {/* The wrapping div is a click-trap: it stops the
                        row's open-player handler from firing when the
                        operator is just toggling consent. The Switch
                        inside IS the interactive element (it carries the
                        aria-label), the div is purely structural —
                        hence role="presentation" to keep a11y lints
                        quiet. */}
                    {/* biome-ignore lint/a11y/noStaticElementInteractions: wrapper exists only to trap row-click propagation; the Switch inside is the interactive element */}
                    <div
                      role='presentation'
                      onClick={(e) => e.stopPropagation()}
                      onKeyDown={(e) => e.stopPropagation()}
                    >
                      <Switch
                        checked={cam.analytics_consent}
                        disabled={patchConsent.isPending}
                        onCheckedChange={(checked) =>
                          patchConsent.mutate({ cameraId: cam.id, consent: checked })
                        }
                        aria-label={`Analytics consent for ${cam.name}`}
                      />
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableContentWrapper>
          </TableBody>
        </Table>
      </TableWrapper>

      <CameraPlayerDialog
        camera={playerCamera}
        open={!!playerCamera}
        onOpenChange={(open) => {
          if (!open) setPlayerCamera(null);
        }}
      />

      <ZoneEditorDialog
        camera={zoneEditorCamera}
        open={!!zoneEditorCamera}
        onOpenChange={(open) => {
          if (!open) setZoneEditorCamera(null);
        }}
      />

      <AddCameraDialog open={addCameraOpen} onOpenChange={setAddCameraOpen} />
    </div>
  );
};

/** Worker-reported camera health — fps, decoder errors,
 *  reconnects, last frame. Sub-`Online` because `online` is the
 *  UniFi-reported state ("does the controller think this camera
 *  exists") whereas this is "is the worker actually decoding
 *  frames right now." Both matter; this one is more actionable.
 *
 *  Status taxonomy comes verbatim from the backend so the badge
 *  stays in sync without UI-side translation logic. */
const HealthBadge: React.FC<{ row: CameraHealthRow | undefined }> = ({ row }) => {
  if (!row || row.status === "unknown") {
    return <span className='text-muted-foreground text-xs'>—</span>;
  }
  const variant: "default" | "secondary" | "destructive" | "outline" =
    row.status === "ok"
      ? "default"
      : row.status === "degraded"
        ? "outline"
        : row.status === "stale"
          ? "destructive"
          : "secondary";
  return (
    <div className='flex flex-col gap-0.5' title={row.reason || undefined}>
      <Badge variant={variant}>{row.status}</Badge>
      {typeof row.fps === "number" && (
        <span className='text-muted-foreground text-xs'>{row.fps.toFixed(1)} fps</span>
      )}
    </div>
  );
};

export default CamerasTable;
