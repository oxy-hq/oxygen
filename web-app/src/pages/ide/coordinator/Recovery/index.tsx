import { AlertTriangle, CheckCircle, RefreshCw, XCircle } from "lucide-react";
import type React from "react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useRecoveryStats from "@/hooks/api/coordinator/useRecoveryStats";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import type { AgentStats, RecoveredRunEntry } from "@/services/api/coordinator";
import useCurrentOrg from "@/stores/useCurrentOrg";

// ── Stat card ───────────────────────────────────────────────────────────────

const StatCard: React.FC<{
  label: string;
  value: number;
  icon: React.ElementType;
  className?: string;
}> = ({ label, value, icon: Icon, className }) => (
  <div className='flex items-center gap-3 rounded-lg border border-border bg-card px-4 py-3'>
    <Icon className={cn("h-5 w-5", className)} />
    <div>
      <p className='font-semibold text-lg'>{value}</p>
      <p className='text-muted-foreground text-xs'>{label}</p>
    </div>
  </div>
);

// ── Agent breakdown table ───────────────────────────────────────────────────

const AgentTable: React.FC<{ agents: AgentStats[] }> = ({ agents }) => {
  const sorted = [...agents].sort((a, b) => b.total - a.total);

  if (sorted.length === 0) {
    return <p className='px-4 py-6 text-center text-muted-foreground text-sm'>No agent data</p>;
  }

  return (
    <div className='overflow-x-auto'>
      <table className='w-full text-sm'>
        <thead>
          <tr className='border-border border-b text-left text-muted-foreground'>
            <th className='px-4 py-2 font-medium'>Agent</th>
            <th className='px-4 py-2 text-right font-medium'>Total</th>
            <th className='px-4 py-2 text-right font-medium'>Succeeded</th>
            <th className='px-4 py-2 text-right font-medium'>Failed</th>
            <th className='px-4 py-2 text-right font-medium'>Recovered</th>
            <th className='px-4 py-2 text-right font-medium'>Failure Rate</th>
          </tr>
        </thead>
        <tbody>
          {sorted.map((a) => {
            const failRate = a.total > 0 ? ((a.failed / a.total) * 100).toFixed(1) : "0.0";
            return (
              <tr key={a.agent_id} className='border-border border-b hover:bg-muted/50'>
                <td className='px-4 py-2 font-mono text-xs'>{a.agent_id || "(unknown)"}</td>
                <td className='px-4 py-2 text-right'>{a.total}</td>
                <td className='px-4 py-2 text-right text-emerald-500'>{a.succeeded}</td>
                <td className='px-4 py-2 text-right text-destructive'>{a.failed}</td>
                <td className='px-4 py-2 text-right text-warning'>{a.recovered}</td>
                <td
                  className={cn(
                    "px-4 py-2 text-right",
                    Number.parseFloat(failRate) > 20 ? "text-destructive" : "text-muted-foreground"
                  )}
                >
                  {failRate}%
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
};

// ── Recovered runs list ─────────────────────────────────────────────────────

const RecoveredRunRow: React.FC<{ run: RecoveredRunEntry }> = ({ run }) => {
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const runTreeHref = ROUTES.ORG(orgSlug).WORKSPACE().IDE.COORDINATOR.RUN_DETAIL(run.run_id);

  return (
    <button
      type='button'
      className='flex w-full cursor-pointer items-center gap-4 border-border border-b px-4 py-2 text-left last:border-b-0 hover:bg-muted/50'
      onClick={() => navigate(runTreeHref)}
    >
      <span
        className={cn(
          "inline-flex items-center gap-1 font-medium text-xs",
          run.status === "done" ? "text-emerald-500" : "text-destructive"
        )}
      >
        {run.status === "done" ? (
          <CheckCircle className='h-3 w-3' />
        ) : (
          <XCircle className='h-3 w-3' />
        )}
        {run.status}
      </span>
      <div className='min-w-0 flex-1'>
        <p className='truncate text-sm'>{run.question}</p>
        <span className='text-muted-foreground text-xs'>{run.agent_id}</span>
      </div>
      <span className='text-warning text-xs'>attempt {run.attempt + 1}</span>
      <span className='font-mono text-muted-foreground text-xs'>{run.run_id.slice(0, 8)}</span>
    </button>
  );
};

// ── Page ────────────────────────────────────────────────────────────────────

const RecoveryPage: React.FC = () => {
  const { data, isPending, error, refetch } = useRecoveryStats();

  if (isPending) {
    return (
      <div className='flex h-full items-center justify-center'>
        <Spinner className='h-6 w-6' />
      </div>
    );
  }

  if (error) {
    return (
      <div className='flex h-full flex-col items-center justify-center gap-2'>
        <p className='text-destructive text-sm'>Failed to load recovery stats</p>
        <Button variant='outline' size='sm' onClick={() => refetch()}>
          Retry
        </Button>
      </div>
    );
  }

  if (!data) return null;

  return (
    <div className='flex h-full flex-col'>
      {/* Header */}
      <div className='flex items-center justify-between border-border border-b px-4 py-3'>
        <div>
          <h2 className='font-semibold text-base'>Recovery & Reliability</h2>
          <p className='text-muted-foreground text-xs'>Last {data.total_runs} root runs</p>
        </div>
        <Button variant='ghost' size='icon' onClick={() => refetch()} className='h-8 w-8'>
          <RefreshCw className='h-4 w-4' />
        </Button>
      </div>

      <div className='flex-1 overflow-y-auto'>
        {/* Stats cards */}
        <div className='grid grid-cols-4 gap-3 p-4'>
          <StatCard
            label='Succeeded'
            value={data.succeeded_count}
            icon={CheckCircle}
            className='text-emerald-500'
          />
          <StatCard
            label='Failed'
            value={data.failed_count}
            icon={XCircle}
            className='text-destructive'
          />
          <StatCard
            label='Recovered'
            value={data.recovered_count}
            icon={RefreshCw}
            className='text-warning'
          />
          <StatCard
            label='Cancelled'
            value={data.cancelled_count}
            icon={AlertTriangle}
            className='text-muted-foreground'
          />
        </div>

        {/* Agent breakdown */}
        <div className='px-4 pb-2'>
          <h3 className='mb-2 font-medium text-sm'>Agent Breakdown</h3>
          <div className='rounded-lg border border-border'>
            <AgentTable agents={data.agents} />
          </div>
        </div>

        {/* Recovered runs */}
        <div className='px-4 pt-4 pb-4'>
          <h3 className='mb-2 font-medium text-sm'>
            Recovered Runs ({data.recovered_runs.length})
          </h3>
          <div className='rounded-lg border border-border'>
            {data.recovered_runs.length === 0 ? (
              <p className='px-4 py-6 text-center text-muted-foreground text-sm'>
                No recovered runs in this window
              </p>
            ) : (
              data.recovered_runs.map((run) => <RecoveredRunRow key={run.run_id} run={run} />)
            )}
          </div>
        </div>
      </div>
    </div>
  );
};

export default RecoveryPage;
