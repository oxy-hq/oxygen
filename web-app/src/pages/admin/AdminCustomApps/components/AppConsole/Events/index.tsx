import { Radio, Search } from "lucide-react";
import { useMemo, useState } from "react";
import { Input } from "@/components/ui/shadcn/input";
import { useAppEventsDead, useAppEventTypes } from "@/hooks/api/customApps/useAppEvents";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { AdminKpiStrip } from "@/pages/admin/components/AdminKpiStrip";
import type { CustomApp } from "@/types/apps";
import { ConsolePanel } from "../../ConsolePanel";
import { DeadLetters } from "./DeadLetters";
import { EventTypesTable } from "./EventTypesTable";
import { eventsView, filterEventRows, formatWait } from "./eventsView";

/**
 * The app's events: what it publishes, what it listens to, and whether the queue
 * between them is moving. Encore's Pub/Sub page is the model: counts first, a type per
 * row, its listeners one click down, and dead letters grouped by cause.
 *
 * Read-only. Redelivery stays in `oxyc events` (each dead listener carries the command).
 */
export const EventsPage = ({
  app,
  environment,
  scope
}: {
  app: CustomApp;
  environment: string;
  scope: { label: string; warn?: boolean };
}) => {
  // The one page that polls: the queue moves, so its numbers should too.
  const types = useAppEventTypes(app.org_slug, { live: true });
  const dead = useAppEventsDead(app.org_slug, app.slug, environment);
  const [query, setQuery] = useState("");
  const [pendingOnly, setPendingOnly] = useState(false);
  const [deadOnly, setDeadOnly] = useState(false);

  const view = useMemo(
    () => (types.data ? eventsView(types.data.types, app.slug, environment) : null),
    [types.data, app.slug, environment]
  );
  const rows = useMemo(
    () => (view ? filterEventRows(view.rows, { query, pendingOnly, deadOnly }) : []),
    [view, query, pendingOnly, deadOnly]
  );
  const totals = view?.totals;
  const wait = formatWait(totals?.oldestPendingSeconds ?? null);

  return (
    <div className='flex flex-col gap-3' data-testid='admin-app-events'>
      <AdminKpiStrip
        data-testid='admin-app-events-kpis'
        items={[
          {
            id: "types",
            label: "Event types",
            value: view ? view.rows.length : null,
            caption: scope.label,
            quiet: view?.rows.length === 0
          },
          {
            id: "pending",
            label: "Pending",
            value: totals ? totals.pending.toLocaleString() : null,
            caption: wait ? `oldest waiting ${wait}` : "nothing waiting",
            quiet: !totals?.pending
          },
          {
            id: "retrying",
            label: "Retrying",
            value: totals ? totals.retrying.toLocaleString() : null,
            caption: "pending after a failed try",
            quiet: !totals?.retrying
          },
          {
            id: "dead",
            label: "Dead",
            value: totals ? totals.dead.toLocaleString() : null,
            caption: "out of attempts",
            tone: "danger",
            quiet: !totals?.dead
          },
          {
            id: "held",
            label: "Held",
            value: totals ? totals.held.toLocaleString() : null,
            caption: "events switch off",
            tone: "warn",
            quiet: !totals?.held,
            info: "A non-production environment holds what is published to it while its events switch is off, or while it is being deleted or is a sandbox idle past its expiry. Production never holds. A held delivery runs once it is redelivered."
          }
        ]}
      />

      <ConsolePanel
        id='events'
        title='Event types'
        scope={scope}
        info='Each type this app publishes or listens to. Counts are read live from the deliveries and re-read every 30 seconds. For a type this app publishes, every listener counts, whichever app it is in.'
        actions={
          <div className='flex items-center gap-1.5'>
            <Toggle
              pressed={pendingOnly}
              onChange={setPendingOnly}
              testId='admin-app-events-filter-pending'
            >
              Pending
            </Toggle>
            <Toggle pressed={deadOnly} onChange={setDeadOnly} testId='admin-app-events-filter-dead'>
              Dead letters
            </Toggle>
            <div className='relative max-sm:hidden'>
              <Search className='-translate-y-1/2 absolute top-1/2 left-2 size-3 text-muted-foreground' />
              <Input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder='Filter types or listeners'
                aria-label='Filter event types or listeners'
                className='h-7 w-48 pl-6 text-xs'
                data-testid='admin-app-events-search'
              />
            </div>
          </div>
        }
      >
        <div className='-m-3'>
          <AdminAsync
            query={types}
            noun="the org's event types"
            rows={3}
            className='m-3'
            isEmpty={() => (view?.rows.length ?? 0) === 0}
            empty={
              <AdminEmptyState
                icon={Radio}
                className='m-3'
                title='No events'
                description={`${app.name} publishes no events and listens to none in this environment.`}
                data-testid='admin-app-events-empty'
              />
            }
          >
            {() =>
              rows.length === 0 ? (
                <p
                  className='px-3 py-4 text-muted-foreground text-xs'
                  data-testid='admin-app-events-filtered-empty'
                >
                  No type matches these filters.
                </p>
              ) : (
                <EventTypesTable orgSlug={app.org_slug} rows={rows} />
              )
            }
          </AdminAsync>
        </div>
      </ConsolePanel>

      {/* Shown when there are dead letters to list, and when the read failed: a source
          that did not answer is a third outcome, not "nothing died", so its error and
          Retry stay on the page rather than the panel quietly disappearing. */}
      {dead.isError || (dead.data?.groups.length ?? 0) > 0 ? (
        <ConsolePanel
          id='events-dead'
          title='Dead letters'
          scope={scope}
          info="This app's listeners' deliveries that ran out of attempts, grouped by the outcome and first line of the error of their last attempt. Newest first."
        >
          <div className='-m-3 text-xs'>
            <AdminAsync query={dead} noun="this app's dead letters" rows={2} className='m-3'>
              {(data) => <DeadLetters dead={data} />}
            </AdminAsync>
          </div>
        </ConsolePanel>
      ) : null}
    </div>
  );
};

const Toggle = ({
  pressed,
  onChange,
  testId,
  children
}: {
  pressed: boolean;
  onChange: (next: boolean) => void;
  testId: string;
  children: string;
}) => (
  <button
    type='button'
    aria-pressed={pressed}
    onClick={() => onChange(!pressed)}
    data-testid={testId}
    className={cn(
      "h-7 rounded-md border px-2 text-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
      pressed
        ? "border-foreground/30 bg-foreground text-background"
        : "border-border/60 text-muted-foreground hover:text-foreground"
    )}
  >
    {children}
  </button>
);
