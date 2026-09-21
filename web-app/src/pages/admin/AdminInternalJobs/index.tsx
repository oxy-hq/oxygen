import { useQueryClient } from "@tanstack/react-query";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useQueueStats } from "@/hooks/api/internalJobs";
import queryKeys from "@/hooks/api/queryKey";
import { AdminAsync } from "../components/AdminAsync";
import { AdminPage } from "../components/AdminPage";
import { HealthRibbon } from "./components/HealthRibbon";
import { JobsConsole } from "./components/JobsConsole";
import { LiveIndicator } from "./components/LiveIndicator";
import { ScheduledJobsPanel } from "./components/ScheduledJobsPanel";
import { WorkerFleetPanel } from "./components/WorkerFleetPanel";
import { LiveProvider, useLive } from "./LiveContext";
import { useInternalJobsHistory } from "./useInternalJobsHistory";

/**
 * Internal Jobs operator console — Oxy-staff cockpit for the agentic task
 * queue and worker fleet. NOT the customer-facing Orchestrator UI.
 *
 * The 2026-06 cockpit pass inverts the old emphasis: realtime charts are
 * demoted to a single compact health ribbon, and the detailed jobs console
 * (failed/dead jobs, each drillable into a full debug panel with the
 * workspace / org / user / error / decoded spec) becomes the centerpiece —
 * because "which job broke, whose was it, and why" matters more here than a
 * live graph.
 */
export default function AdminInternalJobsPage() {
  return (
    <LiveProvider>
      <PageBody />
    </LiveProvider>
  );
}

function PageBody() {
  const { paused, togglePaused } = useLive();
  const qc = useQueryClient();
  const queueStats = useQueueStats({ paused });
  const history = useInternalJobsHistory(queueStats.data, queueStats.dataUpdatedAt);

  const onRefresh = () => {
    qc.invalidateQueries({ queryKey: queryKeys.internalJobs.all });
  };

  return (
    <AdminPage
      width='wide'
      actions={
        <LiveIndicator
          updatedAt={queueStats.dataUpdatedAt || undefined}
          paused={paused}
          onTogglePaused={togglePaused}
          onRefresh={onRefresh}
          isFetching={queueStats.isFetching}
        />
      }
      data-testid='admin-internal-jobs'
    >
      {/* Four independent regions, four gates: the ribbon, the console, the fleet
          and the schedule each poll their own endpoint, so one being down must not
          blank the other three. */}
      <AdminAsync
        query={queueStats}
        noun='queue stats'
        skeleton={<Skeleton className='h-16 w-full' />}
      >
        {(stats) => <HealthRibbon total={stats.total} history={history} />}
      </AdminAsync>

      <JobsConsole />

      <div className='grid gap-5 lg:grid-cols-2'>
        <WorkerFleetPanel />
        <ScheduledJobsPanel />
      </div>
    </AdminPage>
  );
}
