import { FlaskConical } from "lucide-react";
import { Link } from "react-router-dom";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { useExperiments } from "@/hooks/api/useExperiments";
import { EmptyState, ErrorState, LoadingState } from "@/pages/ide/coordinator/components/PageState";
import NewExperimentButton from "./components/NewExperimentButton";
import PageShell from "./components/PageShell";
import RefreshError from "./components/RefreshError";
import StateBadge from "./components/StateBadge";
import { useExperimentRoutes } from "./useExperimentRoutes";

/**
 * Every experiment in the workspace with its status. A failed read is an error
 * with a retry, never an empty list: a 503 means "could not look".
 */
export default function ExperimentsListPage() {
  const { data, isPending, error, refetch } = useExperiments();
  const routes = useExperimentRoutes();

  if (isPending)
    return (
      <PageShell>
        <LoadingState />
      </PageShell>
    );
  // With nothing cached the error is the page; with a cached list it is a banner over it.
  if (error && !data)
    return (
      <PageShell>
        <ErrorState message={error.message} onRetry={() => refetch()} />
      </PageShell>
    );

  const experiments = data ?? [];
  if (experiments.length === 0) {
    return (
      <PageShell>
        <EmptyState
          icon={FlaskConical}
          title='No experiments in this workspace'
          action={<NewExperimentButton />}
        />
      </PageShell>
    );
  }

  return (
    <div className='flex h-full flex-1 flex-col' data-testid='experiments-list'>
      {error && <RefreshError message={error.message} onRetry={() => refetch()} />}
      <div className='flex items-center justify-between gap-3 border-border border-b px-4 py-2.5'>
        <div>
          <h1 className='font-semibold text-base leading-tight'>Experiments</h1>
          <p className='text-muted-foreground text-xs'>
            Pre-registered: priced, run, guarded and reported.
          </p>
        </div>
        <NewExperimentButton />
      </div>
      <div className='flex-1 overflow-y-auto p-4'>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Experiment</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Owner</TableHead>
              <TableHead>Next switch</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {experiments.map((e) => (
              <TableRow key={e.name} data-testid={`experiment-row-${e.name}`}>
                <TableCell className='max-w-xs'>
                  <Link
                    to={routes.DETAIL(e.name)}
                    className='font-medium font-mono text-sm hover:underline'
                    data-testid={`experiment-link-${e.name}`}
                  >
                    {e.name}
                  </Link>
                  <p className='truncate text-muted-foreground text-xs'>{e.hypothesis}</p>
                </TableCell>
                <TableCell>
                  <StateBadge state={e.status} retrospective={e.retrospective} />
                </TableCell>
                <TableCell>{e.owner}</TableCell>
                <TableCell data-testid={`experiment-row-next-switch-${e.name}`}>
                  {e.next_switch_on ?? "—"}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
