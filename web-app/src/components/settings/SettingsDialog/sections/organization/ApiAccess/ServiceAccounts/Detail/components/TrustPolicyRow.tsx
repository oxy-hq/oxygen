import { Ban, FileCode2, MoreHorizontal, Pencil, Play, Trash2, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { useDeleteTrustPolicy, useUpdateTrustPolicy } from "@/hooks/api/orgApiAccess";
import { cn } from "@/libs/shadcn/utils";
import type { TrustPolicy } from "@/types/orgApiAccess";
import { AccessSummary } from "../../../shared/AccessSummary";
import { ConfirmDialog } from "../../../shared/ConfirmDialog";
import { describeApiError } from "../../../utils/errors";
import { describeAccess } from "../../../utils/grants";
import { lastUsedText } from "../../../utils/tokens";

interface TrustPolicyRowProps {
  orgId: string;
  saId: string;
  policy: TrustPolicy;
  onEdit: (policy: TrustPolicy) => void;
  onShowWorkflow: (policy: TrustPolicy) => void;
}

const CELL = "px-3 py-2.5 align-top max-md:px-0 max-md:py-0";

export function TrustPolicyRow({
  orgId,
  saId,
  policy,
  onEdit,
  onShowWorkflow
}: TrustPolicyRowProps) {
  const update = useUpdateTrustPolicy();
  const remove = useDeleteTrustPolicy();
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const disabled = policy.disabled_at !== null;
  const ref = { orgId, saId, policyId: policy.id };

  const setDisabled = async (next: boolean) => {
    try {
      await update.mutateAsync({ ...ref, request: { disabled: next } });
      toast.success(next ? "Disabled the policy" : "Enabled the policy");
    } catch (err) {
      toast.error(describeApiError(err, `Couldn't ${next ? "disable" : "enable"} the policy.`));
    }
  };

  const handleDelete = async () => {
    try {
      await remove.mutateAsync(ref);
      toast.success("Deleted the policy");
      setConfirmingDelete(false);
    } catch (err) {
      toast.error(describeApiError(err, "Couldn't delete the policy."));
    }
  };

  return (
    <TableRow
      className={cn(disabled && "text-muted-foreground")}
      data-testid='api-access-policy-row'
      data-repository={policy.repository}
    >
      <TableCell data-label='Repository' className={cn(CELL, "whitespace-normal")}>
        <p className='break-all font-medium font-mono text-foreground'>{policy.repository}</p>
        <p className='break-all font-mono text-muted-foreground'>{policy.workflow_path}</p>
        {policy.allow_self_hosted && (
          <p className='mt-0.5 text-muted-foreground'>Self-hosted runners allowed</p>
        )}
      </TableCell>
      <TableCell data-label='Environment' className={CELL}>
        {policy.environment ? (
          <span className='font-mono'>{policy.environment}</span>
        ) : (
          <span
            className='inline-flex items-center gap-1'
            title='Anyone who can push to the repository can run this workflow and get a token.'
            data-testid='api-access-policy-no-environment'
          >
            <TriangleAlert className='size-3 text-warning' aria-hidden />
            None
          </span>
        )}
      </TableCell>
      <TableCell data-label='Ref' className={CELL}>
        {policy.ref_pattern ? (
          <span className='font-mono'>{policy.ref_pattern}</span>
        ) : (
          <span className='text-muted-foreground'>Any</span>
        )}
      </TableCell>
      <TableCell data-label='Access' className={cn(CELL, "whitespace-normal")}>
        <AccessSummary access={describeAccess(policy.grants)} />
      </TableCell>
      <TableCell data-label='Last used' className={CELL}>
        {lastUsedText(policy.last_used_at)}
      </TableCell>
      <TableCell data-label='Status' className={CELL}>
        {disabled ? (
          <Badge variant='outline' className='text-muted-foreground'>
            Disabled
          </Badge>
        ) : (
          <Badge variant='outline' className='border-primary/30 bg-primary/5 text-primary'>
            Enabled
          </Badge>
        )}
      </TableCell>
      <TableCell className={cn(CELL, "text-right")}>
        {/* Non-modal, so a dialog opened from an item never inherits a stuck `pointer-events: none`. */}
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              variant='ghost'
              size='icon'
              className='size-7 text-muted-foreground'
              aria-label={`Actions for the ${policy.repository} policy`}
              data-testid='api-access-policy-actions'
            >
              <MoreHorizontal className='size-4' aria-hidden />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align='end'>
            <DropdownMenuItem
              className='text-xs'
              onSelect={() => onShowWorkflow(policy)}
              data-testid='api-access-policy-show-workflow'
            >
              <FileCode2 aria-hidden />
              Show workflow
            </DropdownMenuItem>
            <DropdownMenuItem
              className='text-xs'
              onSelect={() => onEdit(policy)}
              data-testid='api-access-policy-edit'
            >
              <Pencil aria-hidden />
              Edit
            </DropdownMenuItem>
            <DropdownMenuItem
              className='text-xs'
              onSelect={() => setDisabled(!disabled)}
              data-testid={disabled ? "api-access-policy-enable" : "api-access-policy-disable"}
            >
              {disabled ? <Play aria-hidden /> : <Ban aria-hidden />}
              {disabled ? "Enable" : "Disable"}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              variant='destructive'
              className='text-xs'
              onSelect={() => setConfirmingDelete(true)}
              data-testid='api-access-policy-delete'
            >
              <Trash2 aria-hidden />
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>

        <ConfirmDialog
          open={confirmingDelete}
          onOpenChange={setConfirmingDelete}
          title='Delete this policy?'
          confirmLabel='Delete'
          cancelLabel='Keep policy'
          destructive
          isPending={remove.isPending}
          onConfirm={handleDelete}
          testId='api-access-policy-delete-dialog'
        >
          <p>
            Runs of <code className='font-mono text-foreground'>{policy.workflow_path}</code> in{" "}
            <code className='font-mono text-foreground'>{policy.repository}</code> can no longer act
            as this account. A run already in progress keeps its token until it expires, at most 15
            minutes.
          </p>
          <p>To pause it instead, disable the policy.</p>
        </ConfirmDialog>
      </TableCell>
    </TableRow>
  );
}
