import { cn } from "@/libs/shadcn/utils";
import type { OrgApps, OrgResources } from "@/services/api/adminObservability";
import { AdminSectionLabel } from "../../components/AdminSectionLabel";
import { compact, ms, pct } from "../format";

/**
 * Where this org's compute goes.
 *
 * ## Why two rows and not one stacked bar
 *
 * A stacked bar needs two hues to separate its segments, and this console's
 * rule is that **colour means something is wrong** — a categorical pair here
 * would spend the surface's only alarm signal on a neutral composition. The
 * obvious pair (`--chart-1`/`--chart-2`) also failed the dark-mode checks:
 * contrast 2.55:1 against the dark card, and a green outside the dark
 * lightness band.
 *
 * So the composition is encoded by **length and position** instead: two rows,
 * one shared baseline, one hue at two steps. Lightness-only separation is
 * strictly more robust for colour-vision deficiency than any hue pair, and the
 * row headings carry identity, so nothing here is colour-alone.
 *
 * ## Why this split
 *
 * `wall − init` is the tenant's own code and `init` is start-up we impose.
 * That is the one division that says whose problem a compute bill is, and
 * `internal-docs/platform-metrics.md` names it where the metric is defined.
 *
 * ## What the number is not
 *
 * GB·s here is **provisioned**, not consumed: wall time at the isolate
 * ceiling. An invocation parked on a host call bills what one spinning does,
 * because there is no CPU metering to use instead. The panel says so rather
 * than letting a GB·s figure read as measured CPU.
 */
export const ComputePanel = ({
  apps,
  resources
}: {
  apps: OrgApps;
  resources: OrgResources | undefined;
}) => {
  if (!apps.measured || apps.by_day.length === 0) return null;

  const days = apps.by_day;
  // One baseline for both rows: the busiest day's total wall time. Scaling
  // each row to its own maximum would make a tiny start-up share look like a
  // large one, which is precisely the comparison this panel exists to make.
  const peak = Math.max(...days.map((d) => d.wall_ms), 1);

  const totalWall = apps.total.wall_ms;
  const totalInit = Math.min(apps.total.init_ms, totalWall);
  const ownCode = Math.max(totalWall - totalInit, 0);
  const initShare = totalWall > 0 ? totalInit / totalWall : null;

  const gbs = resources?.meters.find((m) => m.key === "function_compute")?.quantity ?? null;
  const ceiling = resources?.basis.provisioned_memory_mb ?? null;

  return (
    <section className='space-y-4' data-testid='admin-observability-compute'>
      <AdminSectionLabel trailing={`last ${apps.window_days}d`}>Compute</AdminSectionLabel>

      <div className='space-y-5 border border-border/60 bg-card p-4'>
        <div className='flex flex-wrap items-end justify-between gap-4'>
          <div>
            <span className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
              Provisioned compute
            </span>
            <div
              className='font-semibold text-2xl tabular-nums tracking-tight'
              data-testid='admin-observability-compute-total'
            >
              {gbs === null ? "—" : `${compact(gbs)} GB·s`}
            </div>
          </div>
          <dl className='flex flex-wrap gap-5 text-right'>
            <Figure label='Wall time' value={ms(totalWall)} />
            <Figure
              label='Start-up'
              value={pct(initShare, 1)}
              hint='Share of wall time spent starting an isolate rather than running app code'
            />
            <Figure
              label='Host calls'
              value={compact(apps.total.host_calls)}
              hint='ctx.* calls — work the app spent waiting on us, which a wall-clock meter still bills'
            />
          </dl>
        </div>

        {/* The composition, directly labelled on both parts. A 2px gap
            separates the segments so they never read as one bar. */}
        <div className='space-y-1.5' data-testid='admin-observability-compute-split'>
          <div className='flex h-2 w-full gap-0.5 overflow-hidden rounded-full'>
            <div
              className='h-full rounded-l-full bg-primary/30'
              style={{ width: `${((totalInit / Math.max(totalWall, 1)) * 100).toFixed(2)}%` }}
            />
            <div
              className='h-full flex-1 rounded-r-full bg-primary/80'
              style={{ width: `${((ownCode / Math.max(totalWall, 1)) * 100).toFixed(2)}%` }}
            />
          </div>
          <div className='flex flex-wrap justify-between gap-2 text-[11px]'>
            <span className='flex items-center gap-1.5'>
              <span className='inline-block size-2 rounded-full bg-primary/30' />
              <span className='text-muted-foreground'>Start-up</span>
              <span className='tabular-nums'>{ms(totalInit)}</span>
            </span>
            <span className='flex items-center gap-1.5'>
              <span className='inline-block size-2 rounded-full bg-primary/80' />
              <span className='text-muted-foreground'>App code</span>
              <span className='tabular-nums'>{ms(ownCode)}</span>
            </span>
          </div>
        </div>

        {/* One stacked bar per day: height is that day's wall time, the split
            inside it is where the time went.

            This replaced two small-multiple rows sharing one baseline. The
            arithmetic defeated them — start-up is about a tenth of wall time,
            so against a baseline set by the busiest day's *total* its row
            rendered as a hairline and read as an empty strip. Stacking keeps
            the one baseline, keeps the day-to-day size readable, and makes the
            share legible *within* each day, which is where a deploy's cold
            starts actually show up. */}
        <div className='space-y-1.5'>
          <div
            className='flex h-28 items-end gap-px'
            data-testid='admin-observability-compute-days'
          >
            {days.map((d) => {
              const init = Math.min(d.init_ms, d.wall_ms);
              const own = Math.max(d.wall_ms - init, 0);
              const h = (v: number) => (v / peak) * 100;
              return (
                <div
                  key={d.day}
                  className='group flex h-full flex-1 flex-col justify-end gap-px'
                  title={`${d.day} · ${ms(d.wall_ms)} total · start-up ${ms(init)} (${pct(
                    d.wall_ms > 0 ? init / d.wall_ms : null,
                    0
                  )}) · app code ${ms(own)}`}
                >
                  {/* Start-up sits on top, so the eye reads it against a
                      straight edge rather than a ragged one. */}
                  <div
                    className='w-full rounded-t-sm bg-primary/30'
                    style={{ height: `${Math.max(h(init), init > 0 ? 3 : 0)}%` }}
                  />
                  <div
                    className={cn(
                      "w-full",
                      init > 0 ? "" : "rounded-t-sm",
                      d.wall_ms > 0 ? "bg-primary/80" : "bg-muted-foreground/20"
                    )}
                    style={{ height: `${Math.max(h(own), d.wall_ms > 0 ? 3 : 1.5)}%` }}
                  />
                </div>
              );
            })}
          </div>
          <div className='flex items-center justify-between font-mono text-[10px] text-muted-foreground tabular-nums'>
            <span>{days[0].day}</span>
            <span className='text-foreground/70'>busiest day {ms(peak)}</span>
            <span>{days[days.length - 1].day}</span>
          </div>
        </div>

        <p className='text-[10px] text-muted-foreground'>
          {ceiling === null
            ? "The isolate heap ceiling is disabled, so wall time has no provisioned basis to convert against."
            : `GB·s is wall time at the ${ceiling} MiB isolate ceiling — provisioned, not consumed. An invocation waiting on a host call bills what one computing does; there is no CPU metering yet.`}
        </p>
      </div>
    </section>
  );
};

const Figure = ({ label, value, hint }: { label: string; value: string; hint?: string }) => (
  <div title={hint}>
    <dt className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.12em]'>
      {label}
    </dt>
    <dd className='font-medium text-sm tabular-nums'>{value}</dd>
  </div>
);
