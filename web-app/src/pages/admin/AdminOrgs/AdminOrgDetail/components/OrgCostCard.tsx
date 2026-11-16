import { ArrowUpRight, DollarSign } from "lucide-react";
import { Link } from "react-router-dom";
import ROUTES from "@/libs/utils/routes";
import type { OrgUsageDetail } from "@/services/api/adminMetrics";
import { AdminSectionLabel } from "../../../components/AdminSectionLabel";
import { AdminTrendBars } from "../../../components/AdminTrendBars";
import { compactInt, usd } from "../format";

/**
 * The org-360's cost summary: the headline figure and the daily trend.
 *
 * Deliberately shallow. Why the number is what it is — by model, by workspace,
 * by surface, and whether a person was there — belongs to Admin →
 * Observability, which is what the link goes to. Two surfaces answering the
 * same question at two depths is fine; two answering it in two *ways* is how
 * they end up disagreeing, so this one does not break the total down at all.
 */
export const OrgCostCard = ({
  orgId,
  usage,
  days,
  isLoading
}: {
  orgId: string;
  usage: OrgUsageDetail | null | undefined;
  days: number;
  isLoading: boolean;
}) => (
  <section className='space-y-3' data-testid='admin-org-cost'>
    <AdminSectionLabel
      trailing={
        <Link
          to={`${ROUTES.ADMIN.OBSERVABILITY_ORG(orgId)}?days=${days}`}
          className='inline-flex items-center gap-1 underline underline-offset-2 hover:text-foreground'
          data-testid='admin-org-cost-observability-link'
        >
          Cost &amp; performance
          <ArrowUpRight className='size-3' />
        </Link>
      }
    >
      LLM cost · last {days}d
    </AdminSectionLabel>
    <div className='rounded-lg border border-border/60 bg-card p-5'>
      {isLoading ? (
        <div className='h-28 animate-pulse rounded bg-muted/40' />
      ) : !usage || usage.total.run_count === 0 ? (
        <p className='text-muted-foreground text-xs'>No LLM activity in this window.</p>
      ) : (
        <div className='space-y-4'>
          <div className='flex items-baseline gap-2'>
            <DollarSign className='size-4 text-muted-foreground' />
            <span className='font-semibold text-2xl tabular-nums tracking-tight'>
              {usd(usage.total.cost_usd)}
            </span>
          </div>
          <AdminTrendBars
            testId='admin-org-cost-trend'
            emptyLabel='No LLM activity in this window.'
            points={usage.by_day.map((d) => ({
              day: d.day,
              value: d.cost_usd,
              detail: `${usd(d.cost_usd)} · ${d.run_count} run${d.run_count === 1 ? "" : "s"}`
            }))}
          />
          <dl className='grid grid-cols-3 gap-2 text-xs'>
            <div className='space-y-0.5'>
              <dt className='text-muted-foreground'>Runs</dt>
              <dd className='font-medium tabular-nums'>{compactInt(usage.total.run_count)}</dd>
            </div>
            <div className='space-y-0.5'>
              <dt className='text-muted-foreground'>Input tok</dt>
              <dd className='font-medium tabular-nums'>{compactInt(usage.total.input_tokens)}</dd>
            </div>
            <div className='space-y-0.5'>
              <dt className='text-muted-foreground'>Output tok</dt>
              <dd className='font-medium tabular-nums'>{compactInt(usage.total.output_tokens)}</dd>
            </div>
          </dl>
        </div>
      )}
    </div>
  </section>
);
