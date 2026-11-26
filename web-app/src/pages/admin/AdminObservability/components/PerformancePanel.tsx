import { Activity, AppWindow, Snowflake, Users } from "lucide-react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import type { OrgApps } from "@/services/api/adminObservability";
import { AdminSectionLabel } from "../../components/AdminSectionLabel";
import { AdminTrendBars } from "../../components/AdminTrendBars";
import { ADMIN_TONE } from "../../components/adminTone";
import { bytes, compact, ms, outcomeLabel, pct } from "../format";
import { reliabilityTone } from "../reliabilityTone";

/**
 * How this org's custom apps are performing.
 *
 * Two choices run through the whole panel and both are deliberate:
 *
 * - **Quantiles, never averages.** p50/p95/p99, and a p95 for cold starts.
 *   A mean hides the tail users feel, and it is the number that stays flat
 *   while p99 triples.
 * - **Failures by class, not by status code.** `outcome` keeps the three SRE
 *   classes apart — an explicit 5xx, an *implicit* failure (a 200 carrying a
 *   shell that never mounted), and a *policy* one (a 200 that took too long).
 *   A `status >= 500` rollup reports the white-screen case as success, which
 *   is the defect the column exists to prevent.
 */
export const PerformancePanel = ({
  apps,
  orgSlug
}: {
  apps: OrgApps;
  /** The org's slug, for the per-app console link. Empty while unknown. */
  orgSlug: string;
}) => {
  if (!apps.measured) {
    return (
      <section className='space-y-3' data-testid='admin-observability-apps'>
        <AdminSectionLabel>Custom-app performance</AdminSectionLabel>
        <p
          className=' border border-border/60 border-dashed bg-muted/20 p-4 text-muted-foreground text-xs'
          data-testid='admin-observability-apps-unmeasured'
        >
          Not measured. This deployment has no observability store, so there is nothing to read —
          set <code className='font-mono'>OXY_OBSERVABILITY_BACKEND=clickhouse</code> (with{" "}
          <code className='font-mono'>OXY_CLICKHOUSE_*</code>) to start capturing. This is not the
          same as an org whose apps served nothing.
        </p>
      </section>
    );
  }

  const t = apps.total;

  return (
    <section className='space-y-5' data-testid='admin-observability-apps'>
      <AdminSectionLabel trailing={`last ${apps.window_days}d`}>
        Custom-app performance
      </AdminSectionLabel>

      <div className='grid gap-4 lg:grid-cols-3'>
        <div className='space-y-4 border border-border/60 bg-card p-4 lg:col-span-2'>
          <div className='flex flex-wrap items-end justify-between gap-4'>
            <div>
              <span className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
                Requests served
              </span>
              <div
                className='font-semibold text-2xl tabular-nums tracking-tight'
                data-testid='admin-observability-apps-requests'
              >
                {compact(t.requests)}
              </div>
            </div>
            <div className='flex flex-wrap gap-5 text-right'>
              <Figure icon={AppWindow} label='Apps' value={compact(t.apps)} />
              <Figure icon={Users} label='People' value={compact(t.people)} />
              <Figure icon={Activity} label='Sessions' value={compact(t.sessions)} />
              <Figure
                icon={Snowflake}
                label='Cold start p95'
                value={ms(t.cold_start_p95_ms)}
                hint='95th percentile of start-up time, over requests that paid one'
              />
            </div>
          </div>

          <AdminTrendBars
            testId='admin-observability-apps-trend'
            emptyLabel='No requests in this window.'
            points={apps.by_day.map((d) => ({
              day: d.day,
              value: d.requests,
              // The bar turns warning-toned on a day that failed enough to
              // matter, so a bad day is visible in the shape rather than only
              // in a number below it.
              flagged: reliabilityTone(d.failure_rate) !== "muted",
              detail: `${compact(d.requests)} req · p95 ${ms(d.p95_ms)} · ${pct(d.failure_rate, 1)} failed`
            }))}
          />

          <dl className='grid grid-cols-2 gap-3 text-xs sm:grid-cols-4'>
            <Figure2 label='p50' value={ms(t.p50_ms)} />
            <Figure2 label='p95' value={ms(t.p95_ms)} />
            <Figure2 label='p99' value={ms(t.p99_ms)} />
            <Figure2 label='Served' value={bytes(t.bytes)} />
          </dl>
        </div>

        <div className='space-y-3 border border-border/60 bg-card p-4'>
          <AdminSectionLabel trailing={pct(t.failure_rate, 1)}>Failures</AdminSectionLabel>
          {apps.failures.length === 0 ? (
            <p
              className='text-muted-foreground text-xs'
              data-testid='admin-observability-apps-failures-empty'
            >
              {t.requests === 0
                ? "No requests in this window."
                : "Every request succeeded in this window."}
            </p>
          ) : (
            <div className='divide-y divide-border/50'>
              {apps.failures.slice(0, 8).map((f) => (
                <div
                  key={`${f.outcome}:${f.error_kind}`}
                  className='flex items-baseline gap-2 py-1.5 text-xs'
                  data-testid='admin-observability-apps-failure-row'
                >
                  <span className='min-w-0 flex-1'>
                    <span className='font-medium'>{outcomeLabel(f.outcome)}</span>
                    {f.error_kind ? (
                      <>
                        {/* A real space, not only a margin: the two read as one
                            word to a screen reader otherwise
                            ("Server erroroltp_connect"). */}{" "}
                        <span className='truncate font-mono text-[10px] text-muted-foreground'>
                          {f.error_kind}
                        </span>
                      </>
                    ) : null}
                  </span>
                  <span className='shrink-0 text-muted-foreground tabular-nums'>
                    {compact(f.requests)}
                  </span>
                  {/* Of ALL traffic, not of all failures: "3% of traffic was a
                      white screen" is actionable; "40% of failures" hides
                      whether there were four of them or forty thousand. */}
                  <span className='w-12 shrink-0 text-right tabular-nums'>{pct(f.share, 1)}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      <AppsTable apps={apps} orgSlug={orgSlug} />
      <RoutesTable apps={apps} />
    </section>
  );
};

/**
 * Per app, ordered by traffic.
 *
 * A row links to that app's console at `/admin/apps/<orgSlug>/<appSlug>` —
 * the shape the route actually mounts, and the one the usage report uses.
 * `/admin/apps/<uuid>` matches nothing and lands on the catch-all.
 *
 * A row with no slug is an app the catalog no longer has, whose events are
 * still inside retention. It stays in the table, because it contributed to the
 * totals above, but it is **not a link**: there is no console to open.
 */
const AppsTable = ({ apps, orgSlug }: { apps: OrgApps; orgSlug: string }) => (
  <div className='space-y-2' data-testid='admin-observability-apps-table'>
    <AdminSectionLabel>By app</AdminSectionLabel>
    {apps.by_app.length === 0 ? (
      <p className='text-muted-foreground text-xs'>No app traffic in this window.</p>
    ) : (
      <div className='overflow-x-auto'>
        <div
          className='grid min-w-max'
          style={{
            gridTemplateColumns: "minmax(12rem,1fr) repeat(6, minmax(4.5rem, max-content))"
          }}
        >
          <Head
            cells={[{ label: "App" }, "Requests", "Errors", "p50", "p95", "Cold p95", "Served"]}
          />
          {apps.by_app.map((a) => (
            <AppRowShell key={a.app_id} app={a} orgSlug={orgSlug}>
              <Cell>
                <span className='block truncate font-medium'>{a.name}</span>
                {a.slug ? (
                  <span className='block truncate font-mono text-[10px] text-muted-foreground'>
                    {a.slug}
                  </span>
                ) : null}
              </Cell>
              <Cell numeric>{compact(a.requests)}</Cell>
              <Cell numeric className={ADMIN_TONE[reliabilityTone(a.failure_rate)].text}>
                {pct(a.failure_rate, 1)}
              </Cell>
              <Cell numeric>{ms(a.p50_ms)}</Cell>
              <Cell numeric>{ms(a.p95_ms)}</Cell>
              <Cell numeric>{ms(a.cold_start_p95_ms)}</Cell>
              <Cell numeric>{bytes(a.bytes)}</Cell>
            </AppRowShell>
          ))}
        </div>
      </div>
    )}
  </div>
);

/**
 * The slowest routes, which is how a slow deployment gets narrowed to one
 * handler.
 *
 * Ranked by p95 and floored at a sample the server enforces — a route hit
 * three times has a p95 that is one of those three requests, and ranking by it
 * puts a cold one-off at the top on every load.
 */
const RoutesTable = ({ apps }: { apps: OrgApps }) => (
  <div className='space-y-2' data-testid='admin-observability-routes-table'>
    <AdminSectionLabel>Slowest routes</AdminSectionLabel>
    {apps.routes.length === 0 ? (
      <p className='text-muted-foreground text-xs'>
        No route has enough traffic in this window to rank.
      </p>
    ) : (
      <div className='overflow-x-auto'>
        <div
          className='grid min-w-max'
          style={{ gridTemplateColumns: "minmax(18rem,1fr) repeat(3, minmax(5rem, max-content))" }}
        >
          <Head cells={[{ label: "Route" }, { label: "App" }, "Requests", "p95"]} />
          {apps.routes.map((r) => (
            <div
              key={`${r.app_id}:${r.route}`}
              data-testid='admin-observability-route-row'
              className='col-span-full grid grid-cols-subgrid border-border/50 border-b last:border-b-0'
            >
              <Cell>
                <span className='block truncate font-mono text-[11px]'>{r.route}</span>
              </Cell>
              <Cell>
                <span className='block truncate'>{r.app_name}</span>
              </Cell>
              <Cell numeric>{compact(r.requests)}</Cell>
              <Cell numeric>{ms(r.p95_ms)}</Cell>
            </div>
          ))}
        </div>
      </div>
    )}
  </div>
);

/**
 * A header row. A bare string is a figure column and is right-aligned, so
 * digits read down the column; `{ label }` is a text column and stays left.
 *
 * Alignment follows the *content*, not the position. The routes table's second
 * column is an app's name, and right-aligning it put a ragged left edge in the
 * middle of the table with a lake of space before it.
 */
type HeadCell = string | { label: string };

/**
 * A row that is a link only when there is somewhere to go. Same grid child
 * either way, so the subgrid columns stay aligned whichever it renders as.
 */
const AppRowShell = ({
  app,
  orgSlug,
  children
}: {
  app: OrgApps["by_app"][number];
  orgSlug: string;
  children: React.ReactNode;
}) => {
  const className =
    "col-span-full grid grid-cols-subgrid border-border/50 border-b last:border-b-0";
  if (!orgSlug || !app.slug) {
    return (
      <div className={className} data-testid='admin-observability-apps-row'>
        {children}
      </div>
    );
  }
  return (
    <Link
      to={`${ROUTES.ADMIN.CUSTOMER_APPS}/${orgSlug}/${app.slug}`}
      data-testid='admin-observability-apps-row'
      className={cn(className, "transition-colors hover:bg-muted/40")}
    >
      {children}
    </Link>
  );
};

const Head = ({ cells }: { cells: HeadCell[] }) => (
  <div className='col-span-full grid grid-cols-subgrid border-border/60 border-b'>
    {cells.map((c) => {
      const label = typeof c === "string" ? c : c.label;
      return (
        <div
          key={label}
          className={cn(
            "px-3 py-2 font-medium text-[10px] text-muted-foreground uppercase tracking-wider",
            typeof c === "string" && "text-right"
          )}
        >
          {label}
        </div>
      );
    })}
  </div>
);

const Cell = ({
  children,
  numeric,
  className
}: {
  children: React.ReactNode;
  numeric?: boolean;
  className?: string;
}) => (
  <div className={cn("min-w-0 px-3 py-2 text-xs", numeric && "text-right tabular-nums", className)}>
    {children}
  </div>
);

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

const Figure2 = ({ label, value }: { label: string; value: string }) => (
  <div className='space-y-0.5'>
    <dt className='text-muted-foreground'>{label}</dt>
    <dd className='font-medium tabular-nums'>{value}</dd>
  </div>
);
