import { Ban, MoreHorizontal, Pencil, Play, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { useDeleteServiceAccount, useUpdateServiceAccount } from "@/hooks/api/orgApiAccess";
import type { ServiceAccount } from "@/types/orgApiAccess";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { describeApiError } from "../../utils/errors";
import { casualties } from "../../utils/serviceAccounts";
import { ServiceAccountDialog } from "./ServiceAccountDialog";

interface ServiceAccountActionsProps {
  orgId: string;
  orgSlug: string;
  account: ServiceAccount;
  takenNames: string[];
  /** Called after the account is gone, so a detail view can step back to the list. */
  onDeleted?: () => void;
}

/**
 * Edit, disable or enable, and delete — the same menu on a list row and on the
 * account's own page, so an action is named and behaves the same in both.
 */
export function ServiceAccountActions({
  orgId,
  orgSlug,
  account,
  takenNames,
  onDeleted
}: ServiceAccountActionsProps) {
  const update = useUpdateServiceAccount();
  const remove = useDeleteServiceAccount();
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState<"disable" | "delete" | null>(null);
  const disabled = account.disabled_at !== null;
  const dies = casualties(account);

  const setDisabled = async (next: boolean) => {
    try {
      await update.mutateAsync({ orgId, saId: account.id, request: { disabled: next } });
      toast.success(next ? `Disabled ${account.name}` : `Enabled ${account.name}`);
      setConfirming(null);
    } catch (err) {
      toast.error(
        describeApiError(err, `Couldn't ${next ? "disable" : "enable"} ${account.name}.`)
      );
    }
  };

  const handleDelete = async () => {
    try {
      await remove.mutateAsync({ orgId, saId: account.id });
      toast.success(`Deleted ${account.name}`);
      setConfirming(null);
      onDeleted?.();
    } catch (err) {
      toast.error(describeApiError(err, `Couldn't delete ${account.name}.`));
    }
  };

  return (
    <>
      {/* Non-modal, so a dialog opened from an item never inherits a stuck `pointer-events: none`. */}
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            variant='ghost'
            size='icon'
            className='size-7 text-muted-foreground'
            aria-label={`Actions for ${account.name}`}
            data-testid='api-access-account-actions'
          >
            <MoreHorizontal className='size-4' aria-hidden />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align='end' className='text-xs'>
          <DropdownMenuItem
            className='text-xs'
            onSelect={() => setEditing(true)}
            data-testid='api-access-account-edit'
          >
            <Pencil aria-hidden />
            Edit
          </DropdownMenuItem>
          {disabled ? (
            <DropdownMenuItem
              className='text-xs'
              onSelect={() => setDisabled(false)}
              data-testid='api-access-account-enable'
            >
              <Play aria-hidden />
              Enable
            </DropdownMenuItem>
          ) : (
            <DropdownMenuItem
              className='text-xs'
              onSelect={() => setConfirming("disable")}
              data-testid='api-access-account-disable'
            >
              <Ban aria-hidden />
              Disable
            </DropdownMenuItem>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            variant='destructive'
            className='text-xs'
            onSelect={() => setConfirming("delete")}
            data-testid='api-access-account-delete'
          >
            <Trash2 aria-hidden />
            Delete
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <ServiceAccountDialog
        open={editing}
        onOpenChange={setEditing}
        orgId={orgId}
        orgSlug={orgSlug}
        account={account}
        takenNames={takenNames}
      />

      <ConfirmDialog
        open={confirming === "disable"}
        onOpenChange={(open) => !open && setConfirming(null)}
        title={`Disable ${account.name}?`}
        confirmLabel='Disable'
        cancelLabel='Keep it enabled'
        isPending={update.isPending}
        onConfirm={() => setDisabled(true)}
        testId='api-access-account-disable-dialog'
      >
        <p>
          Everything that signs in as this account stops working at once
          {dies ? `: ${dies}` : ""}. Nothing is deleted, and enabling the account brings it all back
          as it was.
        </p>
      </ConfirmDialog>

      <ConfirmDialog
        open={confirming === "delete"}
        onOpenChange={(open) => !open && setConfirming(null)}
        title={`Delete ${account.name}?`}
        confirmLabel='Delete'
        cancelLabel='Keep account'
        destructive
        isPending={remove.isPending}
        onConfirm={handleDelete}
        testId='api-access-account-delete-dialog'
      >
        <p data-testid='api-access-account-delete-impact'>
          {dies
            ? `This also ends ${dies}: the tokens are revoked and the policies stop matching. Anything still using them fails from that moment.`
            : "It has no tokens and no trusted-access policies, so nothing else stops working."}
        </p>
        <p>
          This can't be undone. To pause the account instead, disable it. What it already did stays
          in the audit log under its name.
        </p>
      </ConfirmDialog>
    </>
  );
}
