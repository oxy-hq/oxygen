import { AppWindow } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { openAdminPalette } from "@/pages/admin/components/AdminEntitySearch";
import type {
  AppHealthRow,
  AppStorageUsageRow,
  CustomApp,
  FleetHealthResponse,
  FleetStorageResponse
} from "@/types/apps";
import type { BrowserScope } from "../../appBrowserModel";
import type { HealthIndex } from "../../appStatus";
import { AppConsole } from "../AppConsole";
import { AppsFleet } from "../AppsFleet";

export interface DetailPaneData {
  health: HealthIndex | undefined;
  healthRows: ReadonlyMap<string, AppHealthRow> | undefined;
  fleet: FleetHealthResponse | undefined;
  storage: ReadonlyMap<string, AppStorageUsageRow> | undefined;
  storageResponse: FleetStorageResponse | undefined;
}

/**
 * What the rightmost pane shows, which is the one place this redesign *adds* no new
 * surface.
 *
 * - An app selected → that app's console, exactly as before, with its sidebar suppressed
 *   at `lg` and up because the Pages tier is now that sidebar.
 * - An org, or All apps → the fleet table, exactly as before, scoped to what is selected.
 *
 * **The table is still here, and that is the point.** A column browser answers "where is
 * X?"; "is anything broken, and which of these is worst?" is a comparison across rows,
 * and a table answers it better than any column can. So the browser took over navigation
 * and gave up nothing: the search, the discriminating filters, the derived columns, the
 * per-app storage and the batch promote all still live in the pane, and under an org they
 * are scoped to that org's apps rather than silently showing the deployment.
 */
export const DetailPane = ({
  scope,
  app,
  apps,
  missingApp,
  data
}: {
  scope: BrowserScope;
  /** The app the route names, when the registry has it. */
  app: CustomApp | null;
  /** Every app, unscoped — `AppsFleet` narrows them itself. */
  apps: CustomApp[];
  /** `org/slug` the route named when the registry has no such app. */
  missingApp: string | null;
  data: DetailPaneData;
}) => {
  if (missingApp) {
    // A stale link should say it is stale. It must not redirect, and — unlike before —
    // it no longer costs you the browser: the columns stay up beside this, so the next
    // app is one keypress away rather than a trip back to a list.
    return (
      <AdminEmptyState
        className='m-6'
        icon={AppWindow}
        title={`No app at ${missingApp}.`}
        description='It may have been deleted, or the slug may have changed.'
        action={
          <Button size='sm' variant='outline' className='h-7 text-xs' onClick={openAdminPalette}>
            Switch app
          </Button>
        }
        data-testid='admin-apps-browser-missing-app'
      />
    );
  }

  if (app) return <AppConsole app={app} railed />;

  return (
    <AppsFleet
      apps={apps}
      scopeOrg={scope.kind === "org" ? scope.orgSlug : null}
      health={data.health}
      healthRows={data.healthRows}
      fleet={data.fleet}
      storage={data.storage}
      storageResponse={data.storageResponse}
    />
  );
};
