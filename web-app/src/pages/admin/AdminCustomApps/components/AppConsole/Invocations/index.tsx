import { Info } from "lucide-react";
import { useMemo, useState } from "react";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { useAppFunctions } from "@/hooks/api/customApps/useAppFunctions";
import { useAppInvocations } from "@/hooks/api/customApps/useAppInvocations";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { AppInvocationQuery, CustomApp } from "@/types/apps";
import { InvocationRow } from "../../AppDetail/components/Functions/InvocationRow";
import { environmentLabel } from "../../AppDetail/components/Secrets/environmentLabel";
import { ConsolePanel } from "../../ConsolePanel";
import { type InvocationFilters, InvocationsFilters } from "./InvocationsFilters";
import { InvocationsSummary } from "./InvocationsSummary";
import {
  comparePeriods,
  INVOCATION_CAP,
  modeOptions,
  periodCount,
  WINDOWS,
  type WindowKey,
  windowRange
} from "./invocationStats";
import { LatencyStrip } from "./LatencyStrip";

/** Rows rendered before "Show more"; the page itself holds up to the cap. */
const PAGE = 25;

/**
 * Every function's invocations in the console's environment, over a window — the
 * question the Functions panel answered one function at a time.
 *
 * Filters go to the server and are applied before its cap, so "Failed" over a week
 * is the newest 200 failures, not the failures among the newest 200 calls. The
 * counts are the server's over the whole window; the latency strip and the list
 * are the page, and say so when the window holds more than it.
 */
export const InvocationsPanel = ({
  app,
  environment,
  scope
}: {
  app: CustomApp;
  /** The console's environment; production included, it is always sent. */
  environment: string;
  scope: { label: string; warn?: boolean };
}) => {
  const [windowKey, setWindowKey] = useState<WindowKey>("24h");
  // The window ends when it was last read, not at every render: a moving `until`
  // would be a new query key, and a new request, each time anything re-rendered.
  const [anchor, setAnchor] = useState(() => Date.now());
  const [filters, setFilters] = useState<InvocationFilters>({ fn: "", outcome: "all", mode: "" });
  const range = useMemo(() => windowRange(windowKey, anchor), [windowKey, anchor]);
  const [visible, setVisible] = useState(PAGE);

  const narrowed: AppInvocationQuery = {
    environment,
    function: filters.fn || undefined,
    mode: filters.mode || undefined,
    outcome: filters.outcome === "failed" ? "failed" : undefined
  };
  const current = useAppInvocations(app.id, {
    ...narrowed,
    since: range.since,
    until: range.until,
    limit: INVOCATION_CAP
  });
  // Only its count is read: one row, and the server's count of the window.
  const previous = useAppInvocations(app.id, {
    ...narrowed,
    since: range.previousSince,
    until: range.previousUntil,
    limit: 1
  });
  const functions = useAppFunctions(app.id, environment);
  const where = environmentLabel(environment).toLowerCase();
  const windowLabel = WINDOWS[windowKey].label;
  const filtered = !!(filters.fn || filters.mode || filters.outcome === "failed");

  const pickWindow = (next: WindowKey) => {
    setWindowKey(next);
    setAnchor(Date.now());
  };

  return (
    <ConsolePanel
      id='invocations'
      title='Invocations'
      info='Every function call in this environment over the window, newest first, from any trigger: a request, a schedule, an event, a check, a Run.'
      scope={scope}
      actions={
        <ToggleGroup
          type='single'
          size='sm'
          value={windowKey}
          onValueChange={(v) => v && pickWindow(v as WindowKey)}
          className='gap-0 border border-border/60 p-0.5'
          aria-label='Window'
          data-testid='admin-app-invocations-window'
        >
          {(Object.keys(WINDOWS) as WindowKey[]).map((key) => (
            <ToggleGroupItem
              key={key}
              value={key}
              className='h-5 px-1.5 text-[11px] data-[state=on]:bg-muted'
            >
              {key}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      }
    >
      <div className='flex flex-col gap-3' data-testid='admin-app-invocations'>
        <InvocationsFilters
          value={filters}
          onChange={(next) => {
            setFilters(next);
            setVisible(PAGE);
          }}
          functions={(functions.data ?? []).map((f) => f.name)}
          modes={modeOptions(current.data?.invocations ?? [], filters.mode || undefined)}
          asOf={new Date(anchor)}
          onRefresh={() => setAnchor(Date.now())}
          refreshing={current.isFetching}
        />
        <AdminAsync
          query={current}
          noun='this app&rsquo;s invocations'
          rows={4}
          isEmpty={(data) => data.invocations.length === 0}
          empty={
            <p className='text-muted-foreground text-xs' data-testid='admin-app-invocations-empty'>
              No invocations {filtered ? "match these filters " : ""}in {where} in the last{" "}
              {windowLabel}.
            </p>
          }
        >
          {(data) => {
            const total = data.window?.total ?? null;
            // A full page is the newest N of more, unless the server's count says
            // the window held exactly N.
            const capped =
              data.invocations.length >= INVOCATION_CAP &&
              (total == null || total > INVOCATION_CAP);
            const comparison = comparePeriods(
              periodCount(data, INVOCATION_CAP),
              previous.data ? periodCount(previous.data, 1) : { total: 0, failed: 0, atLeast: true }
            );
            return (
              <>
                <InvocationsSummary
                  comparison={comparison}
                  windowLabel={windowLabel}
                  previousPending={!previous.data}
                />
                <LatencyStrip rows={data.invocations} total={total} capped={capped} />
                <ul className='flex flex-col' data-testid='admin-app-invocations-list'>
                  {data.invocations.slice(0, visible).map((iv) => (
                    <InvocationRow key={iv.id} appId={app.id} iv={iv} appRef={app} showFunction />
                  ))}
                </ul>
                {data.invocations.length > visible && (
                  <button
                    type='button'
                    onClick={() => setVisible((v) => v + PAGE)}
                    className='self-start text-muted-foreground text-xs underline underline-offset-2 hover:text-foreground'
                  >
                    Show more ({data.invocations.length - visible} older on this page)
                  </button>
                )}
                {capped && (
                  <p
                    className='flex items-start gap-1.5 bg-muted/40 px-3 py-2 text-xs'
                    data-testid='admin-app-invocations-capped'
                  >
                    <Info className='mt-0.5 size-3 shrink-0 text-muted-foreground' aria-hidden />
                    <span>
                      Listing the newest{" "}
                      <span className='font-medium tabular-nums'>{data.invocations.length}</span>
                      {total != null ? (
                        <>
                          {" "}
                          of{" "}
                          <span className='font-medium tabular-nums'>{total.toLocaleString()}</span>
                        </>
                      ) : null}{" "}
                      in this window. Narrow it by function, outcome or mode to reach further back.
                    </span>
                  </p>
                )}
              </>
            );
          }}
        </AdminAsync>
      </div>
    </ConsolePanel>
  );
};
