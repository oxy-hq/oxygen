import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/shadcn/dialog";
import type { ProvisionMethod } from "../constants";

interface MethodDetailDialogProps {
  method: ProvisionMethod | null;
  onClose: () => void;
}

interface MethodDetail {
  title: string;
  body: React.ReactNode;
}

const DETAILS: Record<ProvisionMethod, MethodDetail> = {
  invoice: {
    title: "Provision via Invoice",
    body: (
      <div className='space-y-3 text-xs leading-relaxed'>
        <p>Create the subscription directly and email the invoice to the customer.</p>
        <ul className='list-disc space-y-1.5 pl-5'>
          <li>Stripe creates a draft invoice and auto-finalizes + emails it after about 1 hour.</li>
          <li>The customer opens the link in the email, enters a card, and pays.</li>
          <li>
            After the first successful payment, the system flips to{" "}
            <code className='bg-muted px-1 text-xs'>charge_automatically</code> for future cycles.
          </li>
          <li>
            No billing address is collected at this step — automatic tax only applies once the
            customer updates their address through the Customer Portal.
          </li>
          <li>
            Supports flexible billing intervals: prices in the same subscription may use different
            intervals (e.g. monthly seat + annual add-on).
          </li>
        </ul>
      </div>
    )
  },
  checkout: {
    title: "Provision via Checkout",
    body: (
      <div className='space-y-3 text-xs leading-relaxed'>
        <p>Create a Stripe Checkout Session and email the link for the customer to pay.</p>
        <ul className='list-disc space-y-1.5 pl-5'>
          <li>The customer opens the link and must enter billing address, tax ID, and a card.</li>
          <li>Automatic tax is calculated based on the address the customer provides.</li>
          <li>The card is saved as the default payment method; future cycles auto-charge.</li>
          <li>If a Checkout Session is already open, you can resend or cancel-and-recreate it.</li>
          <li>
            All prices in the subscription must share the same billing interval — Stripe Checkout
            rejects mixed intervals.
          </li>
        </ul>
      </div>
    )
  }
};

export function MethodDetailDialog({ method, onClose }: MethodDetailDialogProps) {
  if (!method) return null;
  const detail = DETAILS[method];
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-md`}>
        <DialogHeader>
          <DialogTitle>{detail.title}</DialogTitle>
        </DialogHeader>
        {detail.body}
      </DialogContent>
    </Dialog>
  );
}
