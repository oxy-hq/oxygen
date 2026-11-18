import { CalendarClock, Coins, Database, Repeat, TrendingUp } from "lucide-react";
import type { CostBucket, OrgCost } from "@/services/api/adminObservability";
import { AdminSectionLabel } from "../../components/AdminSectionLabel";
import { AdminTrendBars } from "../../components/AdminTrendBars";
import { alignWindow } from "../dailySeries";
import { compact, pct, shortModel, surfaceLabel, usd, versusPrior, versusShort } from "../format";
import { BreakdownBars, type BreakdownItem } from "./BreakdownBars";

/**
 * What this org costs us, and why.
 *
 * The headline is the window's priced spend; everything beside and below it
 * exists because a total on its own cannot be acted on. The four breakdowns
 * are the dimensions already stamped on `agentic_runs` and read by nothing
 * until now — model, workspace, surface, and whether a person was there.
 *
 * **There are no infrastructure dollars here on purpose.** Storage, egress,
 * function compute and OLTP are nearly all shared across every org, and
 * `customer-fleet-scaling.md` §6 put the whole platform at ~$4.4k/mo against
 * one $80k/yr contract. Splitting a shared bill by egress bytes would invent
 * precision on a line that is 3–6% of revenue, and would read as authoritative.
 * The per-org app meters that *are* real appear on the Performance panel as
 * quantities.
 */
export const CostPanel = ({ cost }: { cost: OrgCost }) => {
  const { total, derived } = cost;
  const series = alignWindow(cost.by_day, cost.prior_by_day, cost.window_days);
  const versus = versusPrior(total.cost_usd, cost.prior_cost_usd);

  return (
    <section className='space-y-5' data-testid='admin-observability-cost'>
      <AdminSectionLabel trailing={`last ${cost.window_days}d`}>LLM spend</AdminSectionLabel>

      <div className='grid gap-4 lg:grid-cols-3'>
        <div className='space-y-4 rounded-lg border border-border/60 bg-card p-4 lg:col-span-2'>
          <div className='flex flex-wrap items-end justify-between gap-4'>
            <div>
              <span className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
                Priced spend
              </span>
              <div
                className='font-semibold text-2xl tabular-nums tracking-tight'
                data-testid='admin-observability-cost-total'
              >
                {usd(total.cost_usd)}
              </div>
            </div>
            <div className='flex flex-wrap gap-5 text-right'>
              {versus != null ? (
                <Figure
                  icon={TrendingUp}
                  label='vs prior'
                  value={versusShort(versus)}
                  hint={`Priced spend over the ${cost.window_days} days before: ${usd(cost.prior_cost_usd)}`}
                />
              ) : null}
              <Figure
                icon={CalendarClock}
                label='Run-rate / mo'
                value={usd(derived.monthly_run_rate_usd)}
              />
              <Figure icon={Coins} label='$ / run' value={usd(derived.cost_per_run_usd)} />
              <Figure
                icon={Repeat}
                label='Unattended'
                value={pct(derived.unattended_share)}
                hint='Share of priced cost from runs a schedule started'
              />
              <Figure
                icon={Database}
                label='Cache hit'
                value={pct(derived.cache_hit_share)}
                hint='Share of input tokens served from cache'
              />
            </div>
          </div>

          <AdminTrendBars
            testId='admin-observability-cost-trend'
            emptyLabel='No LLM activity in this window.'
            baselineLabel={`same day, ${cost.window_days}d earlier`}
            points={
              cost.by_day.length === 0
                ? []
                : series.map((d) => ({
                    day: d.day,
                    value: d.cost_usd,
                    baseline: d.prior_cost_usd ?? undefined,
                    detail: `${usd(d.cost_usd)} · ${d.run_count.toLocaleString()} run${d.run_count === 1 ? "" : "s"}${d.prior_cost_usd != null ? ` · ${usd(d.prior_cost_usd)} ${cost.window_days}d earlier` : ""}`
                  }))
            }
          />

          <dl className='grid grid-cols-2 gap-3 text-xs sm:grid-cols-4'>
            <Token label='Runs' value={compact(total.run_count)} />
            <Token label='Tokens in' value={compact(total.input_tokens)} />
            <Token label='Tokens out' value={compact(total.output_tokens)} />
            <Token label='Cache read' value={compact(total.cache_read_tokens)} />
          </dl>

          {total.unpriced_runs > 0 ? (
            // Named rather than folded into the total: the dollar figure above
            // excludes these runs, and an operator comparing windows needs to
            // know when part of one could not be priced.
            <p
              className='text-[10px] text-muted-foreground'
              data-testid='admin-observability-cost-unpriced'
            >
              {total.unpriced_runs.toLocaleString()} run
              {total.unpriced_runs === 1 ? "" : "s"} used a model with no known price — tokens
              counted, dollars excluded.
            </p>
          ) : null}
        </div>

        <div className='space-y-5 rounded-lg border border-border/60 bg-card p-4'>
          <BreakdownBars
            testId='admin-observability-cost-by-model'
            title='By model'
            emptyLabel='No usage.'
            items={toItems(cost.by_model, (b) => shortModel(b.label))}
          />
          <BreakdownBars
            testId='admin-observability-cost-by-attendance'
            title='Attended vs scheduled'
            emptyLabel='No usage.'
            items={toItems(cost.by_attendance, (b) =>
              b.key === "scheduled" ? "Started by a schedule" : "Started by a person"
            )}
          />
        </div>

        <div className='rounded-lg border border-border/60 bg-card p-4'>
          <BreakdownBars
            testId='admin-observability-cost-by-surface'
            title='By surface'
            emptyLabel='No usage.'
            items={toItems(cost.by_surface, (b) => surfaceLabel(b.key))}
          />
        </div>

        <div className='rounded-lg border border-border/60 bg-card p-4 lg:col-span-2'>
          <BreakdownBars
            testId='admin-observability-cost-by-workspace'
            title='By workspace'
            emptyLabel='No usage.'
            items={toItems(cost.by_workspace, (b) => b.label)}
          />
        </div>
      </div>
    </section>
  );
};

/** The API's bucket shape, relabelled for the screen. */
function toItems(buckets: CostBucket[], label: (bucket: CostBucket) => string): BreakdownItem[] {
  return buckets.map((b) => ({
    key: b.key,
    label: label(b),
    cost_usd: b.cost_usd,
    run_count: b.run_count,
    share: b.share
  }));
}

const Figure = ({
  icon: Icon,
  label,
  value,
  hint
}: {
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  value: string;
  hint?: string;
}) => (
  <div title={hint}>
    <span className='flex items-center justify-end gap-1 font-medium text-[10px] text-muted-foreground uppercase tracking-[0.12em]'>
      <Icon className='size-3' />
      {label}
    </span>
    <span className='font-medium text-sm tabular-nums'>{value}</span>
  </div>
);

const Token = ({ label, value }: { label: string; value: string }) => (
  <div className='space-y-0.5'>
    <dt className='text-muted-foreground'>{label}</dt>
    <dd className='font-medium tabular-nums'>{value}</dd>
  </div>
);
