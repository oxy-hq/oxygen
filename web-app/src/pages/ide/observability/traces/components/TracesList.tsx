import { AlertCircle } from "lucide-react";
import { Card } from "@/components/ui/shadcn/card";
import { Spinner } from "@/components/ui/shadcn/spinner";
import type { Trace } from "@/services/api/traces";
import { MAX_COMPARE } from "../constants";
import type { TraceView } from "../types";
import { TraceCard } from "./TraceCard";
import { TracesTable } from "./TracesTable";

interface TracesListProps {
  isLoading: boolean;
  traces: Trace[] | undefined;
  searchQuery: string;
  onTraceClick: (traceId: string) => void;
  view: TraceView;
  selectedIds: string[];
  onToggleSelect: (traceId: string) => void;
}

function TracesList({
  isLoading,
  traces,
  searchQuery,
  onTraceClick,
  view,
  selectedIds,
  onToggleSelect
}: TracesListProps) {
  if (isLoading) {
    return (
      <div className='flex h-64 items-center justify-center'>
        <Spinner className='size-8 text-muted-foreground' />
      </div>
    );
  }

  if (!traces || traces.length === 0) {
    return (
      <Card className='p-12 text-center'>
        <div className='flex flex-col items-center gap-2'>
          <AlertCircle className='h-12 w-12 text-muted-foreground' />
          <h3 className='font-semibold text-lg'>No traces found</h3>
          <p className='text-muted-foreground text-sm'>
            {searchQuery
              ? "Try adjusting your search or filters"
              : "Start running agents to see traces here. A run can take up to 30 seconds to appear."}
          </p>
        </div>
      </Card>
    );
  }

  const selectionFull = selectedIds.length >= MAX_COMPARE;

  if (view === "table") {
    return (
      <TracesTable
        traces={traces}
        onTraceClick={onTraceClick}
        selectedIds={selectedIds}
        onToggleSelect={onToggleSelect}
        selectionFull={selectionFull}
      />
    );
  }

  return (
    <div className='space-y-2'>
      {traces.map((trace) => (
        <TraceCard
          key={trace.traceId}
          trace={trace}
          onClick={() => onTraceClick(trace.traceId)}
          selected={selectedIds.includes(trace.traceId)}
          selectDisabled={selectionFull}
          onToggleSelect={() => onToggleSelect(trace.traceId)}
        />
      ))}
    </div>
  );
}

export default TracesList;
