import { Loader2, RefreshCw } from "lucide-react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import type { PreaggRollupStatus } from "@/services/api/semantic";
import { BuiltAt, CacheState, FieldChips, MeasureChips } from "./RollupStatus";

/** One rollup as a table row: what it covers, whether it's on disk, rebuild. */
export default function RollupRow({
  rollup,
  blobReads,
  rebuilding,
  onRebuild
}: {
  rollup: PreaggRollupStatus;
  /** Whether a rollup built on another node is readable from here. */
  blobReads: boolean;
  rebuilding: boolean;
  onRebuild: () => void;
}) {
  return (
    <TableRow>
      <TableCell className='whitespace-nowrap font-mono text-xs'>{rollup.view_name}</TableCell>
      <TableCell className='whitespace-nowrap font-medium'>{rollup.rollup_name}</TableCell>
      <TableCell>
        {rebuilding ? (
          <span className='flex items-center gap-1.5 whitespace-nowrap text-muted-foreground'>
            <Loader2 className='h-3.5 w-3.5 shrink-0 animate-spin' />
            Rebuilding…
          </span>
        ) : (
          <CacheState rollup={rollup} blobReads={blobReads} size='md' />
        )}
      </TableCell>
      <TableCell>
        <FieldChips items={rollup.dimensions} />
      </TableCell>
      <TableCell>
        <MeasureChips measures={rollup.measures} />
      </TableCell>
      <TableCell className='whitespace-nowrap font-mono text-xs'>
        {rollup.time_dimension ? (
          <>
            {rollup.time_dimension}
            {rollup.granularity && (
              <span className='ml-1 text-muted-foreground'>/ {rollup.granularity}</span>
            )}
          </>
        ) : (
          <span className='text-muted-foreground'>—</span>
        )}
      </TableCell>
      <TableCell className='whitespace-nowrap font-mono text-muted-foreground text-xs'>
        {rollup.refresh_key ?? <span className='text-muted-foreground'>—</span>}
      </TableCell>
      <TableCell className='whitespace-nowrap text-xs'>
        <BuiltAt rollup={rollup} />
      </TableCell>
      <TableCell className='text-right'>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant='ghost'
              size='icon'
              className='h-7 w-7'
              disabled={rebuilding}
              onClick={onRebuild}
              aria-label={`Rebuild ${rollup.view_name}.${rollup.rollup_name}`}
            >
              <RefreshCw className='h-3.5 w-3.5' />
            </Button>
          </TooltipTrigger>
          {/* Say that it ignores the refresh key — that is the whole reason to
              press it on a rollup the schedule already considers fresh. */}
          <TooltipContent className={DESIGN_REFERENCE} side='left'>
            Rebuild now, ignoring the refresh key
          </TooltipContent>
        </Tooltip>
      </TableCell>
    </TableRow>
  );
}
