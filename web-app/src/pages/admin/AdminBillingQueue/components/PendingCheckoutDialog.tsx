import { ExternalLink } from "lucide-react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { useCancelPendingCheckout, useResendCheckoutEmail } from "@/hooks/api/billing";
import type {
  CheckoutAlreadyPendingError,
  ProvisionCheckoutResponse
} from "@/services/api/billing";

interface Props {
  orgId: string;
  info: CheckoutAlreadyPendingError;
  onClose: () => void;
  onRecreate: () => void;
}

export default function PendingCheckoutDialog({ orgId, info, onClose, onRecreate }: Props) {
  const resend = useResendCheckoutEmail(orgId);
  const cancel = useCancelPendingCheckout(orgId);

  const onResend = async () => {
    try {
      const res = await resend.mutateAsync();
      reportResendOutcome(res);
      onClose();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Resend failed");
    }
  };

  const onCancelAndRecreate = async () => {
    try {
      await cancel.mutateAsync();
      onRecreate();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Cancel failed");
    }
  };

  const onCopy = () => {
    void navigator.clipboard.writeText(info.url);
    toast.success("Link copied");
  };

  const busy = resend.isPending || cancel.isPending;
  const expiresHuman = new Date(info.expires_at * 1000).toLocaleString();

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-md`}>
        <DialogHeader>
          <DialogTitle>A Checkout link is already pending</DialogTitle>
        </DialogHeader>

        <div className='space-y-3 text-xs'>
          <p className='text-muted-foreground'>
            Resend the existing email, cancel and create a new link, or close this dialog.
          </p>
          <div className='space-y-1 border p-3'>
            <div className='text-muted-foreground text-xs uppercase tracking-wide'>Expires</div>
            <div>{expiresHuman}</div>
          </div>
          <div className='space-y-1 border p-3'>
            <div className='flex items-center justify-between gap-2'>
              <span className='text-muted-foreground text-xs uppercase tracking-wide'>
                Checkout link
              </span>
              <Button type='button' size='sm' variant='ghost' onClick={onCopy}>
                Copy
              </Button>
            </div>
            <a
              href={info.url}
              target='_blank'
              rel='noopener noreferrer'
              className='inline-flex items-center gap-1 break-all font-mono text-xs underline'
            >
              {info.url}
              <ExternalLink className='h-3 w-3 shrink-0' />
            </a>
          </div>
        </div>

        <DialogFooter className='flex-col gap-2 sm:flex-row'>
          <Button type='button' variant='outline' onClick={onClose} disabled={busy}>
            Close
          </Button>
          <Button type='button' variant='destructive' onClick={onCancelAndRecreate} disabled={busy}>
            {cancel.isPending ? "Cancelling…" : "Cancel & Recreate"}
          </Button>
          <Button type='button' onClick={onResend} disabled={busy}>
            {resend.isPending ? "Sending…" : "Resend email"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function reportResendOutcome(res: ProvisionCheckoutResponse) {
  if (res.email_skipped) {
    toast.warning("Email not sent — copy the link and share manually.", {
      description: res.email_skip_reason ?? undefined
    });
  } else {
    toast.success(`Checkout link re-sent to ${res.email_sent_to}`);
  }
}
