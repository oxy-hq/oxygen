import { ExternalLink, RefreshCw } from "lucide-react";
import { useState } from "react";
import { SubscriptionItemsList } from "@/components/billing/SubscriptionItemsList";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Separator } from "@/components/ui/shadcn/separator";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useAdminSubscription } from "@/hooks/api/billing";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type {
  AdminOrgRow,
  AdminSubscriptionDetail,
  LatestInvoiceSummary
} from "@/services/api/billing";
import ResyncConfirmDialog from "./ResyncConfirmDialog";

interface Props {
  org: AdminOrgRow;
  onClose: () => void;
}

export default function SubscriptionDetailDialog({ org, onClose }: Props) {
  const { data, isLoading, error, refetch } = useAdminSubscription(org.id);
  const [resyncOpen, setResyncOpen] = useState(false);

  return (
    <>
      <Dialog open onOpenChange={(open) => !open && onClose()}>
        <DialogContent className='max-w-xl'>
          <DialogHeader>
            <DialogTitle>Subscription — {org.name}</DialogTitle>
          </DialogHeader>

          {isLoading ? (
            <div className='flex items-center gap-2 text-muted-foreground text-xs'>
              <Spinner /> Loading subscription…
            </div>
          ) : error ? (
            <div className='text-destructive text-xs'>
              {error instanceof Error ? error.message : "Failed to load subscription."}
            </div>
          ) : data ? (
            <SubscriptionDetailBody detail={data} />
          ) : null}

          <DialogFooter>
            {data ? (
              <>
                <Button variant='outline' onClick={() => setResyncOpen(true)} className='gap-1'>
                  <RefreshCw className='h-3.5 w-3.5' />
                  Resync from Stripe
                </Button>
                <Button asChild variant='outline'>
                  <a
                    href={stripeDashboardUrl(data)}
                    target='_blank'
                    rel='noopener noreferrer'
                    className='gap-1'
                  >
                    Open in Stripe <ExternalLink className='h-3.5 w-3.5' />
                  </a>
                </Button>
              </>
            ) : null}
            <Button onClick={onClose}>Close</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ResyncConfirmDialog
        orgId={org.id}
        orgName={org.name}
        open={resyncOpen}
        onOpenChange={setResyncOpen}
        onSuccess={() => {
          refetch();
        }}
      />
    </>
  );
}

function SubscriptionDetailBody({ detail }: { detail: AdminSubscriptionDetail }) {
  return (
    <div className='space-y-4 text-xs'>
      <div className='flex flex-wrap items-center gap-2'>
        <Badge variant={detail.status === "active" ? "default" : "secondary"}>
          {detail.status}
        </Badge>
        {!detail.livemode ? <Badge variant='outline'>test mode</Badge> : null}
        {detail.cancel_at_period_end ? (
          <Badge variant='destructive'>cancels at period end</Badge>
        ) : null}
      </div>

      <DetailRow label='Subscription ID' value={<code className='text-xs'>{detail.id}</code>} />
      {detail.customer_id ? (
        <DetailRow
          label='Customer ID'
          value={<code className='text-xs'>{detail.customer_id}</code>}
        />
      ) : null}
      {detail.collection_method ? (
        <DetailRow label='Collection' value={detail.collection_method.replace(/_/g, " ")} />
      ) : null}
      <DetailRow
        label='Current period'
        value={`${formatUnix(detail.current_period_start)} → ${formatUnix(detail.current_period_end)}`}
      />
      {detail.created != null ? (
        <DetailRow label='Created' value={formatUnix(detail.created)} />
      ) : null}

      <Separator />

      <div className='space-y-2'>
        <div className='text-muted-foreground text-xs uppercase tracking-wide'>Items</div>
        <SubscriptionItemsList items={detail.items} showItemPeriods />
      </div>

      {detail.latest_invoice ? (
        <>
          <Separator />
          <LatestInvoiceSection
            invoice={detail.latest_invoice}
            collectionMethod={detail.collection_method}
            livemode={detail.livemode}
          />
        </>
      ) : null}
    </div>
  );
}

function LatestInvoiceSection({
  invoice,
  collectionMethod,
  livemode
}: {
  invoice: LatestInvoiceSummary;
  collectionMethod: string | null;
  livemode: boolean;
}) {
  // Subscriptions with `send_invoice` create the invoice in `draft` and wait
  // ~1 hour before auto-finalizing + emailing. Show that to the admin so they
  // don't think the email failed.
  const isDeferredDraft =
    invoice.status === "draft" &&
    (invoice.collection_method === "send_invoice" || collectionMethod === "send_invoice") &&
    invoice.auto_advance !== false;

  return (
    <div className='space-y-2'>
      <div className='text-muted-foreground text-xs uppercase tracking-wide'>Latest invoice</div>

      <div className='space-y-2 rounded-md border p-3'>
        <div className='flex flex-wrap items-center gap-2'>
          <Badge variant={invoiceStatusVariant(invoice.status)}>{invoice.status}</Badge>
          <code className='text-muted-foreground text-xs'>{invoice.id}</code>
        </div>

        <DetailRow label='Amount due' value={formatAmount(invoice.amount_due, invoice.currency)} />
        {invoice.amount_paid > 0 ? (
          <DetailRow
            label='Amount paid'
            value={formatAmount(invoice.amount_paid, invoice.currency)}
          />
        ) : null}
        {invoice.due_date != null ? (
          <DetailRow label='Due date' value={formatUnix(invoice.due_date)} />
        ) : null}
        {invoice.next_payment_attempt != null ? (
          <DetailRow
            label='Next payment attempt'
            value={formatUnix(invoice.next_payment_attempt)}
          />
        ) : null}

        {isDeferredDraft ? (
          <div
            className={cn(
              "rounded-md p-2 text-xs leading-relaxed ring-1 ring-inset",
              ADMIN_TONE.warn.bg,
              ADMIN_TONE.warn.text,
              ADMIN_TONE.warn.ring
            )}
          >
            Stripe holds new subscription invoices in <strong>draft</strong> for ~1 hour, then
            auto-finalizes and emails them to the customer. Open the invoice in Stripe if you want
            to finalize and send it now.
          </div>
        ) : null}

        <div className='flex flex-wrap gap-2 pt-1'>
          {invoice.hosted_invoice_url ? (
            <Button asChild variant='outline' size='sm'>
              <a
                href={invoice.hosted_invoice_url}
                target='_blank'
                rel='noopener noreferrer'
                className='gap-1'
              >
                Hosted invoice <ExternalLink className='h-3.5 w-3.5' />
              </a>
            </Button>
          ) : null}
          <Button asChild variant='outline' size='sm'>
            <a
              href={stripeInvoiceDashboardUrl(invoice.id, livemode)}
              target='_blank'
              rel='noopener noreferrer'
              className='gap-1'
            >
              Open in Stripe <ExternalLink className='h-3.5 w-3.5' />
            </a>
          </Button>
        </div>
      </div>
    </div>
  );
}

function invoiceStatusVariant(status: string): "default" | "secondary" | "destructive" | "outline" {
  switch (status) {
    case "paid":
      return "default";
    case "open":
      return "secondary";
    case "uncollectible":
    case "void":
      return "destructive";
    default:
      return "outline";
  }
}

function stripeInvoiceDashboardUrl(invoiceId: string, livemode: boolean): string {
  const prefix = livemode ? "https://dashboard.stripe.com" : "https://dashboard.stripe.com/test";
  return `${prefix}/invoices/${invoiceId}`;
}

function DetailRow({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className='flex items-baseline justify-between gap-4'>
      <span className='text-muted-foreground'>{label}</span>
      <span className='text-right'>{value}</span>
    </div>
  );
}

function formatAmount(unitAmountCents: number, currency: string) {
  return (unitAmountCents / 100).toLocaleString(undefined, {
    style: "currency",
    currency: currency.toUpperCase()
  });
}

function formatUnix(secs: number | null) {
  if (secs == null) return "—";
  return new Date(secs * 1000).toLocaleString();
}

function stripeDashboardUrl(detail: AdminSubscriptionDetail) {
  const prefix = detail.livemode
    ? "https://dashboard.stripe.com"
    : "https://dashboard.stripe.com/test";
  return `${prefix}/subscriptions/${detail.id}`;
}
