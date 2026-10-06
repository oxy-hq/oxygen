import { AlertCircle, CheckCircle2 } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import type { Trace } from "@/services/api/traces";
import { formatDuration, formatTimeAgo, SpanIcon } from "../../utils";
import { deriveTraceRow } from "./traceRow";

interface TracesTableProps {
  traces: Trace[];
  onTraceClick: (traceId: string) => void;
  selectedIds: string[];
  onToggleSelect: (traceId: string) => void;
  /** Max reached — unselected rows can no longer be checked. */
  selectionFull: boolean;
}

/** Dense, scannable table view of traces (Theme 3d). */
export function TracesTable({
  traces,
  onTraceClick,
  selectedIds,
  onToggleSelect,
  selectionFull
}: TracesTableProps) {
  return (
    <div className='overflow-x-auto rounded-md border'>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className='w-8' />
            <TableHead className='w-8' />
            <TableHead>Trace</TableHead>
            <TableHead className='w-28'>Type</TableHead>
            <TableHead className='w-40'>Ref</TableHead>
            <TableHead className='w-24 text-right'>Duration</TableHead>
            <TableHead className='w-24 text-right'>Tokens</TableHead>
            <TableHead className='w-24 text-right'>Time</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {traces.map((trace) => {
            const row = deriveTraceRow(trace);
            const selected = selectedIds.includes(row.traceId);
            return (
              <TableRow
                key={row.traceId}
                className='cursor-pointer'
                onClick={() => onTraceClick(row.traceId)}
              >
                <TableCell className='align-middle' onClick={(e) => e.stopPropagation()}>
                  <Checkbox
                    checked={selected}
                    disabled={!selected && selectionFull}
                    onCheckedChange={() => onToggleSelect(row.traceId)}
                    aria-label='Select trace to compare'
                  />
                </TableCell>
                <TableCell className='align-middle'>
                  {row.isError ? (
                    <AlertCircle className='size-4 text-destructive' />
                  ) : (
                    // Grey, not green: almost every row is healthy, and a
                    // column of green ticks hides the red one.
                    <CheckCircle2 className='size-4 text-muted-foreground' />
                  )}
                </TableCell>
                <TableCell className='max-w-0'>
                  <div className='flex items-center gap-2'>
                    <SpanIcon
                      spanName={trace.spanName}
                      className='size-3.5 shrink-0 text-muted-foreground'
                    />
                    <span className='truncate font-medium text-sm'>{row.title}</span>
                  </div>
                </TableCell>
                <TableCell>
                  <Badge variant='outline' className='text-xs'>
                    {row.spanLabel}
                  </Badge>
                </TableCell>
                <TableCell className='truncate font-mono text-muted-foreground text-xs'>
                  {row.entityRef ?? "—"}
                </TableCell>
                <TableCell
                  className={cn(
                    "text-right text-sm tabular-nums",
                    row.isError && "text-destructive"
                  )}
                >
                  {formatDuration(row.durationMs)}
                </TableCell>
                <TableCell className='text-right text-muted-foreground text-sm tabular-nums'>
                  {row.tokensTotal ? row.tokensTotal.toLocaleString() : "—"}
                </TableCell>
                <TableCell className='text-right text-muted-foreground text-xs tabular-nums'>
                  {formatTimeAgo(row.timestamp)}
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}
