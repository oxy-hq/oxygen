import { useMemo } from "react";
import { useAppEventTypes } from "@/hooks/api/customApps/useAppEvents";
import { useAppIssues } from "@/hooks/api/customApps/useAppIssues";
import { useFleetStorage } from "@/hooks/api/customApps/useAppStorage";
import { useListedAvailability } from "@/hooks/api/customApps/useCustomApps";
import { AdminKpiStrip } from "@/pages/admin/components/AdminKpiStrip";
import { PRODUCTION } from "@/types/appEnvironments";
import type { CustomApp } from "@/types/apps";
import { Issues, IssuesBadge } from "../../AppDetail/components/Issues";
import { liveIssueCount } from "../../AppDetail/components/Issues/issueBrief";
import { AppLifecycle } from "../../AppLifecycle";
import { ConsolePanel } from "../../ConsolePanel";
import { formatBytes } from "../../StorageTab/utils";
import { type ConsolePageId, consolePagePath } from "../consolePages";
import { eventsView } from "../Events/eventsView";
import { PreviewCard } from "../PreviewCard";

const DAY_MINUTES = 24 * 60;

const percent = (ratio: number) =>
  ratio === 0 ? "0%" : ratio < 0.001 ? "<0.1%" : `${(ratio * 100).toFixed(ratio < 0.1 ? 1 : 0)}%`;

/**
 * The console's home: five numbers about production, then what is broken and what is
 * live. Everything else is a page away in the nav. Encore's environment overview is
 * the model: insights first, one chart's worth of detail, recent deploys beside it.
 *
 * The numbers are production's on purpose. Availability and issues only read
 * production, and a home page whose numbers silently follow a switch would read as
 * "fine" while a sandbox burns. The environment switch still rules every other page.
 */
export const OverviewPage = ({
  app,
  servedAt,
  onOpenFunction
}: {
  app: CustomApp;
  servedAt: string;
  onOpenFunction: (name: string) => void;
}) => {
  const { reading } = useListedAvailability(app);
  const issues = useAppIssues(app.id);
  const storage = useFleetStorage();
  const types = useAppEventTypes(app.org_slug);

  const day = reading?.windows.find((w) => w.window_minutes === DAY_MINUTES) ?? null;
  const live = issues.data ? liveIssueCount(issues.data.issues) : null;
  const storageRow = storage.data?.rows.find((r) => r.appId === app.id) ?? null;
  const dead = useMemo(
    () => (types.data ? eventsView(types.data.types, app.slug, PRODUCTION).totals.dead : null),
    [types.data, app.slug]
  );
  const link = (page: ConsolePageId) => consolePagePath(app, page);

  return (
    <div className='flex flex-col gap-3' data-testid='apps-console-overview'>
      <AdminKpiStrip
        data-testid='apps-console-kpis'
        items={[
          {
            id: "requests",
            label: "Requests",
            value: day ? day.total.toLocaleString() : null,
            caption: "last 24h, production",
            quiet: day?.total === 0,
            to: link("activity")
          },
          {
            id: "failure-rate",
            label: "Failure rate",
            value: day && day.failure_ratio !== null ? percent(day.failure_ratio) : null,
            caption: day
              ? `${day.failed.toLocaleString()} failed of ${day.total.toLocaleString()}`
              : "no traffic",
            tone: reading?.verdict === "burning" ? "danger" : undefined,
            quiet: day?.failure_ratio === 0,
            info: `Failed requests over all requests in the last 24 hours. The app burns its budget past a ${Math.round((reading?.objective ?? 0.99) * 100)}% availability objective.`,
            to: link("activity")
          },
          {
            id: "issues",
            label: "Issues",
            value: live === null ? null : live.toLocaleString(),
            caption: "on the live build, 7 days",
            tone: "danger",
            quiet: live === 0,
            to: link("activity")
          },
          {
            id: "dead-events",
            label: "Dead events",
            value: dead === null ? null : dead.toLocaleString(),
            caption: "deliveries out of attempts",
            tone: "danger",
            quiet: dead === 0,
            to: link("events")
          },
          {
            id: "storage",
            label: "Storage",
            value: storageRow ? formatBytes(storageRow.bytes) : null,
            caption: storageRow
              ? `${storageRow.objectCount.toLocaleString()} objects`
              : "not measured yet",
            quiet: storageRow?.bytes === 0,
            to: link("data")
          }
        ]}
      />

      <div className='grid items-start gap-3 xl:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]'>
        <ConsolePanel
          id='issues'
          title='Issues'
          scope={{ label: "Production" }}
          info='Production failures of the last 7 days, one per function and cause, the way the pager groups them.'
          actions={<IssuesBadge appId={app.id} />}
        >
          {/* The list runs edge to edge and owns its gutter. */}
          <div className='-mx-3 -mb-3'>
            <Issues app={app} onOpenFunction={onOpenFunction} />
          </div>
        </ConsolePanel>

        <div className='flex min-w-0 flex-col gap-3'>
          <ConsolePanel
            id='build'
            title='Live build'
            info='What production serves, and the moves that change it: promote, roll back, unpublish. Release history and retained builds are on Releases.'
          >
            <AppLifecycle app={app} />
          </ConsolePanel>
          <PreviewCard app={app} servedAt={servedAt} />
        </div>
      </div>
    </div>
  );
};
