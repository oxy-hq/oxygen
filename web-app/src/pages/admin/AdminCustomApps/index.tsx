import { AppWindow } from "lucide-react";
import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useFleetStorage } from "@/hooks/api/customApps/useAppStorage";
import { useAppHealthIndex, useFleetHealth } from "@/hooks/api/customApps/useFleetHealth";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { openAdminPalette } from "@/pages/admin/components/AdminEntitySearch";
import type { AppStorageUsageRow, FleetStorageResponse } from "@/types/apps";
import { AllowListError, isAllowListError } from "./AllowListError";
import type { HealthIndex } from "./appStatus";
import {
  AgentSkillsButton,
  AgentSkillsStrip,
  useAgentSkillsDismissed
} from "./components/AgentSkills";
import { AppBrowser } from "./components/AppBrowser";
import { AppSwitcherTrigger } from "./components/AppSwitcher";
import { CreateCustomAppDialog } from "./components/CreateCustomAppDialog";
import { FleetStrip } from "./components/FleetStrip";
import { useAdminAppRegistry } from "./useAdminAppRegistry";

/**
 * The custom-apps staff console.
 *
 * **One surface now, in three tiers.** `/admin/apps` is a Finder column browser —
 * organizations, that org's apps, that app's console pages — with the thing you selected
 * in the pane beside them (`components/AppBrowser`). The URLs did not change: a bare
 * `/admin/apps` opens on every app, `?org=acme` on one org, and
 * `/admin/apps/:orgSlug/:appSlug/:consolePage` on one page of one app's console, which is
 * what the columns resolve to a selection.
 *
 * **What this replaced, and what survived it.** Before the browser, this route was two
 * surfaces that did not know about each other: a flat worst-first table of every app, and
 * an app console with its own eight-entry sidebar. Both are still here — the table is
 * what the pane shows when no app is selected, scoped to the selected org, and the
 * console is what it shows when one is — but neither is the navigation any more. That
 * was the cost worth removing: the table flattened orgs into a text cell and the sidebar
 * hid the pages behind a selection, so moving one app sideways meant going back to a
 * list, re-reading the whole fleet, and two more clicks.
 *
 * Two things left the header, because the browser says them better. The "All apps" button
 * is the path bar's first segment, and the app's name and org are in the pane's own
 * header and the path bar both — a third copy beside the controls was there only because
 * nothing else on screen said where you were.
 *
 * The genuinely deployment-level views stay routed escape hatches, never tabs:
 * `/admin/apps/storage`, `/admin/apps/access`, and `…/preview` for one app's stage.
 *
 * **Below `lg` the browser does not render** and this is the surface it always was: the
 * table, or the console with its own nav. Three columns need width, and a drill-down
 * stack for phones would be a third navigation model to keep working.
 */
export default function AdminCustomApps() {
  const params = useParams<{ orgSlug?: string; appSlug?: string }>();
  const { apps, selected, isLoading, isWalking, error, refetch } = useAdminAppRegistry(
    params.orgSlug,
    params.appSlug
  );
  // Two reads of ONE query — same key, so one request. `useFleetHealth` for the
  // deployment-wide facts the strip states, `useAppHealthIndex` for the per-app rows,
  // which is already `useMemo`'d over the same data.
  const fleet = useFleetHealth();
  const { rows: healthRows } = useAppHealthIndex();
  // `AppHealthRow` carries `health`, so the whole row satisfies `HealthIndex` and one
  // map serves the status model, the fleet's Requests column and the browser's dots.
  // Hand-rolling a second, narrower map here is what made three `Map`s get rebuilt every
  // keystroke.
  const health: HealthIndex | undefined = healthRows;

  // Only the fleet table reads per-app storage, and the pane shows it exactly when no app
  // is selected — which is still what this gate says. The console's own panel fetches its
  // one app.
  const onFleet = !params.appSlug;
  const { data: storageResponse } = useFleetStorage("bytes", { enabled: onFleet });
  const storage = useMemo(() => storageIndex(storageResponse), [storageResponse]);

  const [createOpen, setCreateOpen] = useState(false);
  // The strip is the fleet-wide reminder until it is dismissed; after that, and on every
  // app console, it is the header's "Agent skills" button.
  const [skillsDismissed, dismissSkills] = useAgentSkillsDismissed();
  const skillsStrip = onFleet && !skillsDismissed;

  // Deliberately NOT a second ⌘K binding. The admin console already has one palette on
  // that key (`AdminEntitySearch`), and binding it again here opened both dialogs
  // stacked on a single keypress, each filtering its own half of the results. Custom
  // apps are a group in that palette now, and this surface asks it to open rather than
  // competing with it. The browser's own type-ahead is per column and takes no shortcut,
  // so it cannot collide with either.

  return (
    <div className='flex h-[calc(100vh-3.5rem)] flex-col' data-testid='admin-customer-apps'>
      {/* The deployment-wide fact, once, above everything. Renders nothing when there is
          nothing fleet-wide to say. */}
      <FleetStrip fleet={fleet.data} />

      {/* On a phone the bar wraps rather than dropping a control. */}
      <header className='flex h-10 shrink-0 items-center gap-2 border-b px-3 max-sm:h-auto max-sm:min-h-10 max-sm:flex-wrap max-sm:gap-y-1 max-sm:py-1.5'>
        <AppSwitcherTrigger onClick={openAdminPalette} />
        {/* The surface's one line, and only where it is still true of what is on screen:
            inside an app the pane's own header and the path bar say where you are, and
            this would be dead text beside them. */}
        <span className='min-w-0 flex-1 truncate text-muted-foreground text-xs'>
          {selected ? "" : "Every custom app on this deployment, by org."}
        </span>
        {!skillsStrip && <AgentSkillsButton />}
        <Button
          variant='outline'
          size='sm'
          className='h-7 text-xs'
          onClick={() => setCreateOpen(true)}
          data-testid='apps-new'
        >
          New app
        </Button>
      </header>

      {skillsStrip && <AgentSkillsStrip onDismiss={dismissSkills} />}

      {/* A 403 is the one failure here an operator can fix themselves, and `AdminAsync`
          cannot say so — `errorDetail` strips axios's "status code 403". Ahead of the
          kit, therefore, not routed through it. */}
      {error && isAllowListError(error) ? (
        <AllowListError error={error} onRetry={refetch} noun='the custom-app registry' />
      ) : (
        <AdminAsync
          // `isPending` also covers "named an app page one does not have, while later
          // pages are still arriving" — which used to be a hand-rolled skeleton branch
          // below, byte-identical to the one `AdminAsync` already renders. Two guards,
          // both load-bearing:
          //
          //   `params.appSlug && !selected` — `isWalking` alone would hold the BROWSER in
          //   a skeleton, when it should show the tiers it has and grow.
          //
          //   `&& !error` — `AdminAsync` checks loading BEFORE error, so a forced
          //   `isPending` makes the error branch and its Retry unreachable. That is not
          //   hypothetical: a failed page 2 leaves `error` truthy while `hasNextPage`,
          //   recomputed from the last *successful* page, stays true — so `isWalking`
          //   never clears and the skeleton is permanent, with the walk effect re-firing
          //   behind it. Measured before this guard: skeleton forever, no message, no
          //   Retry, 6 page-2 attempts. The old hand-rolled branch lived inside
          //   `children`, i.e. after the error check, so it could not do this.
          query={{
            isPending: isLoading || Boolean(params.appSlug && !selected && isWalking && !error),
            isError: Boolean(error),
            data: apps,
            error,
            refetch
          }}
          noun='the custom-app registry'
          rows={4}
          className='p-6'
          isEmpty={(list) => list.length === 0}
          empty={
            <AdminEmptyState
              icon={AppWindow}
              title='No custom apps on this deployment yet.'
              description='An app appears here once it is registered; builds arrive with oxyc publish.'
              action={
                <Button size='sm' className='h-7 text-xs' onClick={() => setCreateOpen(true)}>
                  Register an app
                </Button>
              }
            />
          }
        >
          {(list) => (
            <AppBrowser
              apps={list}
              selected={selected}
              // A stale link says it is stale in the pane, with the tiers still up beside
              // it — distinct from "no apps exist" and from a load failure, and it must
              // never silently redirect somewhere else.
              missingApp={
                params.appSlug && !selected ? `${params.orgSlug}/${params.appSlug}` : null
              }
              health={health}
              data={{ health, healthRows, fleet: fleet.data, storage, storageResponse }}
            />
          )}
        </AdminAsync>
      )}

      <CreateCustomAppDialog open={createOpen} onOpenChange={setCreateOpen} />
    </div>
  );
}

/**
 * Per-app storage keyed by app id, and `undefined` until it genuinely has an answer.
 *
 * A plain function, called inside `useMemo` by the caller — deliberately not named
 * `use*`. The three helpers this replaces were `use`-prefixed but contained no hooks and
 * returned early on `if (!rows) return undefined`, which is a rules-of-hooks landmine
 * the moment anyone adds a real hook inside one. They also rebuilt a `Map` on every
 * render, and since these feed the fleet's `fleetColumns` / `discriminatingFilters` /
 * `byAttention` memos by identity, every keystroke in the search box re-ran all three
 * plus a full re-sort of the registry.
 *
 * Returning an empty map while the rollup is in flight would let `fleetColumns` conclude
 * "no app can answer Storage" and drop the column, which would then pop in a moment
 * later and shift every other column sideways. `undefined` means *not known yet*.
 */
function storageIndex(
  data: FleetStorageResponse | undefined
): ReadonlyMap<string, AppStorageUsageRow> | undefined {
  if (!data) return undefined;
  return new Map(data.rows.map((r) => [r.appId, r]));
}
