import { ArrowUpCircle, Edit3, FileText, Trash2, X } from "lucide-react";
import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { toast } from "sonner";
import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Label } from "@/components/ui/shadcn/label";
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
import useFleetMembers from "@/hooks/api/cameras/useFleetMembers";
import useRemoveFleetMember from "@/hooks/api/cameras/useRemoveFleetMember";
import useSites from "@/hooks/api/cameras/useSites";
import useWorkspaceCost from "@/hooks/api/cameras/useWorkspaceCost";
import { useSelectedSite } from "@/hooks/useSelectedSite";
import ROUTES from "@/libs/utils/routes";
import {
  type CostRange,
  type EdgeBoxCost,
  type EdgeBoxSummary,
  type FleetMemberRow,
  formatCostUsd
} from "@/services/api/cameras";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import AddDeviceDialog from "./AddDeviceDialog";
import CohortCell from "./CohortCell";
import SetTargetImageDialog from "./SetTargetImageDialog";
import TestSlackButton from "./TestSlackButton";
import VlmBudgetCard from "./VlmBudgetCard";

const statusVariant = (status: string): "default" | "secondary" | "destructive" | "outline" => {
  switch (status) {
    case "active":
      return "default";
    case "offline":
      return "destructive";
    case "retired":
      return "outline";
    default:
      return "secondary";
  }
};

/**
 * Threshold for the "new" badge on the EdgeBoxesTable: a row is
 * fresh-looking for 24h after its `registered_at` timestamp.
 * Short enough that the indicator clears itself overnight; long
 * enough that an operator who installs in the morning still sees
 * it in the afternoon. No persistence — the timestamp on the
 * row IS the source of truth.
 */
const NEW_BOX_THRESHOLD_MS = 24 * 60 * 60 * 1000;

function isRecentlyRegistered(registeredAt: string | null | undefined): boolean {
  if (!registeredAt) return false;
  const ts = new Date(registeredAt).getTime();
  if (Number.isNaN(ts)) return false;
  return Date.now() - ts < NEW_BOX_THRESHOLD_MS;
}

/**
 * Format a byte count as a human-friendly string. The bandwidth
 * stamp covers a 5-minute window of MTX outbound, so values are
 * usually KB–MB; GB is included for the edge case of a very
 * chatty box.
 */
function formatBytes(b: number | null | undefined): string {
  if (b == null) return "—";
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  if (b < 1024 * 1024 * 1024) return `${(b / 1024 / 1024).toFixed(1)} MB`;
  return `${(b / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

const EdgeBoxesTable: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const boxHref = (id: string) => ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.BOX(id);
  const { data: fleetMembers = [], isLoading, error, refetch } = useFleetMembers(workspace?.id);

  // Split the merged feed into the two table sections:
  //  - real edge boxes (with operational columns)
  //  - pending claims that haven't bootstrapped yet (identity only)
  // Pending boxes live in a small mini-card above the main table;
  // the existing UI keeps its complex per-box affordances (bulk
  // select, log viewer, set target) for boxes that are actually
  // running.
  const { edgeBoxes, pendingMembers, claimByEdgeId } = useMemo(() => {
    const eb: EdgeBoxSummary[] = [];
    const pending: FleetMemberRow[] = [];
    const byEdge = new Map<string, FleetMemberRow>();
    for (const m of fleetMembers) {
      if (m.edge_box) {
        eb.push({ ...m.edge_box, site_name: m.site_name });
        byEdge.set(m.edge_box.id, m);
      } else if (m.claim_status === "pending_claim") {
        pending.push(m);
      }
    }
    return { edgeBoxes: eb, pendingMembers: pending, claimByEdgeId: byEdge };
  }, [fleetMembers]);

  // Selection state lives here (parent-of-rows). A Set rather than
  // an array because we hit `.has()` on every row render — O(1)
  // beats O(N) when an operator selects 50 boxes.
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [dialogBoxes, setDialogBoxes] = useState<EdgeBoxSummary[]>([]);
  const [addDeviceOpen, setAddDeviceOpen] = useState(false);
  // Pending Remove confirmation. Carries enough info for the
  // mutation call (one or both ids) AND enough to render a
  // meaningful description ("Remove Jetson Orin Nano @ Pokehouse").
  const [removeTarget, setRemoveTarget] = useState<{
    edgeBoxId?: string;
    deviceId?: string;
    label: string;
    siteName: string;
  } | null>(null);
  const removeMember = useRemoveFleetMember(workspace?.id);
  // Bulk-retire confirmation — separate from `removeTarget` because
  // the bulk path operates on the selectedBoxes set rather than a
  // single row, and the copy / button label changes accordingly.
  const [bulkRetireOpen, setBulkRetireOpen] = useState(false);
  // Site filter — defers to the global SitePicker mounted in the
  // edge layout header (`useSelectedSite`, `?site=<uuid>`). One
  // shared source of truth means the filter survives switching
  // between Devices, Topology, Detections, etc., and we avoid the
  // duplicate-dropdown UX of an in-table picker. Falls back to
  // "All sites" when no site is selected; an unknown id (renamed
  // or deleted site) just shows an empty table — the global picker
  // can be reset to All sites to recover.
  const { siteId: selectedSiteId } = useSelectedSite();
  const { data: sites = [] } = useSites(workspace?.id);
  const selectedSiteName = useMemo(
    () => sites.find((s) => s.id === selectedSiteId)?.name ?? null,
    [sites, selectedSiteId]
  );
  // Cost range powering the Cost column. 24h is the most-actionable
  // default ("what is it costing me right now?"); operators can
  // widen to 7d/30d for trend analysis.
  const [costRange, setCostRange] = useState<CostRange>("24h");

  const { data: workspaceCost } = useWorkspaceCost(workspace?.id, costRange);

  // Per-box cost lookup. The workspace rollup ships the breakdown
  // already sorted heaviest-first; we index by id for O(1) lookup
  // in the render loop.
  const costByBoxId = useMemo(() => {
    const m = new Map<string, EdgeBoxCost>();
    for (const b of workspaceCost?.by_box ?? []) m.set(b.edge_box_id, b);
    return m;
  }, [workspaceCost]);

  // Rows shown in the table (and the scope for "select all").
  const visibleBoxes = useMemo(
    () => (selectedSiteId ? edgeBoxes.filter((eb) => eb.site_id === selectedSiteId) : edgeBoxes),
    [edgeBoxes, selectedSiteId]
  );

  // Clear bulk selection whenever the site scope changes so
  // "Set target on N" never lies about which rows are about to
  // be hit — operators reasonably expect a scope change to reset
  // their working set.
  // biome-ignore lint/correctness/useExhaustiveDependencies: only reset on site change
  useEffect(() => {
    setSelectedIds(new Set());
  }, [selectedSiteId]);

  const allSelected = visibleBoxes.length > 0 && visibleBoxes.every((eb) => selectedIds.has(eb.id));
  const someSelected =
    selectedIds.size > 0 && !allSelected && visibleBoxes.some((eb) => selectedIds.has(eb.id));

  const toggleAll = () => {
    if (allSelected || someSelected) {
      setSelectedIds(new Set());
    } else {
      setSelectedIds(new Set(visibleBoxes.map((eb) => eb.id)));
    }
  };

  const toggleOne = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const selectedBoxes = useMemo(
    () => edgeBoxes.filter((eb) => selectedIds.has(eb.id)),
    [edgeBoxes, selectedIds]
  );

  const openBulkDialog = () => {
    if (selectedBoxes.length === 0) return;
    setDialogBoxes(selectedBoxes);
  };

  const openSingleDialog = (eb: EdgeBoxSummary) => {
    setDialogBoxes([eb]);
  };

  const runBulkRetire = async () => {
    if (selectedBoxes.length === 0) return;
    // `Promise.allSettled` over the selected boxes so a single
    // failure (e.g. a 403 because the box belongs to a different
    // workspace, or a transient 5xx) doesn't abort the batch.
    // Each call goes through `removeMember`, which retires the
    // edge_box, revokes its bearers, unbinds its cameras (so the
    // next live box claims them on next config poll), and
    // revokes the linked claim. Workspace size is bounded
    // (~50 PoC) so a parallel fan-out is cheap.
    const results = await Promise.allSettled(
      selectedBoxes.map((eb) => removeMember.mutateAsync({ edgeBoxId: eb.id }))
    );
    const retired = results.filter(
      (r) => r.status === "fulfilled" && r.value.edge_box_retired
    ).length;
    const alreadyRetired = results.filter(
      (r) => r.status === "fulfilled" && !r.value.edge_box_retired
    ).length;
    const failed = results.filter((r) => r.status === "rejected").length;

    setBulkRetireOpen(false);
    if (failed === 0) {
      const parts: string[] = [];
      if (retired > 0) parts.push(`${retired} retired`);
      if (alreadyRetired > 0) parts.push(`${alreadyRetired} already retired`);
      toast.success(`Bulk remove complete: ${parts.join(", ") || "no changes"}.`);
      setSelectedIds(new Set());
    } else {
      const ok = retired + alreadyRetired;
      toast.error(
        `Retired ${ok} of ${results.length}; ${failed} failed. Selection kept so you can retry.`
      );
    }
  };

  return (
    <div className='flex flex-col gap-3'>
      {/* Action buttons — provisioning a new box happens here so
          the operator doesn't have to hunt for it in a sub-tab.
          Add device mints a per-install identity server-side.
          Test Slack fires a no-op alert to validate notification
          wiring without waiting for a real degradation. */}
      <div className='flex items-center justify-end gap-2'>
        <TestSlackButton workspaceId={workspace?.id} />
        <Button size='sm' onClick={() => setAddDeviceOpen(true)}>
          Add device
        </Button>
      </div>

      {/* Pending devices — boxes the operator has added or claimed
          but that haven't called home yet. Surfaced as a small
          banner so the operator doesn't lose track during the
          install window. */}
      {pendingMembers.length > 0 && (
        <PendingDevicesCard
          members={pendingMembers}
          onRemove={(m) =>
            setRemoveTarget({
              deviceId: m.device_id ?? undefined,
              label: m.hardware_revision || m.sku || "pending device",
              siteName: m.site_name
            })
          }
        />
      )}

      {/* Monthly VLM budget — spans full width at the top. Hosts
          the projected-cost banner + inline budget editor. Hidden
          numerics still render (MTD / projected) so operators
          have one place to read both the cap and current burn. */}
      <VlmBudgetCard />

      {/* Header row — scope readout + cost-range picker. Site
          selection lives in the layout header's global SitePicker,
          not here; this strip just echoes the active scope so the
          operator can see at a glance how many boxes the bulk
          actions below will touch. */}
      <div className='flex flex-wrap items-center gap-3'>
        <span className='text-muted-foreground text-xs'>
          {selectedSiteName ? (
            <>
              {visibleBoxes.length} of {edgeBoxes.length} boxes
              <span className='mx-1'>·</span>
              <span className='text-foreground'>at {selectedSiteName}</span>
            </>
          ) : (
            <>{edgeBoxes.length} boxes · all sites</>
          )}
        </span>

        <div className='ml-auto flex items-center gap-2'>
          <Label
            htmlFor='cost-range'
            className='text-muted-foreground text-xs uppercase tracking-wide'
          >
            Cost
          </Label>
          <Select value={costRange} onValueChange={(v) => setCostRange(v as CostRange)}>
            <SelectTrigger id='cost-range' className='h-8 w-24 text-xs'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='24h'>24h</SelectItem>
              <SelectItem value='7d'>7d</SelectItem>
              <SelectItem value='30d'>30d</SelectItem>
            </SelectContent>
          </Select>
          <Badge variant='secondary' className='font-mono'>
            {formatCostUsd(workspaceCost?.cost_usd_micros ?? 0)}
          </Badge>
        </div>
      </div>

      {/* Bulk action bar — appears when ≥1 row is selected. Keeps
          the action discoverable without cluttering each row with
          duplicate buttons. */}
      {selectedBoxes.length > 0 && (
        <div className='flex items-center justify-between gap-3 rounded-md border border-primary/40 bg-primary/5 p-2'>
          <div className='flex items-center gap-2'>
            <Checkbox checked aria-label='Selection indicator' />
            <span className='text-sm'>
              {selectedBoxes.length} {selectedBoxes.length === 1 ? "box" : "boxes"} selected
              {selectedSiteName && (
                <span className='text-muted-foreground'> in {selectedSiteName}</span>
              )}
            </span>
          </div>
          <div className='flex items-center gap-2'>
            <Button size='sm' onClick={openBulkDialog}>
              <ArrowUpCircle className='mr-1.5 h-4 w-4' />
              Set target on {selectedBoxes.length}
            </Button>
            <Button
              size='sm'
              variant='destructive'
              onClick={() => setBulkRetireOpen(true)}
              disabled={removeMember.isPending}
            >
              <Trash2 className='mr-1.5 h-4 w-4' />
              Retire {selectedBoxes.length}
            </Button>
            <Button
              size='icon'
              variant='ghost'
              aria-label='Clear selection'
              onClick={() => setSelectedIds(new Set())}
            >
              <X className='h-4 w-4' />
            </Button>
          </div>
        </div>
      )}

      <TableWrapper>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className='w-8'>
                <Checkbox
                  checked={allSelected ? true : someSelected ? "indeterminate" : false}
                  onCheckedChange={toggleAll}
                  aria-label='Select all edge boxes'
                />
              </TableHead>
              <TableHead>Hardware</TableHead>
              <TableHead>Site</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Cohort</TableHead>
              <TableHead>Last seen</TableHead>
              <TableHead>Outbound (5m)</TableHead>
              <TableHead>VLM cost ({costRange})</TableHead>
              <TableHead>Image</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableContentWrapper
              isEmpty={visibleBoxes.length === 0}
              loading={isLoading}
              colSpan={9}
              noFoundTitle={
                edgeBoxes.length === 0
                  ? "No edge boxes registered"
                  : `No boxes at ${selectedSiteName ?? "this site"}`
              }
              noFoundDescription={
                edgeBoxes.length === 0
                  ? "Register an edge box for a site to start ingesting camera events."
                  : "Switch sites from the picker in the header to see other boxes."
              }
              error={error?.message}
              onRetry={() => refetch()}
            >
              {visibleBoxes.map((eb) => {
                const updatePending =
                  eb.target_image_tag != null && eb.target_image_tag !== eb.current_image_tag;
                const checked = selectedIds.has(eb.id);
                const isNew = isRecentlyRegistered(eb.registered_at);
                return (
                  <TableRow
                    key={eb.id}
                    data-state={checked ? "selected" : undefined}
                    className={isNew ? "bg-primary/5 hover:bg-primary/10" : undefined}
                  >
                    <TableCell>
                      <Checkbox
                        checked={checked}
                        onCheckedChange={() => toggleOne(eb.id)}
                        aria-label={`Select ${eb.hardware_model}`}
                      />
                    </TableCell>
                    <TableCell className='font-medium'>
                      <div className='flex flex-col'>
                        <div className='flex items-center gap-1.5'>
                          <Link
                            to={boxHref(eb.id)}
                            className='hover:underline focus:underline focus:outline-none'
                          >
                            {eb.hardware_model}
                          </Link>
                          {isNew && (
                            <Badge
                              variant='outline'
                              className='border-primary/40 bg-primary/10 text-[10px] text-primary'
                              title={`Registered ${new Date(eb.registered_at).toLocaleString()}`}
                            >
                              new
                            </Badge>
                          )}
                        </div>
                        <span className='text-muted-foreground text-xs'>{eb.image_tag}</span>
                        {eb.edge_compatibility_json?.edge_version && (
                          <span
                            className='font-mono text-[10px] text-muted-foreground'
                            title={
                              eb.edge_compatibility_json.built_at
                                ? `built ${eb.edge_compatibility_json.built_at}`
                                : undefined
                            }
                          >
                            edge v{eb.edge_compatibility_json.edge_version}
                          </span>
                        )}
                        {(() => {
                          const claim = claimByEdgeId.get(eb.id);
                          if (!claim || claim.claim_status === "legacy") return null;
                          return (
                            <span
                              className='font-mono text-[10px] text-muted-foreground'
                              title={`Device ${claim.device_id ?? ""}`}
                            >
                              {claim.claim_status === "claimed"
                                ? "linked to fleet"
                                : "claim pending"}
                            </span>
                          );
                        })()}
                      </div>
                    </TableCell>
                    <TableCell className='text-muted-foreground'>{eb.site_name}</TableCell>
                    <TableCell>
                      <div className='flex flex-wrap items-center gap-1'>
                        <Badge variant={statusVariant(eb.status)}>{eb.status}</Badge>
                        {eb.incompatible_reason && (
                          <Badge
                            variant='outline'
                            className='gap-1 border-amber-500/40 bg-amber-500/10 text-[10px] text-amber-900 dark:text-amber-300'
                            title={eb.incompatible_reason}
                          >
                            needs upgrade
                          </Badge>
                        )}
                      </div>
                    </TableCell>
                    <TableCell className='text-muted-foreground text-xs'>
                      <CohortCell workspaceId={workspace?.id} boxId={eb.id} cohort={eb.cohort} />
                    </TableCell>
                    <TableCell className='text-muted-foreground'>
                      {eb.last_seen_at ? new Date(eb.last_seen_at).toLocaleString() : "Never"}
                    </TableCell>
                    <TableCell className='text-muted-foreground'>
                      <div className='flex flex-col'>
                        <span>{formatBytes(eb.bandwidth_5min_bytes)}</span>
                        {eb.bandwidth_reported_at && (
                          <span className='text-xs'>
                            {new Date(eb.bandwidth_reported_at).toLocaleTimeString()}
                          </span>
                        )}
                      </div>
                    </TableCell>
                    <TableCell className='font-mono text-xs'>
                      {(() => {
                        const c = costByBoxId.get(eb.id);
                        if (!c) return <span className='text-muted-foreground'>—</span>;
                        return (
                          <div className='flex flex-col'>
                            <span>{formatCostUsd(c.cost_usd_micros)}</span>
                            <span className='text-muted-foreground text-xs'>
                              {c.reports} {c.reports === 1 ? "report" : "reports"}
                            </span>
                          </div>
                        );
                      })()}
                    </TableCell>
                    <TableCell>
                      <div className='flex items-center gap-2'>
                        <div className='flex min-w-0 flex-col'>
                          <code className='truncate text-xs'>
                            {eb.current_image_tag ?? "(not reported)"}
                          </code>
                          {updatePending && (
                            <Badge variant='outline' className='mt-0.5 gap-1 self-start'>
                              <ArrowUpCircle className='h-3 w-3' />→ {eb.target_image_tag}
                            </Badge>
                          )}
                          {eb.last_update_result && (
                            <span className='text-muted-foreground text-xs'>
                              last: {eb.last_update_result}
                            </span>
                          )}
                        </div>
                        <Button asChild size='icon' variant='ghost' aria-label='View logs'>
                          <Link to={boxHref(eb.id)}>
                            <FileText className='h-4 w-4' />
                          </Link>
                        </Button>
                        <Button
                          size='icon'
                          variant='ghost'
                          aria-label='Set target image'
                          onClick={() => openSingleDialog(eb)}
                        >
                          <Edit3 className='h-4 w-4' />
                        </Button>
                        <Button
                          size='icon'
                          variant='ghost'
                          aria-label='Remove'
                          onClick={() => {
                            const claim = claimByEdgeId.get(eb.id);
                            setRemoveTarget({
                              edgeBoxId: eb.id,
                              deviceId: claim?.device_id ?? undefined,
                              label: eb.hardware_model,
                              siteName: eb.site_name
                            });
                          }}
                        >
                          <Trash2 className='h-4 w-4' />
                        </Button>
                      </div>
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableContentWrapper>
          </TableBody>
        </Table>
      </TableWrapper>

      <SetTargetImageDialog
        boxes={dialogBoxes}
        open={dialogBoxes.length > 0}
        onOpenChange={(open) => {
          if (!open) {
            setDialogBoxes([]);
            // Clear selection after a successful bulk save — keeps
            // operator from accidentally hitting Apply again on the
            // same boxes.
            if (dialogBoxes.length > 1) setSelectedIds(new Set());
          }
        }}
      />

      <AddDeviceDialog open={addDeviceOpen} onOpenChange={setAddDeviceOpen} />

      <AlertDialog
        open={bulkRetireOpen}
        onOpenChange={(open) => {
          if (!open) setBulkRetireOpen(false);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Retire {selectedBoxes.length} {selectedBoxes.length === 1 ? "edge box" : "edge boxes"}
              ?
            </AlertDialogTitle>
            <AlertDialogDescription asChild>
              <div className='flex flex-col gap-2'>
                <span>
                  Each box's status flips to <strong>retired</strong>, its bearer tokens are
                  revoked, its linked device claim is revoked, and its cameras are unbound (the next
                  live box at the same site claims them on the next config poll). Rows disappear
                  from the table; underlying records remain for compliance reports and audit
                  history.
                </span>
                {selectedBoxes.length <= 12 && (
                  <div className='max-h-40 overflow-y-auto rounded-md border border-border bg-muted/30 p-2 text-foreground text-xs'>
                    <ul className='flex flex-col gap-0.5'>
                      {selectedBoxes.map((eb) => (
                        <li key={eb.id} className='flex justify-between gap-2'>
                          <span className='truncate font-medium'>{eb.hardware_model}</span>
                          <span className='shrink-0 text-muted-foreground'>{eb.site_name}</span>
                        </li>
                      ))}
                    </ul>
                  </div>
                )}
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={removeMember.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={runBulkRetire} disabled={removeMember.isPending}>
              Retire {selectedBoxes.length}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={removeTarget !== null}
        onOpenChange={(open) => {
          if (!open) setRemoveTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove {removeTarget?.label ?? "device"}?</AlertDialogTitle>
            <AlertDialogDescription>
              {removeTarget?.edgeBoxId ? (
                <>
                  Retires the edge box at <strong>{removeTarget.siteName}</strong>, revokes its
                  bearer + any linked device claim. The row disappears from the boxes table; the
                  underlying record stays for compliance reports and audit history.
                </>
              ) : (
                <>
                  Cancels the pending claim for this device. You can re-add it later via Add device.
                </>
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={async () => {
                if (!removeTarget) return;
                try {
                  const res = await removeMember.mutateAsync({
                    edgeBoxId: removeTarget.edgeBoxId,
                    deviceId: removeTarget.deviceId
                  });
                  const parts: string[] = [];
                  if (res.edge_box_retired) parts.push("box retired");
                  if (res.claim_revoked) parts.push("claim revoked");
                  toast.success(
                    parts.length > 0 ? `Removed: ${parts.join(", ")}.` : "Already removed."
                  );
                  setRemoveTarget(null);
                } catch (e) {
                  const msg = e instanceof Error ? e.message : "Remove failed.";
                  toast.error(msg);
                }
              }}
            >
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
};

/** Pending-device mini-card. Surfaces devices the operator has
 *  added or claimed but that haven't called home yet, so they
 *  don't get lost in the gap between provisioning and first
 *  bootstrap. */
const PendingDevicesCard: React.FC<{
  members: FleetMemberRow[];
  onRemove: (member: FleetMemberRow) => void;
}> = ({ members, onRemove }) => (
  <div className='flex flex-col gap-2 rounded-md border border-primary/40 bg-primary/5 p-3'>
    <span className='font-medium text-primary text-sm'>
      {members.length} {members.length === 1 ? "device" : "devices"} waiting to bootstrap
    </span>
    <span className='text-muted-foreground text-xs'>
      These will appear in the table below once they complete their first /control/* call (~5 min
      after the install snippet runs).
    </span>
    <div className='flex flex-col gap-1'>
      {members.map((m) => (
        <div
          key={m.device_id ?? m.site_id}
          className='flex flex-wrap items-center gap-3 rounded-sm border border-border bg-background p-2 text-xs'
        >
          <Badge variant='secondary'>pending</Badge>
          <span className='text-muted-foreground'>{m.site_name}</span>
          <span className='font-mono text-foreground'>
            {m.hardware_revision || m.sku || "device"}
          </span>
          {m.device_id && (
            <span className='font-mono text-muted-foreground'>{m.device_id.slice(0, 8)}…</span>
          )}
          <Button
            size='icon'
            variant='ghost'
            aria-label='Remove pending device'
            className='ml-auto h-6 w-6'
            onClick={() => onRemove(m)}
          >
            <Trash2 className='h-3.5 w-3.5' />
          </Button>
        </div>
      ))}
    </div>
  </div>
);

export default EdgeBoxesTable;
