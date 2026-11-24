import { AppMark } from "@/components/apps/AppMark";
import type { CustomApp } from "@/types/apps";
import { appKey, type BrowserScope } from "../../appBrowserModel";
import { type HealthIndex, statusOf } from "../../appStatus";
import type { ConsolePageId } from "../AppConsole/consolePages";
import { consolePagePath } from "../AppConsole/consolePages";
import { ColumnPane } from "./ColumnPane";
import { StatusDot } from "./StatusDot";
import type { SelectHow } from "./useColumnKeys";

/**
 * The second tier: the apps in scope — one org's, or every one.
 *
 * **A row keeps the page you are on.** The href of every app points at the *current*
 * console page, so moving down this column while Activity is open walks you through
 * every app's Activity. That is the trip this redesign exists to shorten: it used to
 * cost going back to a flat list, finding the app, opening it, and finding the page
 * again.
 *
 * **Under All apps a row carries its org, under one org it does not.** A name does not
 * identify an app — `oxy-starter` exists under two of them — so the ambiguity
 * `AppIdentity` makes unrepresentable elsewhere must not reappear here. Inside one org
 * the slug would be the same word on every row, which is noise, so it is dropped.
 */
export const AppsColumn = ({
  apps,
  health,
  scope,
  page,
  search,
  selectedKey,
  onSelect,
  onLeaveColumn
}: {
  /** Already scoped and ordered by `appsInScope`. */
  apps: readonly CustomApp[];
  health: HealthIndex | undefined;
  scope: BrowserScope;
  page: ConsolePageId | null;
  search: string;
  selectedKey: string | null;
  onSelect: (id: string, how: SelectHow) => void;
  onLeaveColumn: (delta: -1 | 1) => void;
}) => (
  <ColumnPane
    id='apps'
    label={scope.kind === "all" ? "Apps" : `${scope.orgSlug} apps`}
    depth={1}
    width='w-60 2xl:w-72'
    selectedId={selectedKey}
    empty={
      scope.kind === "all"
        ? "No custom apps on this deployment yet."
        : "This org has no custom apps."
    }
    onSelect={onSelect}
    onLeaveColumn={onLeaveColumn}
    rows={apps.map((app) => ({
      id: appKey(app),
      label: app.name,
      href: consolePagePath(app, page ?? "overview", search),
      children: (
        <>
          <AppMark iconUrl={app.icon_url} name={app.name} size='sm' />
          <span className='min-w-0 flex-1 truncate' title={app.name}>
            {app.name}
          </span>
          {scope.kind === "all" && (
            <span
              className='max-w-20 shrink-0 truncate font-mono text-[10px] opacity-60 2xl:max-w-28'
              title={app.org_slug}
            >
              {app.org_slug}
            </span>
          )}
          <StatusDot status={statusOf(app, health)} />
        </>
      )
    }))}
  />
);
