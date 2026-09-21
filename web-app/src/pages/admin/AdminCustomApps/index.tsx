import { AppWindow } from "lucide-react";
import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useFleetStorage } from "@/hooks/api/customApps/useAppStorage";
import { useAppHealthIndex, useFleetHealth } from "@/hooks/api/customApps/useFleetHealth";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { openAdminPalette } from "@/pages/admin/components/AdminEntitySearch";
import type { AppStorageUsageRow, FleetStorageResponse } from "@/types/apps";
import { AllowListError, isAllowListError } from "./AllowListError";
import type { HealthIndex } from "./appStatus";
import { AppConsole } from "./components/AppConsole";
import { AppIdentity } from "./components/AppIdentity";
import { AppSwitcherTrigger } from "./components/AppSwitcher";
import { AppsFleet } from "./components/AppsFleet";
import { CreateCustomAppDialog } from "./components/CreateCustomAppDialog";
import { FleetStrip } from "./components/FleetStrip";
import { useAdminAppRegistry } from "./useAdminAppRegistry";

/**
 * The custom-apps staff console.
 *
 * Two surfaces on one route. `/admin/apps` is **the fleet** — every app, worst first;
 * `/admin/apps/:orgSlug/:appSlug` is **one app's console**. The fleet hands you to an
 * app; the console is where you work.
 *
 * **The fleet was cut and then asked back for, and the maintainer was right.** The first
 * pass of this redesign made the bare route redirect to whichever app most needed a
 * person, on the reasoning that an operator always arrives about *one* app. That
 * overcorrected: "is anything broken?" and "what is even deployed here?" are fleet
 * questions, and a console showing one app at a time cannot answer either. Worse, the
 * redirect made the URL non-deterministic — typing `/admin/apps` landed you somewhere
 * that depended on data, with no way to see what you were not being shown.
 *
 * What was actually wrong was never that a list existed. It was *that* list: two
 * structurally empty columns, three filter chips that selected the same three apps, one
 * deployment fact repeated once per row, and two apps both reading "Oxy Starter". Those
 * are defects of a particular table, not of listing things, and `fleetColumns` and
 * `fleetFilters` now make each of them unrepresentable rather than merely fixed. The
 * ordering is still `byAttention`, so the fleet's top row is the app the old redirect
 * would have chosen — the information survived; the coercion did not.
 *
 * What this replaces: four sibling tabs — Apps, Organizations, Publish tokens, Storage —
 * which were four *object types* presented as four *tasks*. Storage and Publishing/CI
 * are facts about an app, so they are panels on the console. The genuinely
 * deployment-level views are routed escape hatches, never tabs: `/admin/apps/storage`,
 * `/admin/apps/access`, and `…/preview` for one app's stage.
 *
 * **Accepted cost, still accepted:** "Organizations" as a browsable *second* app list is
 * gone. Org is part of every app's identity here, so the fleet answers "what does acme
 * have?" by filtering rather than by being a different page with a different layout.
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
  // map serves both the status model and the fleet's Requests column. Hand-rolling a
  // second, narrower map here is what made three `Map`s get rebuilt every keystroke.
  const health: HealthIndex | undefined = healthRows;

  // Only the fleet reads per-app storage; the console's own panel fetches its one app.
  const onFleet = !params.appSlug;
  const { data: storageResponse } = useFleetStorage("bytes", { enabled: onFleet });
  const storage = useMemo(() => storageIndex(storageResponse), [storageResponse]);

  const [createOpen, setCreateOpen] = useState(false);

  // Deliberately NOT a second ⌘K binding. The admin console already has one palette on
  // that key (`AdminEntitySearch`), and binding it again here opened both dialogs
  // stacked on a single keypress, each filtering its own half of the results. Custom
  // apps are a group in that palette now, and this surface asks it to open rather than
  // competing with it.

  return (
    <div className='flex h-[calc(100vh-3.5rem)] flex-col' data-testid='admin-customer-apps'>
      {/* The deployment-wide fact, once, above everything. Renders nothing when there is
          nothing fleet-wide to say. */}
      <FleetStrip fleet={fleet.data} />

      <header className='flex h-10 shrink-0 items-center gap-2 border-b px-3'>
        <AppSwitcherTrigger onClick={openAdminPalette} />
        {selected ? (
          <>
            {/* On an app, the way back to the whole fleet is a link, not the browser's
                Back button — an operator often arrives here from the palette or a
                pasted URL, where Back goes somewhere else entirely. */}
            <Button asChild variant='ghost' size='sm' className='h-7 shrink-0 text-xs'>
              <Link to='/admin/apps' data-testid='apps-console-all-apps'>
                All apps
              </Link>
            </Button>
            <AppIdentity app={selected} className='min-w-0 flex-1' />
          </>
        ) : (
          <span className='flex-1 text-muted-foreground text-xs'>
            Every custom app on this deployment, worst first.
          </span>
        )}
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
          //   `params.appSlug && !selected` — `isWalking` alone would hold the FLEET in a
          //   skeleton, when it should show the rows it has and grow.
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
          {(list) =>
            params.appSlug && !selected ? (
              // Named an app the registry does not have. Distinct from "no apps exist" and
              // from a load failure, and it must not silently redirect somewhere else —
              // a stale link should say it is stale.
              <AdminEmptyState
                className='m-6'
                icon={AppWindow}
                title={`No app at ${params.orgSlug}/${params.appSlug}.`}
                description='It may have been deleted, or the slug may have changed.'
                action={
                  <Button
                    size='sm'
                    variant='outline'
                    className='h-7 text-xs'
                    onClick={openAdminPalette}
                  >
                    Switch app
                  </Button>
                }
              />
            ) : selected ? (
              <AppConsole app={selected} health={health} />
            ) : (
              // Bare `/admin/apps`: the fleet. It used to redirect to whichever app most
              // needed a person — same ordering, but it decided *for* the operator and
              // hid everything it did not choose.
              <AppsFleet
                apps={list}
                health={health}
                healthRows={healthRows}
                fleet={fleet.data}
                storage={storage}
                storageResponse={storageResponse}
              />
            )
          }
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
