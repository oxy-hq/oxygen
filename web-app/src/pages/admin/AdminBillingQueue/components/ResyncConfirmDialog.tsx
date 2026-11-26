import { isAxiosError } from "axios";
import { useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
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
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Label } from "@/components/ui/shadcn/label";
import { useResyncAdminSubscription } from "@/hooks/api/billing";

interface Props {
  orgId: string;
  orgName: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSuccess?: () => void;
}

export default function ResyncConfirmDialog({
  orgId,
  orgName,
  open,
  onOpenChange,
  onSuccess
}: Props) {
  const [syncSeats, setSyncSeats] = useState(true);
  const resync = useResyncAdminSubscription(orgId);

  const handleConfirm = async (e: React.MouseEvent) => {
    e.preventDefault();
    try {
      await resync.mutateAsync({ sync_seats: syncSeats });
      toast.success("Subscription resynced from Stripe");
      onSuccess?.();
      onOpenChange(false);
    } catch (err) {
      const message = isAxiosError(err)
        ? (err.response?.data?.message ?? err.message)
        : err instanceof Error
          ? err.message
          : "Resync failed";
      toast.error(message);
    }
  };

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className={DESIGN_REFERENCE}>
        <AlertDialogHeader>
          <AlertDialogTitle>Resync billing — {orgName}</AlertDialogTitle>
          <AlertDialogDescription>
            Re-fetches the subscription from Stripe and mirrors its state into the local database.
            Useful when a webhook was missed.
          </AlertDialogDescription>
        </AlertDialogHeader>

        <div className='flex items-start gap-3 border p-3'>
          <Checkbox
            id='sync-seats'
            checked={syncSeats}
            onCheckedChange={(v) => setSyncSeats(v === true)}
            disabled={resync.isPending}
            className='mt-0.5'
          />
          <div className='space-y-1'>
            <Label htmlFor='sync-seats' className='font-medium text-xs'>
              Also push seat count to Stripe
            </Label>
            <p className='text-muted-foreground text-xs'>
              Sends the current member count up to Stripe before mirroring back. Leave unchecked for
              a pull-only resync.
            </p>
          </div>
        </div>

        <AlertDialogFooter>
          <AlertDialogCancel disabled={resync.isPending}>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={handleConfirm} disabled={resync.isPending}>
            {resync.isPending ? "Resyncing…" : "Resync"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
