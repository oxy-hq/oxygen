import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { apiErrorMessage, apiStatus } from "@/libs/apiError";
import type { BoundKioskDevice } from "@/types/frontline";

interface Props {
  device: BoundKioskDevice;
  onConfirm: () => void;
  onCancel: () => void;
  isLeaving: boolean;
  error: unknown;
}

/**
 * The in-page confirmation for "Leave kiosk mode" — not `window.confirm`,
 * whose one line of text cannot say what leaving does to the store, and whose
 * failure has nowhere to be shown.
 */
export default function LeaveKioskConfirm({
  device,
  onConfirm,
  onCancel,
  isLeaving,
  error
}: Props) {
  return (
    <section
      className='flex flex-col gap-3 rounded-md border border-destructive/40 p-4'
      aria-labelledby='kiosk-leave-title'
      data-testid='kiosk-manage-leave-panel'
    >
      <h2 id='kiosk-leave-title' className='font-medium'>
        Leave kiosk mode on this browser?
      </h2>
      <p className='text-muted-foreground text-sm'>
        {device.device} stops being {device.orgName}&apos;s kiosk: crew can&apos;t sign in here any
        more, and this browser&apos;s login page goes back to asking for an account. The kiosk is
        revoked, not moved — to put a tablet here again, create a new kiosk in Settings → Crew.
      </p>
      {error !== null && error !== undefined && (
        <p className='text-destructive text-sm' role='alert' data-testid='kiosk-manage-leave-error'>
          {leaveFailure(error, device)}
        </p>
      )}
      <div className='flex justify-end gap-2'>
        <Button
          variant='outline'
          onClick={onCancel}
          disabled={isLeaving}
          data-testid='kiosk-manage-leave-cancel'
        >
          Cancel
        </Button>
        <Button
          variant='destructive'
          onClick={onConfirm}
          disabled={isLeaving}
          data-testid='kiosk-manage-leave-confirm'
        >
          {isLeaving && <Loader2 className='h-4 w-4 animate-spin' />}
          Leave kiosk mode
        </Button>
      </div>
    </section>
  );
}

/** What a failed leave means, in the words of the person holding the tablet. */
function leaveFailure(error: unknown, device: BoundKioskDevice): string {
  switch (apiStatus(error)) {
    case 404:
      return `This browser isn't a kiosk of ${device.orgName} any more — it may already have been revoked. Reload to check.`;
    case 403:
      return `Only an owner or admin of ${device.orgName} can take this browser out of kiosk mode.`;
    // The server could not look the kiosk up. Nothing was revoked, so the
    // tablet is exactly as it was — not "already revoked", which a 404 means.
    case 503:
      return "Oxygen couldn't check this tablet just now, so it's still a kiosk. Try again in a moment.";
    default:
      return apiErrorMessage(error, "Couldn't leave kiosk mode. Try again in a moment.");
  }
}
