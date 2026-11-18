import { AppWindow, ArrowUpCircle, Search } from "lucide-react";
import { useMemo, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { useBatchPromoteLatestApps } from "@/hooks/api/customApps/useBatchAppMutations";
import { errMessage } from "@/hooks/api/errMessage";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
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
import { discriminatingFilters } from "../../fleetFilters";
import { LIFECYCLE_COPY, LifecycleConfirm } from "../AppLifecycle";
import { statusTone } from "../AppSwitcher";
import { FleetRow, gridTemplate } from "./FleetRow";
import { FleetSummary } from "./FleetSummary";

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
  health,
  healthRows,
  fleet,
  storage,
  storageResponse
}: {
  apps: CustomApp[];
  health: HealthIndex | undefined;
  healthRows: ReadonlyMap<string, AppHealthRow> | undefined;
  fleet: FleetHealthResponse | undefined;
  storage: ReadonlyMap<string, AppStorageUsageRow> | undefined;
  /** The raw rollup, for the fleet-wide totals the summary states. */
  storageResponse: FleetStorageResponse | undefined;
}) => {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState<AppStatus | null>(null);

  const { shown, hidden } = useMemo(
    () =>
      fleetColumns({
        apps,
        observabilityConfigured: fleet?.observability_configured ?? false,
        storage
      }),
    [apps, fleet?.observability_configured, storage]
  );

  const filters = useMemo(() => discriminatingFilters(apps, health), [apps, health]);

  // Worst first — the same ordering the console's resolver uses, so the fleet's top row
  // and "the app that most needs a person" are the same app by construction.
  const rows = useMemo(() => {
    const ordered = byAttention(apps, health);
    return ordered.filter(
      (a) =>
        (!query || matchesQuery(a, query)) && (status === null || statusOf(a, health) === status)
    );
  }, [apps, health, query, status]);

  // The fleet's one verb: promote the newest build on every published app in view.
  // The batch endpoint and its hook existed with no consumer; this is the consumer.
  // Published apps only — on a draft the same call would be a first publish, which is
  // the per-app card's decision, not a fleet-wide one.
  const promoteLatest = useBatchPromoteLatestApps();
  const [promoteOpen, setPromoteOpen] = useState(false);
  const promotable = useMemo(() => rows.filter((a) => a.published_at), [rows]);
  const runPromoteLatest = async () => {
    try {
      const result = await promoteLatest.mutateAsync(promotable.map((a) => a.id));
      if (result.failed > 0) {
        toast.warning(`Promoted ${result.succeeded}, ${result.failed} failed.`);
      } else {
        toast.success(
          `Promoted the latest build on ${result.succeeded} app${result.succeeded === 1 ? "" : "s"}.`
        );
      }
      setPromoteOpen(false);
    } catch (err) {
      toast.error(errMessage(err, "Couldn't promote the latest builds"));
    }
  };

  return (
    <div className='flex min-h-0 flex-col' data-testid='admin-apps-fleet'>
      <div className='flex items-center gap-2 px-4 py-2'>
        <div className='relative min-w-0 flex-1 md:max-w-xs'>
          <Search className='absolute top-1/2 left-2 size-3 -translate-y-1/2 text-muted-foreground' />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder='Filter by name, slug or org…'
            className='h-7 pl-7 text-xs'
            data-testid='admin-apps-fleet-search'
          />
        </div>
        {filters.map((f) => (
          <button
            key={f.status}
            type='button'
            onClick={() => setStatus((s) => (s === f.status ? null : f.status))}
            data-testid={`admin-apps-fleet-filter-${f.status}`}
            className={cn(
              "h-7 shrink-0 rounded-md border px-2 text-xs transition-colors",
              status === f.status
                ? // Through `statusTone`, not a down/not-down guess: `discriminatingFilters`
                  // returns a chip for ANY status forming a proper subset, so an
                  // "Operational 2" chip was being painted in the warning tone. On this
                  // surface colour means something is wrong.
                  cn(ADMIN_TONE[statusTone(f.status)].bg, "border-transparent")
                : "border-border/60 text-muted-foreground hover:text-foreground"
            )}
          >
            {f.label} <span className='tabular-nums'>{f.count}</span>
          </button>
        ))}
        <span className='ml-auto shrink-0 text-muted-foreground text-xs tabular-nums'>
          {rows.length === apps.length
            ? `${apps.length} app${apps.length === 1 ? "" : "s"}`
            : `${rows.length} of ${apps.length}`}
        </span>
        <Button
          variant='outline'
          size='sm'
          className='h-7 shrink-0 gap-1.5 text-xs'
          disabled={promotable.length === 0 || promoteLatest.isPending}
          onClick={() => setPromoteOpen(true)}
          data-testid='admin-apps-fleet-promote-latest'
        >
          <ArrowUpCircle className='size-3.5' />
          Promote latest
          <span className='text-muted-foreground tabular-nums'>{promotable.length}</span>
        </Button>
      </div>

      <LifecycleConfirm
        copy={promoteOpen ? LIFECYCLE_COPY.promoteLatest(promotable.length) : null}
        pending={promoteLatest.isPending}
        onConfirm={runPromoteLatest}
        onCancel={() => setPromoteOpen(false)}
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
            title={query || status ? "Nothing matches." : "No custom apps on this deployment yet."}
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

        <FleetSummary apps={apps} storage={storageResponse} />
      </div>
    </div>
  );
};
