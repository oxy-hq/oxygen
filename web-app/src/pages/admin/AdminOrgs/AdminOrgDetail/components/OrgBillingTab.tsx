import { ExternalLink, Receipt } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAdminSubscription } from "@/hooks/api/billing/useAdminSubscription";
import ROUTES from "@/libs/utils/routes";
import { AdminEmptyState } from "../../../components/AdminEmptyState";
import { AdminSectionLabel } from "../../../components/AdminSectionLabel";
import { AdminStatusPill } from "../../../components/AdminStatusPill";

const STATUS_TONE: Record<string, "ok" | "warn" | "danger" | "muted"> = {
  active: "ok",
  trialing: "ok",
  past_due: "danger",
  unpaid: "danger",
  incomplete: "warn",
  incomplete_expired: "muted",
  canceled: "muted"
};

const fmtDate = (epoch: number | null | undefined): string =>
  epoch
    ? new Date(epoch * 1000).toLocaleDateString(undefined, {
        year: "numeric",
        month: "short",
        day: "numeric"
      })
    : "—";

const money = (amount: number, currency: string): string =>
  new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: currency.toUpperCase()
  }).format(amount / 100);

/**
 * Owner-only billing summary inside the org 360. Read-only — surfaces the
 * subscription state an operator needs while investigating a tenant (status,
 * seats, period, latest invoice) and links out to the Billing Queue for the
 * mutating flows (provision / resync), which stay owner-gated there.
 */
export const OrgBillingTab = ({ orgId }: { orgId: string }) => {
  const { data: sub, isLoading, isError } = useAdminSubscription(orgId);

  if (isLoading) {
    return <Skeleton className='h-48 w-full' />;
  }

  if (isError || !sub) {
    return (
      <div className='space-y-4'>
        <AdminEmptyState
          size='inline'
          icon={Receipt}
          title='No subscription on file'
          description='This organization has no Stripe subscription, or billing details are unavailable.'
        />
        <Button asChild variant='outline' size='sm'>
          <Link to={ROUTES.ADMIN.BILLING_QUEUE}>
            Open Billing queue
            <ExternalLink className='size-3.5' />
          </Link>
        </Button>
      </div>
    );
  }

  const seats = sub.items.reduce((sum, i) => sum + (i.quantity ?? 0), 0);
  const tone = STATUS_TONE[sub.status] ?? "muted";

  return (
    <div className='space-y-6'>
      <section className='space-y-4 rounded-lg border border-border/60 bg-card p-6'>
        <div className='flex items-center justify-between'>
          <div className='flex items-center gap-3'>
            <h3 className='font-semibold text-sm'>Subscription</h3>
            <AdminStatusPill tone={tone} label={sub.status.replace(/_/g, " ")} />
            {sub.livemode ? null : <AdminStatusPill tone='warn' label='test mode' />}
          </div>
          <Button asChild variant='outline' size='sm'>
            <Link to={ROUTES.ADMIN.BILLING_QUEUE}>
              Manage
              <ExternalLink className='size-3.5' />
            </Link>
          </Button>
        </div>

        <dl className='grid gap-4 text-xs sm:grid-cols-3'>
          <div className='space-y-0.5'>
            <dt className='text-muted-foreground text-xs'>Seats</dt>
            <dd className='font-medium tabular-nums'>{seats.toLocaleString()}</dd>
          </div>
          <div className='space-y-0.5'>
            <dt className='text-muted-foreground text-xs'>Current period</dt>
            <dd className='font-medium'>
              {fmtDate(sub.current_period_start)} → {fmtDate(sub.current_period_end)}
            </dd>
          </div>
          <div className='space-y-0.5'>
            <dt className='text-muted-foreground text-xs'>Renews</dt>
            <dd className='font-medium'>
              {sub.cancel_at_period_end ? "Cancels at period end" : "Auto-renews"}
            </dd>
          </div>
        </dl>
      </section>

      <section className='space-y-3'>
        <AdminSectionLabel
          trailing={`${sub.items.length} item${sub.items.length === 1 ? "" : "s"}`}
        >
          Line items
        </AdminSectionLabel>
        <ul className='divide-y divide-border/60 overflow-hidden rounded-md border border-border/60 bg-card'>
          {sub.items.map((item) => (
            <li key={item.id} className='flex items-center justify-between gap-3 px-4 py-3 text-xs'>
              <div className='min-w-0'>
                <p className='truncate font-medium'>
                  {item.product_name ?? item.price_nickname ?? item.price_id}
                </p>
                <p className='text-muted-foreground text-xs tabular-nums'>×{item.quantity}</p>
              </div>
              <span className='shrink-0 tabular-nums'>{item.amount_display}</span>
            </li>
          ))}
        </ul>
      </section>

      {sub.latest_invoice ? (
        <section className='space-y-3'>
          <AdminSectionLabel>Latest invoice</AdminSectionLabel>
          <div className='flex items-center justify-between gap-3 rounded-md border border-border/60 bg-card px-4 py-3 text-xs'>
            <div className='flex items-center gap-3'>
              <AdminStatusPill
                tone={sub.latest_invoice.status === "paid" ? "ok" : "warn"}
                label={sub.latest_invoice.status}
              />
              <span className='tabular-nums'>
                {money(sub.latest_invoice.amount_due, sub.latest_invoice.currency)} due ·{" "}
                {money(sub.latest_invoice.amount_paid, sub.latest_invoice.currency)} paid
              </span>
            </div>
            {sub.latest_invoice.hosted_invoice_url ? (
              <Button asChild variant='ghost' size='sm' className='h-7'>
                <a href={sub.latest_invoice.hosted_invoice_url} target='_blank' rel='noreferrer'>
                  View
                  <ExternalLink className='size-3.5' />
                </a>
              </Button>
            ) : null}
          </div>
        </section>
      ) : null}
    </div>
  );
};
