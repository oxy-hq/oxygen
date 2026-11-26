import { CheckCircle2, Loader2, Plus, RefreshCcw, Rocket, XCircle } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
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
import useEdgeBoxes from "@/hooks/api/cameras/useEdgeBoxes";
import {
  useAbortRolloutPlan,
  useCreateRolloutPlan,
  useRolloutPlans
} from "@/hooks/api/cameras/useRolloutPlans";
import ROUTES from "@/libs/utils/routes";
import type { RolloutPlan } from "@/services/api/cameras";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

const TERMINAL_STATUSES = new Set(["complete", "aborted"]);

const statusBadge = (status: string) => {
  switch (status) {
    case "complete":
      return (
        <Badge className='gap-1' variant='default'>
          <CheckCircle2 className='h-3 w-3' /> Complete
        </Badge>
      );
    case "aborted":
      return (
        <Badge className='gap-1' variant='destructive'>
          <XCircle className='h-3 w-3' /> Aborted
        </Badge>
      );
    case "canary":
      return (
        <Badge className='gap-1 border-dot-warning/40 bg-dot-warning/10 text-status-warning'>
          <Loader2 className='h-3 w-3 animate-spin' /> Canary
        </Badge>
      );
    case "promoting":
      return (
        <Badge className='gap-1' variant='secondary'>
          <Loader2 className='h-3 w-3 animate-spin' /> Promoting
        </Badge>
      );
    default:
      return (
        <Badge className='gap-1' variant='outline'>
          <RefreshCcw className='h-3 w-3' /> {status}
        </Badge>
      );
  }
};

const formatWhen = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "—");

const RolloutsPage: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const navigate = useNavigate();
  const { data: plans = [], isLoading, error, refetch } = useRolloutPlans(workspace?.id);
  const [newOpen, setNewOpen] = useState(false);
  const abort = useAbortRolloutPlan(workspace?.id);

  const handleAbort = async (plan: RolloutPlan) => {
    const reason = window.prompt("Reason for aborting (optional)") ?? undefined;
    try {
      await abort.mutateAsync({ planId: plan.id, reason });
      toast.success("Rollout aborted.");
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Failed to abort.");
    }
  };

  return (
    <div className='flex h-full min-h-0 flex-col gap-3 p-4' data-testid='edge-rollouts'>
      <div className='flex items-center justify-between'>
        <div>
          <h2 className='font-semibold text-base'>Rollouts</h2>
          <p className='text-muted-foreground text-xs'>
            Canary-first OTA image updates. The supervisor watches each canary cohort and
            auto-promotes to the full fleet when the soak window passes without failures.
          </p>
        </div>
        <Button size='sm' onClick={() => setNewOpen(true)}>
          <Plus className='h-3.5 w-3.5' /> New rollout
        </Button>
      </div>

      <div className='overflow-hidden border'>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Target image</TableHead>
              <TableHead>Canary cohort</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Started</TableHead>
              <TableHead>Soak</TableHead>
              <TableHead className='text-right' aria-label='Actions' />
            </TableRow>
          </TableHeader>
          <TableBody>
            {isLoading ? (
              <TableRow>
                <TableCell colSpan={6} className='py-8'>
                  <div className='flex justify-center'>
                    <Spinner />
                  </div>
                </TableCell>
              </TableRow>
            ) : error ? (
              <TableRow>
                <TableCell colSpan={6} className='py-8 text-center'>
                  <p className='text-destructive text-sm'>
                    {error instanceof Error ? error.message : "Failed to load rollouts."}
                  </p>
                  <Button size='sm' variant='outline' className='mt-2' onClick={() => refetch()}>
                    Retry
                  </Button>
                </TableCell>
              </TableRow>
            ) : plans.length === 0 ? (
              <TableRow>
                <TableCell colSpan={6} className='py-8 text-center text-muted-foreground text-sm'>
                  No rollouts yet. Click <em>New rollout</em> to ship an image tag to a cohort.
                </TableCell>
              </TableRow>
            ) : (
              plans.map((p) => (
                <TableRow
                  key={p.id}
                  data-testid={`rollout-row-${p.id}`}
                  className='cursor-pointer hover:bg-muted/40'
                  onClick={() => navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.ROLLOUT(p.id))}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.ROLLOUT(p.id));
                    }
                  }}
                  role='link'
                  tabIndex={0}
                >
                  <TableCell className='font-mono text-xs'>{p.target_image_tag}</TableCell>
                  <TableCell className='text-sm'>{p.canary_cohort}</TableCell>
                  <TableCell>{statusBadge(p.status)}</TableCell>
                  <TableCell className='text-muted-foreground text-xs'>
                    {formatWhen(p.canary_started_at)}
                  </TableCell>
                  <TableCell className='text-muted-foreground text-xs'>
                    {p.canary_min_minutes} min
                  </TableCell>
                  <TableCell className='text-right'>
                    {!TERMINAL_STATUSES.has(p.status) && (
                      <Button
                        size='sm'
                        variant='outline'
                        onClick={(e) => {
                          e.stopPropagation();
                          void handleAbort(p);
                        }}
                        disabled={abort.isPending}
                      >
                        Abort
                      </Button>
                    )}
                    {p.status === "aborted" && p.abort_reason && (
                      <span className='text-muted-foreground text-xs' title={p.abort_reason}>
                        {p.abort_reason.length > 40
                          ? `${p.abort_reason.slice(0, 40)}…`
                          : p.abort_reason}
                      </span>
                    )}
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>

      <NewRolloutDialog open={newOpen} onOpenChange={setNewOpen} workspaceId={workspace?.id} />
    </div>
  );
};

// ── New rollout wizard ─────────────────────────────────────────────────────

const NewRolloutDialog: React.FC<{
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspaceId: string | undefined;
}> = ({ open, onOpenChange, workspaceId }) => {
  const create = useCreateRolloutPlan(workspaceId);
  const { data: edgeBoxes = [] } = useEdgeBoxes(workspaceId);

  // Derive the cohort dropdown from cohorts actually present on
  // edge boxes — no point letting the operator type a cohort that
  // doesn't match anything (the supervisor would auto-abort with
  // "no boxes match"). Sorted alphabetically for predictability.
  const cohorts = useMemo(() => {
    const set = new Set<string>(edgeBoxes.map((b) => b.cohort).filter(Boolean));
    return Array.from(set).sort();
  }, [edgeBoxes]);

  const [tag, setTag] = useState("");
  const [cohort, setCohort] = useState<string>("");
  const [soakMinutes, setSoakMinutes] = useState<number>(1440);
  const [thresholdPct, setThresholdPct] = useState<number>(5);

  const reset = () => {
    setTag("");
    setCohort("");
    setSoakMinutes(1440);
    setThresholdPct(5);
  };

  const handleSubmit = async () => {
    if (!tag.trim() || !cohort.trim()) {
      toast.error("Target image tag and canary cohort are required.");
      return;
    }
    try {
      await create.mutateAsync({
        target_image_tag: tag.trim(),
        canary_cohort: cohort.trim(),
        canary_min_minutes: soakMinutes,
        failure_threshold_pct: thresholdPct
      });
      toast.success("Rollout created. Supervisor will start canary on next tick.");
      reset();
      onOpenChange(false);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Failed to create rollout.");
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={DESIGN_REFERENCE}>
        <DialogHeader>
          <DialogTitle className='flex items-center gap-2'>
            <Rocket className='h-4 w-4 text-primary' /> New rollout
          </DialogTitle>
          <DialogDescription>
            The supervisor applies the target image to your canary cohort first, watches for
            failures, then auto-promotes to the rest of the fleet after the soak window.
          </DialogDescription>
        </DialogHeader>
        <div className='flex flex-col gap-3 py-2'>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='rollout-tag'>Target image tag</Label>
            <Input
              id='rollout-tag'
              placeholder='ghcr.io/oxy-hq/oxy-edge:v2.3.1'
              value={tag}
              onChange={(e) => setTag(e.target.value)}
              className='font-mono text-xs'
            />
          </div>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='rollout-cohort'>Canary cohort</Label>
            <Select value={cohort} onValueChange={setCohort}>
              <SelectTrigger id='rollout-cohort'>
                <SelectValue placeholder='Pick a cohort' />
              </SelectTrigger>
              <SelectContent className={DESIGN_REFERENCE}>
                {cohorts.length === 0 ? (
                  <SelectItem value='__none' disabled>
                    No cohorts found — set <code>cohort</code> on at least one edge box first.
                  </SelectItem>
                ) : (
                  cohorts.map((c) => (
                    <SelectItem key={c} value={c}>
                      {c} ({edgeBoxes.filter((b) => b.cohort === c).length} boxes)
                    </SelectItem>
                  ))
                )}
              </SelectContent>
            </Select>
          </div>
          <div className='grid grid-cols-2 gap-3'>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='rollout-soak'>Soak window (min)</Label>
              <Input
                id='rollout-soak'
                type='number'
                min={0}
                value={soakMinutes}
                onChange={(e) => setSoakMinutes(Number(e.target.value))}
              />
            </div>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='rollout-threshold'>Failure threshold (%)</Label>
              <Input
                id='rollout-threshold'
                type='number'
                min={0}
                max={100}
                step={0.5}
                value={thresholdPct}
                onChange={(e) => setThresholdPct(Number(e.target.value))}
              />
            </div>
          </div>
          <p className='text-muted-foreground text-xs'>
            With these settings: apply <code className='font-mono'>{tag || "<tag>"}</code> to the{" "}
            <strong>{cohort || "<cohort>"}</strong> cohort, soak {soakMinutes} min, auto-abort if
            more than {thresholdPct}% of canary boxes report a revert.
          </p>
        </div>
        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)} disabled={create.isPending}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={create.isPending}>
            {create.isPending && <Loader2 className='h-3.5 w-3.5 animate-spin' />}
            Create rollout
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default RolloutsPage;
