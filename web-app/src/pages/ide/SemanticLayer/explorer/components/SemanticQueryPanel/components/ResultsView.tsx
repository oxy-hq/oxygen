import { Clock, Zap } from "lucide-react";
import type { ReactNode } from "react";
import SqlResultsTable from "@/components/sql/SqlResultsTable";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";

interface ResultsViewProps {
  result?: string[][];
  resultFile?: string;
  executionError: string | null;
  loading?: boolean;
  loadingIndicator?: ReactNode;
  isPreagg?: boolean;
  executionTime?: number | null;
}

const formatDuration = (ms: number): string => {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
};

const ResultsView = ({
  result,
  resultFile,
  executionError,
  loading,
  loadingIndicator,
  isPreagg,
  executionTime
}: ResultsViewProps) => {
  if (loading) {
    return (
      <div className='flex h-full flex-col items-center justify-center gap-2'>
        {loadingIndicator ?? <Spinner className='size-6 text-muted-foreground' />}
      </div>
    );
  }

  if (executionError) {
    return (
      <div className='h-full overflow-auto p-4' data-testid='semantic-query-error'>
        <ErrorAlert>
          <div className='whitespace-pre-wrap font-mono text-xs'>{executionError}</div>
        </ErrorAlert>
      </div>
    );
  }

  return (
    <div className='flex h-full min-h-0 flex-col'>
      {(isPreagg || executionTime != null) && (
        <div className='flex shrink-0 items-center gap-3 border-b px-3 py-1.5'>
          {isPreagg && (
            <Tooltip>
              <TooltipTrigger asChild>
                <span className='flex items-center gap-1 text-primary text-xs'>
                  <Zap className='h-3 w-3' />
                  Using pre-aggregation
                </span>
              </TooltipTrigger>
              <TooltipContent side='bottom'>
                Results served from pre-aggregated cache
              </TooltipContent>
            </Tooltip>
          )}
          {executionTime != null && (
            <span
              className='flex items-center gap-1 text-muted-foreground text-xs'
              data-testid='semantic-query-execution-time'
            >
              <Clock className='h-3 w-3' />
              {formatDuration(executionTime)}
            </span>
          )}
        </div>
      )}
      <div className='min-h-0 flex-1' data-testid='semantic-query-results'>
        <SqlResultsTable result={result} resultFile={resultFile} />
      </div>
    </div>
  );
};

export default ResultsView;
