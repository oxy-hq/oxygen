import { useMemo } from "react";
import { useLocation, useParams, useSearchParams } from "react-router-dom";
import { PRODUCTION } from "@/types/appEnvironments";
import type { CustomApp } from "@/types/apps";
import type { HealthIndex } from "../../appStatus";
import { consolePageFrom } from "../AppConsole/consolePages";
import { AppsColumn } from "./AppsColumn";
import { CollapseToggle } from "./CollapseToggle";
import { DetailPane, type DetailPaneData } from "./DetailPane";
import { ALL_ORGS, OrgsColumn } from "./OrgsColumn";
import { PagesColumn } from "./PagesColumn";
import { PathBar } from "./PathBar";
import { pathSegments } from "./pathSegments";
import { useBrowserSelection } from "./useBrowserSelection";
import { useRailCollapsed } from "./useRailCollapsed";
import { useRailFocus } from "./useRailFocus";

/**
 * `/admin/apps` as a Finder column browser: organizations, then their apps, then that
 * app's console pages, then whatever you selected.
 *
 * It replaces two separate ideas about navigation — a flat worst-first table of every app
 * on the deployment, and, once you had picked one, that app's own eight-entry sidebar.
 * The registry's real shape is org → app → page; the table flattened the first tier into
 * a text cell and the sidebar hid the third behind a selection. So moving from "acme's
 * store-ops Activity" to "mars's command-center Activity" cost a trip back to the list, a
 * scan of every org's apps mixed together, and two more clicks. Here all three tiers stay
 * on screen and that move is one ↓.
 *
 * **Below `lg` none of this renders.** Three columns need width, and a drill-down stack
 * would be a third navigation model to keep working for the narrowest case. At phone and
 * tablet width the surface is exactly what it was: the fleet table, and the console with
 * `ConsoleNav` above it.
 *
 * Related: `appBrowserModel.ts` for the two derived tiers and why they sort
 * alphabetically; `useBrowserSelection` for what selecting does to the URL;
 * `useColumnKeys` for the keyboard; `DetailPane` for what the pane shows and why the
 * table survived.
 */
export const AppBrowser = ({
  apps,
  selected,
  missingApp,
  health,
  data
}: {
  apps: CustomApp[];
  /** The app the route names, when the registry has it. */
  selected: CustomApp | null;
  /** `org/slug` the route named when the registry has no such app. */
  missingApp: string | null;
  health: HealthIndex | undefined;
  data: DetailPaneData;
}) => {
  const { consolePage } = useParams<{ consolePage?: string }>();
  const [params] = useSearchParams();
  const { search } = useLocation();
  const [collapsed, toggleCollapsed] = useRailCollapsed();

  const page = consolePageFrom(consolePage);
  const environment = params.get("env") ?? PRODUCTION;
  const selection = useBrowserSelection(apps, selected, health, page);

  // Collapsing buys working width for a console page, so it only applies when there is
  // one. With no app selected the pane holds the fleet table, and hiding the tiers would
  // leave the surface with no navigation at all.
  const railCollapsed = collapsed && Boolean(selected);
  const depths = useMemo(() => {
    const open = railCollapsed ? [] : [0, 1];
    return selected ? [...open, 2] : open;
  }, [railCollapsed, selected]);
  const { railRef, moveFocus } = useRailFocus(depths);

  return (
    <div className='flex min-h-0 flex-1 flex-col' data-testid='admin-apps-browser'>
      <div className='flex min-h-0 flex-1'>
        {/* **The rail yields before the pane does.** The tiers want ~624px and the pane
            is the working surface — at 1280px, a rail that refused to shrink left the
            console 400px and broke it. So the rail may shrink and scroll (the deepest
            tier is kept in view by `useRailFocus`) while the pane holds a floor, which
            is Finder's behaviour too: its ancestor columns scroll off to the left rather
            than squeezing what you are looking at. */}
        <div
          ref={railRef}
          className='flex min-h-0 min-w-0 overflow-x-auto max-lg:hidden'
          data-testid='admin-apps-browser-rail'
          data-collapsed={railCollapsed}
        >
          {!railCollapsed && (
            <>
              <OrgsColumn
                orgs={selection.orgs}
                all={selection.everything}
                selectedId={selection.scope.kind === "org" ? selection.scope.orgSlug : ALL_ORGS}
                onSelect={selection.selectOrg}
                onLeaveColumn={(delta) => moveFocus(0, delta)}
              />
              <AppsColumn
                apps={selection.scopedApps}
                health={health}
                scope={selection.scope}
                page={page}
                search={search}
                selectedKey={selection.highlightedKey}
                onSelect={selection.selectApp}
                onLeaveColumn={(delta) => moveFocus(1, delta)}
              />
            </>
          )}
          {selected && (
            <PagesColumn
              app={selected}
              page={page}
              environment={environment}
              search={search}
              onSelect={selection.selectPage}
              onLeaveColumn={(delta) => moveFocus(2, delta)}
            />
          )}
        </div>

        {/* The floor the rail yields to. Below it the console's own cards start clipping,
            and a cramped working surface is worse than a rail you have to scroll. */}
        <div className='flex min-h-0 min-w-2xl flex-1 flex-col'>
          <DetailPane
            scope={selection.scope}
            app={selected}
            apps={apps}
            missingApp={missingApp}
            data={data}
          />
        </div>
      </div>

      <div className='flex shrink-0 items-center border-t max-lg:hidden'>
        {selected && <CollapseToggle collapsed={railCollapsed} onToggle={toggleCollapsed} />}
        <PathBar segments={pathSegments(selection.scope, selected, page)} />
      </div>
    </div>
  );
};
