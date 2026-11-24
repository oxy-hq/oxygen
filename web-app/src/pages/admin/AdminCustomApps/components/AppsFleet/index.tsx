import { AppWindow } from "lucide-react";
import { useMemo, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import type {
  AppHealthRow,
  AppStorageUsageRow,
  CustomApp,
  FleetHealthResponse,
  FleetStorageResponse
} from "@/types/apps";
import {
  type AppStatus,
  byAttention,
  type HealthIndex,
  matchesQuery,
  statusOf
} from "../../appStatus";
import { fleetColumns } from "../../fleetColumns";
import { appliedStatus, discriminatingFilters } from "../../fleetFilters";
import { LIFECYCLE_COPY, LifecycleConfirm } from "../AppLifecycle";
import { FleetRow, gridTemplate } from "./FleetRow";
import { FleetSummary } from "./FleetSummary";
import { FleetToolbar } from "./FleetToolbar";
import { usePromoteLatest } from "./usePromoteLatest";

/**
 * The fleet: every custom app on the deployment, worst first.
 *
 * This surface was removed in the first cut of the redesign and asked for back, which
 * was the right call — "is anything broken?" and "what is out there?" are fleet
 * questions, and a console that shows one app at a time cannot answer either. What was
 * wrong before was never that a list existed; it was *that particular list*. So the list
 * returns and its seven measured defects do not:
 *
 * - **Columns are derived, not declared** (`fleetColumns`). A column this deployment
 *   cannot answer is absent and named once at the foot, instead of rendering `—` on
 *   every row forever in the two widest columns.
 * - **Filters must partition** (`discriminatingFilters`). "All 3 · Needs attention 3 ·
 *   Not measured 3" — three chips selecting the same three apps — cannot recur, because
 *   a chip matching the whole fleet is excluded at the source. On today's data no chips
 *   render at all, which is the honest answer.
 * - **Identity always carries the org** (`AppIdentity`), so two apps both called "Oxy
 *   Starter" are never told apart by a distant cell.
 * - **The deployment's own facts are stated once**, by `FleetStrip` above this, not
 *   re-stated per row.
 * - **Unknown is not a verdict.** `statusOf` returns `null` before health lands and the
 *   row says "Unknown" rather than borrowing a green pill.
 *
 * It is also not the four-tab surface it replaced: there is no Organizations tab (org is
 * part of every app's identity here), no Publish tokens tab and no Storage tab (both are
 * per-app facts on the console, with the genuinely fleet-wide storage view one link
 * away).
 */
export const AppsFleet = ({
  apps,
  scopeOrg = null,
  health,
  healthRows,
  fleet,
  storage,
  storageResponse
}: {
  apps: CustomApp[];
  /**
   * One org's slug when the column browser has an org selected, `null` for the whole
   * deployment.
   *
   * **Every count narrows with it.** The derived columns, the discriminating filters,
   * the "n of m" and the promote-latest set are all computed over the scoped apps, so an
   * org view cannot say "3 of 40" about three apps or offer to promote the deployment.
   * The deployment-wide summary is the one thing that does *not* narrow — it links to
   * audits that are fleet-wide and counts orgs, so under one org it would be a sentence
   * about a scope nobody asked for. It is dropped instead of being rewritten.
   */
  scopeOrg?: string | null;
  health: HealthIndex | undefined;
  healthRows: ReadonlyMap<string, AppHealthRow> | undefined;
  fleet: FleetHealthResponse | undefined;
  storage: ReadonlyMap<string, AppStorageUsageRow> | undefined;
  /** The raw rollup, for the fleet-wide totals the summary states. */
  storageResponse: FleetStorageResponse | undefined;
}) => {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState<AppStatus | null>(null);

  const inScope = useMemo(
    () => (scopeOrg ? apps.filter((a) => a.org_slug === scopeOrg) : apps),
    [apps, scopeOrg]
  );

  const { shown, hidden } = useMemo(
    () =>
      fleetColumns({
        apps: inScope,
        observabilityConfigured: fleet?.observability_configured ?? false,
        storage
      }),
    [inScope, fleet?.observability_configured, storage]
  );

  const filters = useMemo(() => discriminatingFilters(inScope, health), [inScope, health]);

  // What the chosen filter means in THIS scope — see `appliedStatus`. Everything below
  // reads this rather than `status`, so the rows, the chips and the empty state cannot
  // disagree about whether a filter is on.
  const active = appliedStatus(status, filters);

  // Worst first — the same ordering the console's resolver uses, so the fleet's top row
  // and "the app that most needs a person" are the same app by construction.
  const rows = useMemo(() => {
    const ordered = byAttention(inScope, health);
    return ordered.filter(
      (a) =>
        (!query || matchesQuery(a, query)) && (active === null || statusOf(a, health) === active)
    );
  }, [inScope, health, query, active]);

  // The fleet's one verb, with its mutation and its partial-failure handling in a hook
  // of its own. The batch endpoint existed with no consumer; this is the consumer.
  const promote = usePromoteLatest(rows);

  return (
    // `flex-1` so the scroller below actually has a height to fill: inside the browser's
    // pane this is what keeps the table scrolling on its own instead of pushing the path
    // bar off the bottom of the page.
    <div className='flex min-h-0 flex-1 flex-col' data-testid='admin-apps-fleet'>
      <FleetToolbar
        query={query}
        onQueryChange={setQuery}
        filters={filters}
        status={active}
        onStatusChange={setStatus}
        counts={{ shown: rows.length, total: inScope.length }}
        promote={{
          count: promote.promotable.length,
          disabled: promote.promotable.length === 0 || promote.pending,
          onOpen: () => promote.setOpen(true)
        }}
      />

      <LifecycleConfirm
        copy={promote.open ? LIFECYCLE_COPY.promoteLatest(promote.promotable.length) : null}
        pending={promote.pending}
        onConfirm={promote.run}
        onCancel={() => promote.setOpen(false)}
      />

      <div className='min-h-0 flex-1 overflow-auto'>
        {/* The one grid container. Tracks are declared here and inherited by the header
            and every row through `grid-cols-subgrid`, which is what actually makes a
            column a column — see the note on `gridTemplate`.

            **The sticky header survives being a grid item, and that is measured, not
            assumed.** The reasoning against it is good and worth knowing: a grid item's
            containing block is its grid area, the header occupies row 1 alone, and a
            sticky box cannot be offset outside its containing block — which is the
            mechanism behind the familiar "sticky sidebar in a grid needs
            `align-self: start`". It predicts the header should have zero slack and
            scroll away. Measured in Chromium with 43 rows: the list moved -600px, the
            header moved 0.0px and stayed flush with the scrollport, and all 26 grid
            containers still resolved one identical column layout mid-scroll.

            So: do not "fix" this from the spec argument alone — re-measure first, with
            enough rows to overflow (three never scroll, which is why neither this nor
            the misalignment it replaced was visible). Only Chromium was checked. If a
            non-Chromium browser does scroll the header away, the fallback is explicit
            track widths on `FleetColumn`, which lets the header move back outside the
            grid — at the cost of `App` no longer taking the slack. */}
        <div className='grid gap-x-3' style={{ gridTemplateColumns: gridTemplate(shown) }}>
          <div className='sticky top-0 z-10 col-span-full grid grid-cols-subgrid border-b bg-background px-4 py-1.5 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
            {shown.map((c) => (
              <span key={c.id} className={cn("min-w-0 truncate", c.numeric && "text-right")}>
                {c.label}
              </span>
            ))}
          </div>

          {rows.map((app) => (
            <FleetRow
              key={app.id}
              app={app}
              columns={shown}
              health={healthRows?.get(app.id)}
              storage={storage?.get(app.id)}
            />
          ))}
        </div>

        {rows.length === 0 ? (
          <AdminEmptyState
            className='m-6'
            icon={AppWindow}
            title={emptyTitle(Boolean(query || active), scopeOrg)}
            description={
              query || status
                ? "Clear the filter to see the whole fleet."
                : "An app appears here once it is registered; builds arrive with oxyc publish."
            }
          />
        ) : null}

        {/* A column that vanished says why, once. Silence would leave an operator unable
            to tell "this deployment cannot measure that" from "someone removed it". */}
        {hidden.length > 0 && (
          <p
            className='border-t px-4 py-1.5 text-[11px] text-muted-foreground'
            data-testid='admin-apps-fleet-hidden-columns'
          >
            Not shown:{" "}
            {hidden.map((h, i) => (
              <span key={h.label}>
                {i > 0 && ", "}
                <span className='text-foreground/70'>{h.label}</span> — {h.why}
              </span>
            ))}
          </p>
        )}

        {/* How old the health column is, beside it. The endpoint returns
            `evaluated_at` so the page can say its own age (`useFleetHealth`), and
            nothing did: "nothing changed" and "nothing re-checked" looked the same. */}
        {fleet && (
          <p
            className='border-t px-4 py-1.5 text-[11px] text-muted-foreground'
            data-testid='admin-apps-fleet-evaluated'
            title={new Date(fleet.evaluated_at).toLocaleString()}
          >
            Health {timeAgo(fleet.evaluated_at)} · every minute
          </p>
        )}

        {/* Deployment-wide facts, so only on the deployment-wide view — see `scopeOrg`. */}
        {!scopeOrg && <FleetSummary apps={apps} storage={storageResponse} />}
      </div>
    </div>
  );
};

/**
 * What an empty list says, which is three different facts and not one with modifiers: a
 * filter that matched nothing, an org with no apps, and a deployment with none. The last
 * two must not borrow each other's sentence — "no custom apps on this deployment" under
 * one org would be a claim about the whole fleet from a view that cannot see it.
 */
function emptyTitle(filtered: boolean, scopeOrg: string | null): string {
  if (filtered) return "Nothing matches.";
  if (scopeOrg) return `No custom apps under ${scopeOrg}.`;
  return "No custom apps on this deployment yet.";
}
