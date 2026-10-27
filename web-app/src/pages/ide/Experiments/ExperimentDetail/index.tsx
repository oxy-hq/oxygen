import { ArrowLeft } from "lucide-react";
import { Link, useParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useExperiments } from "@/hooks/api/useExperiments";
import { EmptyState, ErrorState, LoadingState } from "@/pages/ide/coordinator/components/PageState";
import PageShell from "@/pages/ide/Experiments/components/PageShell";
import RefreshError from "@/pages/ide/Experiments/components/RefreshError";
import StateBadge from "@/pages/ide/Experiments/components/StateBadge";
import { COLUMN } from "@/pages/ide/Experiments/ExperimentDetail/column";
import { useExperimentRoutes } from "@/pages/ide/Experiments/useExperimentRoutes";
import { viewForState } from "@/pages/ide/Experiments/viewForState";
import type { ExperimentListItem } from "@/types/experiment";
import DefinitionView from "./components/DefinitionView";
import ExperimentMenu from "./components/ExperimentMenu";
import OperationsView from "./components/OperationsView";
import PricingPanel from "./components/PricingPanel";
import RefusedView from "./components/RefusedView";
import RegistrableView from "./components/RegistrableView";
import ResultPane from "./components/ResultPane";

function DetailBody({ summary }: { summary: ExperimentListItem }) {
  switch (viewForState(summary.status)) {
    case "pricing":
      return <PricingPanel summary={summary} />;
    case "registrable":
      return <RegistrableView summary={summary} />;
    case "operations":
      return <OperationsView summary={summary} />;
    case "report":
      return <ResultPane summary={summary} />;
    case "refused":
      return <RefusedView summary={summary} />;
    default:
      return (
        <p className='text-muted-foreground text-sm' data-testid='experiment-unknown-state'>
          This experiment has a status this version of the app does not know:{" "}
          <code className='font-mono'>{summary.status}</code>. Nothing is shown for it.
        </p>
      );
  }
}

/**
 * One experiment. The body is whatever `viewForState` says its status is — the
 * only place a status becomes a view — and each body mounts only the queries it
 * is allowed to make, so a live run never reaches `/result`.
 */
export default function ExperimentDetailPage() {
  const { name = "" } = useParams<{ name: string }>();
  const routes = useExperimentRoutes();
  const { data, isPending, error, refetch } = useExperiments();

  if (isPending)
    return (
      <PageShell>
        <LoadingState />
      </PageShell>
    );
  // With nothing cached the error is the page; with cached data it is a banner, so
  // the mounted body (typed pricing rows, a pending request) survives a failed refetch.
  if (error && !data)
    return (
      <PageShell>
        <ErrorState message={error.message} onRetry={() => refetch()} />
      </PageShell>
    );

  const summary = (data ?? []).find((e) => e.name === name);
  if (!summary) {
    return (
      <PageShell>
        <EmptyState
          title='Experiment not found'
          hint='It may have been removed, or it may not exist in this workspace.'
          action={
            <Button asChild size='sm' variant='outline'>
              <Link to={routes.ROOT}>Back to experiments</Link>
            </Button>
          }
        />
      </PageShell>
    );
  }

  const reportFirst = viewForState(summary.status) === "report";
  return (
    <div
      className='flex h-full flex-1 flex-col'
      data-testid='experiment-detail'
      data-status={summary.status}
      data-view={viewForState(summary.status)}
    >
      <div className='flex items-center gap-3 border-border border-b px-4 py-2.5'>
        <Button asChild variant='ghost' size='icon' className='h-8 w-8'>
          <Link to={routes.ROOT} aria-label='Back to experiments'>
            <ArrowLeft className='h-4 w-4' />
          </Link>
        </Button>
        <div className='min-w-0'>
          <h2 className='truncate font-mono font-semibold text-base leading-tight'>
            {summary.name}
          </h2>
          <p className='truncate text-muted-foreground text-xs'>
            {summary.hypothesis} · owner {summary.owner}
          </p>
        </div>
        <StateBadge state={summary.status} retrospective={summary.retrospective} />
        <div className='ml-auto'>
          <ExperimentMenu summary={summary} />
        </div>
      </div>
      {error && <RefreshError message={error.message} onRetry={() => refetch()} />}
      <div className='flex-1 space-y-4 overflow-y-auto p-4'>
        {/* A report is what the reader came for, so it leads; the definition follows it. */}
        {reportFirst && <DetailBody summary={summary} />}
        <div className={COLUMN}>
          <DefinitionView summary={summary} />
        </div>
        {!reportFirst && <DetailBody summary={summary} />}
      </div>
    </div>
  );
}
