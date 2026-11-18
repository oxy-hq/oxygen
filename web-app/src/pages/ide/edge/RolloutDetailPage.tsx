import {
  AlertTriangle,
  ArrowLeft,
  CheckCircle2,
  Circle,
  Clock,
  Minus,
  XCircle
} from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { Link, useParams } from "react-router-dom";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { useRolloutConvergence, useRolloutPlans } from "@/hooks/api/cameras/useRolloutPlans";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import type { RolloutConvergenceRow } from "@/services/api/cameras";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

const formatWhen = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "—");

const statusIcon = (status: RolloutConvergenceRow["convergence_status"]) => {
  switch (status) {
    case "converged":
      return <CheckCircle2 className='h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400' />;
    case "failed":
      return <XCircle className='h-3.5 w-3.5 text-destructive' />;
    case "pending":
      return <Clock className='h-3.5 w-3.5 text-amber-500' />;
    case "untargeted":
      return <Minus className='h-3.5 w-3.5 text-muted-foreground' />;
    default:
      return <Circle className='h-3.5 w-3.5 text-muted-foreground' />;
  }
};

const statusLabel = (status: RolloutConvergenceRow["convergence_status"]) => {
  switch (status) {
    case "converged":
      return "Converged";
    case "failed":
      return "Failed";
    case "pending":
      return "Pending";
    case "untargeted":
      return "Not targeted yet";
  }
};

const RolloutDetailPage: React.FC = () => {
  const { planId } = useParams<{ planId: string }>();
  const { workspace } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const back = ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.ROLLOUTS;

  // We pull the plan summary off the list query so a deep-link to
  // a detail page doesn't double-fetch — the user almost always
  // arrives via the list.
  const { data: plans = [] } = useRolloutPlans(workspace?.id);
  const plan = useMemo(() => plans.find((p) => p.id === planId), [plans, planId]);

  const { data: rows = [], isLoading, error } = useRolloutConvergence(workspace?.id, planId);

  // Per-status counts for the summary strip — gives the operator
  // a one-glance answer to "how is this rollout doing?" before
  // they scan the per-box table.
  const summary = useMemo(() => {
    return rows.reduce(
      (acc, r) => {
        acc[r.convergence_status]++;
        if (r.in_canary) acc.canary++;
        return acc;
      },
      { converged: 0, failed: 0, pending: 0, untargeted: 0, canary: 0 }
    );
  }, [rows]);

  return (
    <div className='flex h-full min-h-0 flex-col gap-3 p-4' data-testid='edge-rollout-detail'>
      <div className='flex items-start gap-3'>
        <Button asChild variant='ghost' size='sm' className='-ml-2'>
          <Link to={back}>
            <ArrowLeft className='h-4 w-4' /> Rollouts
          </Link>
        </Button>
        <div className='flex flex-col'>
          <h2 className='font-semibold text-base'>
            {plan ? (
              <code className='font-mono text-base'>{plan.target_image_tag}</code>
            ) : (
              "Rollout"
            )}
          </h2>
          {plan && (
            <p className='text-muted-foreground text-xs'>
              Cohort <strong>{plan.canary_cohort}</strong> · status <strong>{plan.status}</strong>
              {plan.aborted_at && plan.abort_reason && ` · ${plan.abort_reason}`}
            </p>
          )}
        </div>
      </div>

      {/* Summary strip — derived counts from the per-box rows. */}
      <div className='flex flex-wrap gap-2'>
        <Badge variant='outline' className='gap-1.5'>
          {statusIcon("converged")} Converged · {summary.converged}
        </Badge>
        <Badge variant='outline' className='gap-1.5'>
          {statusIcon("failed")} Failed · {summary.failed}
        </Badge>
        <Badge variant='outline' className='gap-1.5'>
          {statusIcon("pending")} Pending · {summary.pending}
        </Badge>
        <Badge variant='outline' className='gap-1.5'>
          {statusIcon("untargeted")} Not targeted · {summary.untargeted}
        </Badge>
        <Badge variant='secondary' className='gap-1.5'>
          Canary · {summary.canary}
        </Badge>
      </div>

      <div className='overflow-hidden rounded-lg border'>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className='w-8' aria-label='Status' />
              <TableHead>Edge box</TableHead>
              <TableHead>Cohort</TableHead>
              <TableHead>Site</TableHead>
              <TableHead>Current image</TableHead>
              <TableHead>Last result</TableHead>
              <TableHead>Last update</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {isLoading ? (
              <TableRow>
                <TableCell colSpan={7} className='py-8'>
                  <Skeleton className='h-4 w-full' />
                </TableCell>
              </TableRow>
            ) : error ? (
              <TableRow>
                <TableCell colSpan={7} className='py-8 text-center'>
                  <div className='flex items-center justify-center gap-2 text-destructive'>
                    <AlertTriangle className='h-4 w-4' />
                    {error instanceof Error ? error.message : "Failed to load convergence."}
                  </div>
                </TableCell>
              </TableRow>
            ) : rows.length === 0 ? (
              <TableRow>
                <TableCell colSpan={7} className='py-8 text-center text-muted-foreground text-sm'>
                  No edge boxes in this workspace.
                </TableCell>
              </TableRow>
            ) : (
              rows.map((r) => (
                <TableRow
                  key={r.edge_box_id}
                  data-testid={`convergence-row-${r.edge_box_id}`}
                  className={cn(r.convergence_status === "failed" && "bg-destructive/5")}
                >
                  <TableCell title={statusLabel(r.convergence_status)}>
                    {statusIcon(r.convergence_status)}
                  </TableCell>
                  <TableCell className='font-medium'>
                    {r.hardware_model}
                    {r.in_canary && (
                      <Badge variant='secondary' className='ml-2 text-[10px]'>
                        canary
                      </Badge>
                    )}
                  </TableCell>
                  <TableCell className='text-muted-foreground text-xs'>{r.cohort}</TableCell>
                  <TableCell className='text-muted-foreground text-xs'>{r.site_name}</TableCell>
                  <TableCell className='font-mono text-xs'>{r.current_image_tag ?? "—"}</TableCell>
                  <TableCell className='text-muted-foreground text-xs'>
                    {r.last_update_result ?? "—"}
                  </TableCell>
                  <TableCell className='text-muted-foreground text-xs'>
                    {formatWhen(r.last_update_at)}
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

export default RolloutDetailPage;
