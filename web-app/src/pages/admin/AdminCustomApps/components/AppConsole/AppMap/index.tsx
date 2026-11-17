import { useMemo, useRef } from "react";
import { useAppFunctions } from "@/hooks/api/customApps/useAppFunctions";
import { useAppSecrets } from "@/hooks/api/customApps/useAppSecrets";
import { useRecentTraces } from "@/hooks/api/customApps/useRecentTraces";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { ConsolePanel } from "../../ConsolePanel";
import { layoutAppMap, MIN_DIAGRAM_WIDTH } from "./appMapLayout";
import { type AppMap, buildAppMap } from "./appMapModel";
import { AppMapDiagram } from "./components/AppMapDiagram";
import { AppMapFreshness } from "./components/AppMapFreshness";
import { AppMapList } from "./components/AppMapList";
import { useElementWidth } from "./useElementWidth";
import { useSeenOnce } from "./useSeenOnce";

/**
 * The app as a diagram, for the environment the console's switch names: what starts
 * each function (routes, schedules, webhooks, Airway steps), the functions, and what
 * they reach — the warehouse destinations the manifest declares, and every table,
 * host, store, message and secret the latest recorded runs called.
 *
 * The left and middle are the Functions panel's own read (same query, same cache
 * entry); the right is one bounded read of each function's last few traces, held
 * until the panel is near the viewport. Clicking a function opens it in Functions.
 */
export const AppMapPanel = ({
  appId,
  environment,
  scope,
  selected,
  onOpenFunction
}: {
  appId: string;
  /** The console's environment (`production` when the switch is at rest). */
  environment: string;
  scope: { label: string; warn?: boolean };
  /** `?fn=` — held lit on the map. */
  selected: string | null;
  onOpenFunction: (name: string) => void;
}) => {
  const frame = useRef<HTMLDivElement>(null);
  const seen = useSeenOnce(frame);
  const functions = useAppFunctions(appId, environment);
  const traces = useRecentTraces(appId, environment, seen);
  const secrets = useAppSecrets(appId, environment);
  const map = useMemo(
    () =>
      buildAppMap({
        functions: functions.data ?? [],
        traces: traces.data,
        secrets: secrets.data?.entries
      }),
    [functions.data, traces.data, secrets.data]
  );

  return (
    <ConsolePanel
      id='app-map'
      title='App map'
      question='Q1 — what starts it, what it touches'
      scope={scope}
    >
      <div ref={frame} className='min-w-0' data-testid='admin-app-map'>
        <AdminAsync
          query={functions}
          noun='this app&rsquo;s functions'
          rows={3}
          isEmpty={(fns) => fns.length === 0}
          empty={
            <p className='text-muted-foreground text-xs' data-testid='admin-app-map-empty'>
              This build ships no Oxy Functions, so there is nothing to map.
            </p>
          }
        >
          {() => (
            <div className='flex flex-col gap-3'>
              <AppMapFreshness
                freshness={map.freshness}
                isError={traces.isError}
                error={traces.error}
                onRetry={() => traces.refetch()}
                untracedSecrets={map.untracedSecrets}
              />
              <MapBody map={map} selected={selected} onOpen={onOpenFunction} />
            </div>
          )}
        </AdminAsync>
      </div>
    </ConsolePanel>
  );
};

/** The diagram when the column is wide enough for three, with the list behind a
 *  disclosure; the list alone when it is not. The width is the scroller's own, so a
 *  scrollbar it grows is room the diagram is laid out without. */
const MapBody = ({
  map,
  selected,
  onOpen
}: {
  map: AppMap;
  selected: string | null;
  onOpen: (name: string) => void;
}) => {
  const scroller = useRef<HTMLDivElement>(null);
  const width = useElementWidth(scroller);
  const layout = useMemo(
    () => (width >= MIN_DIAGRAM_WIDTH ? layoutAppMap(map, width) : null),
    [map, width]
  );
  return (
    <>
      {/* Bounded: an app of fifty functions is a map 1,750px tall, and the Functions
          panel under it should not move that far down the page. */}
      <div
        ref={scroller}
        className='max-h-160 overflow-y-auto'
        data-testid='admin-app-map-scroller'
      >
        {layout ? (
          <AppMapDiagram map={map} layout={layout} selected={selected} onOpenFunction={onOpen} />
        ) : (
          <AppMapList map={map} onOpenFunction={onOpen} />
        )}
      </div>
      {layout ? (
        <details className='text-xs' data-testid='admin-app-map-as-list'>
          <summary className='cursor-pointer text-muted-foreground hover:text-foreground'>
            Read as a list
          </summary>
          <div className='pt-2'>
            <AppMapList map={map} onOpenFunction={onOpen} />
          </div>
        </details>
      ) : null}
    </>
  );
};
