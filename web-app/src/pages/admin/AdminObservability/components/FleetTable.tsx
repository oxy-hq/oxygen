import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import type { Fleet, FleetRow } from "@/services/api/adminObservability";
import { ADMIN_TONE } from "../../components/adminTone";
import { type FleetColumn, fleetColumns } from "../fleetColumns";
import { compact, ms, pct, usd } from "../format";
import { reliabilityTone } from "../reliabilityTone";

/**
 * Every org, one row, ordered by spend.
 *
 * **Rows are `<Link>`s, so the table is a grid and not a `<table>`.** Operators
 * cmd-click and copy link addresses on this surface, which rules out a table
 * whose rows carry `onClick`. The cost of that is the rule the admin console
 * learned the hard way: per-row `display:grid` does **not** make columns —
 * `max-content` and `auto` tracks size from the items in *their own* container,
 * so N rows applying one template string resolve N different layouts, and the
 * fleet list shipped with its header sitting 119px off its rows.
 *
 * So: one grid container on the scroller, header and rows as
 * `grid-cols-subgrid` children of it. jsdom does not lay out grids, so no test
 * here can catch a regression — check it in the browser by reading back
 * `getComputedStyle(el).gridTemplateColumns` for the header *and* every row and
 * asserting one distinct result.
 */
export const FleetTable = ({ fleet }: { fleet: Fleet }) => {
  const { shown, hidden } = fleetColumns(fleet);
  // Spend descending: "which org got expensive" is the question this list is
  // opened with. Ties break on name so the order does not shuffle between
  // requests.
  const rows = [...fleet.rows].sort(
    (a, b) => b.cost_usd - a.cost_usd || a.org_name.localeCompare(b.org_name)
  );

  return (
    <div className='space-y-2' data-testid='admin-observability-fleet'>
      <div className='overflow-x-auto'>
        <div
          className='grid min-w-max'
          style={{
            gridTemplateColumns: `minmax(14rem,1fr) repeat(${shown.length - 1}, minmax(5rem, max-content))`
          }}
        >
          <div className='col-span-full grid grid-cols-subgrid border-border/60 border-b'>
            {shown.map((c) => (
              <HeaderCell key={c.id} column={c} />
            ))}
          </div>
          {rows.map((row) => (
            <Link
              key={row.org_id}
              // Carry the window: a 7- or 90-day list must not land on the
              // 30-day console and refetch everything for the default.
              to={`${ROUTES.ADMIN.OBSERVABILITY_ORG(row.org_id)}?days=${fleet.window_days}`}
              data-testid='admin-observability-fleet-row'
              className='col-span-full grid grid-cols-subgrid border-border/50 border-b transition-colors last:border-b-0 hover:bg-muted/40'
            >
              {shown.map((c) => (
                <BodyCell key={c.id} column={c} row={row} />
              ))}
            </Link>
          ))}
        </div>
      </div>
      {!fleet.storage_read ? (
        <p
          className={cn("text-xs", ADMIN_TONE.warn.text)}
          data-testid='admin-observability-fleet-storage-failed'
        >
          Couldn&rsquo;t read storage for this window. Every organization&rsquo;s storage figure is
          missing because of that, not because nothing was measured.
        </p>
      ) : null}
      {hidden ? (
        <p className='text-muted-foreground text-xs' data-testid='admin-observability-fleet-hidden'>
          {hidden.labels.join(", ")} {hidden.labels.length === 1 ? "is" : "are"} not shown because{" "}
          {hidden.why}.
        </p>
      ) : null}
    </div>
  );
};

const HeaderCell = ({ column }: { column: FleetColumn }) => (
  <div
    title={column.hint}
    className={cn(
      "px-3 py-2 font-medium text-[10px] text-muted-foreground uppercase tracking-wider",
      column.numeric && "text-right"
    )}
  >
    {column.label}
  </div>
);

/**
 * One cell. The formatters do the `null` → `—` work, so a column that the
 * server could not answer for *this* org reads as unknown rather than zero
 * even where the column itself is shown.
 */
const BodyCell = ({ column, row }: { column: FleetColumn; row: FleetRow }) => {
  const base = cn("px-3 py-2 text-xs", column.numeric && "text-right tabular-nums");

  switch (column.id) {
    case "org":
      return (
        <div className={cn(base, "min-w-0")}>
          <span className='block truncate font-medium'>{row.org_name}</span>
          <span className='block truncate font-mono text-[10px] text-muted-foreground'>
            {row.org_slug}
          </span>
        </div>
      );
    case "spend":
      return (
        <div className={cn(base, "font-medium")}>
          {usd(row.cost_usd)}
          {row.unpriced_runs > 0 ? (
            // Named, not hidden: the dollar figure excludes these runs, and an
            // operator comparing two orgs needs to know one of them is partly
            // unpriced.
            <span
              className='ml-1 text-[10px] text-muted-foreground'
              title={`${row.unpriced_runs.toLocaleString()} run${row.unpriced_runs === 1 ? "" : "s"} on a model with no known rate — tokens counted, dollars excluded`}
            >
              +{compact(row.unpriced_runs)}?
            </span>
          ) : null}
        </div>
      );
    case "runRate":
      return <div className={base}>{usd(row.monthly_run_rate_usd)}</div>;
    case "costPerRun":
      return <div className={base}>{usd(row.cost_per_run_usd)}</div>;
    case "unattended":
      return <div className={base}>{pct(row.unattended_run_share)}</div>;
    case "cache":
      return <div className={base}>{pct(row.cache_hit_share)}</div>;
    case "requests":
      return <div className={base}>{compact(row.requests)}</div>;
    case "p95":
      return <div className={base}>{ms(row.p95_ms)}</div>;
    case "errorRate":
      return (
        // Colour only when something is wrong — on this surface colour means
        // exactly that, and the threshold lives in `reliabilityTone` so this
        // table and the per-org console cannot disagree about what "bad" is.
        <div className={cn(base, ADMIN_TONE[reliabilityTone(row.failure_rate)].text)}>
          {pct(row.failure_rate, 1)}
        </div>
      );
  }
};
