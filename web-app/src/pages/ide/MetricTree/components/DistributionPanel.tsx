import { useMemo, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useDistributionQuery, useMetricTree, useTimeDimensions } from "@/hooks/api/useMetricTree";
import type { DistributionRequest } from "@/types/metricTree";
import DistributionGraph from "./DistributionGraph";

interface DistributionPanelProps {
  /** The selected measure, or `null` when nothing is selected. */
  measureId: string | null;
}

function firstOfMonthOffset(monthsBack: number): string {
  const d = new Date();
  d.setUTCDate(1);
  d.setUTCMonth(d.getUTCMonth() - monthsBack);
  return d.toISOString().slice(0, 10);
}

function lastOfMonthOffset(monthsBack: number): string {
  const d = new Date();
  d.setUTCDate(1);
  d.setUTCMonth(d.getUTCMonth() - monthsBack + 1);
  d.setUTCDate(0);
  return d.toISOString().slice(0, 10);
}

/**
 * Renders the selected measure's distribution for a single period. Calls
 * `/semantic/metric-tree/distribution`; the backend auto-derives the
 * baseline window so the structural decomposition has signal to walk.
 * Time-dimension candidates come from `/time-dimensions` (every `date` /
 * `datetime` dimension declared on the measure's view).
 */
export function DistributionPanel({ measureId }: DistributionPanelProps) {
  const { data: tree } = useMetricTree();
  const { data: timeDims, isPending: timeDimsLoading } = useTimeDimensions();

  const selectedNode = useMemo(
    () => (measureId ? tree?.nodes.find((n) => n.id === measureId) : null),
    [tree, measureId]
  );
  const candidates = useMemo<string[]>(() => {
    if (!selectedNode || !timeDims) return [];
    return timeDims.by_view[selectedNode.view] ?? [];
  }, [selectedNode, timeDims]);

  const [timeDimOverride, setTimeDimOverride] = useState<string | null>(null);
  const timeDim =
    timeDimOverride && candidates.includes(timeDimOverride)
      ? timeDimOverride
      : (candidates[0] ?? "");

  const [periodStart, setPeriodStart] = useState(() => firstOfMonthOffset(1));
  const [periodEnd, setPeriodEnd] = useState(() => lastOfMonthOffset(1));
  const periodValid = !!periodStart && !!periodEnd && periodStart <= periodEnd;

  const request = useMemo<DistributionRequest | null>(() => {
    if (!measureId || !timeDim || !periodValid) return null;
    return {
      target: measureId,
      time_dimension: timeDim,
      period: [periodStart, periodEnd]
    };
  }, [measureId, timeDim, periodValid, periodStart, periodEnd]);

  const distribution = useDistributionQuery(request, !!request);

  if (!measureId) {
    return (
      <p className='p-4 text-muted-foreground text-sm'>
        Select a measure in the graph to see its distribution.
      </p>
    );
  }

  return (
    <div className='flex h-full min-h-0 flex-col gap-3 p-4'>
      <div className='flex flex-col gap-1'>
        <Label htmlFor='dist-time-dim' className='text-muted-foreground text-xs'>
          Time dimension
        </Label>
        <Select
          value={timeDim}
          onValueChange={(v) => setTimeDimOverride(v)}
          disabled={candidates.length === 0}
        >
          <SelectTrigger id='dist-time-dim' size='sm' className='h-8 text-xs'>
            <SelectValue placeholder={timeDimsLoading ? "Loading…" : "Select…"} />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            {candidates.map((c) => (
              <SelectItem key={c} value={c} className='text-xs'>
                {c}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className='flex flex-col gap-2'>
        <Label className='text-muted-foreground text-xs'>Period</Label>
        <div className='grid grid-cols-2 gap-2'>
          <DateField
            id='dist-period-start'
            value={periodStart}
            onChange={setPeriodStart}
            label='Start'
          />
          <DateField id='dist-period-end' value={periodEnd} onChange={setPeriodEnd} label='End' />
        </div>
        {!periodValid && (
          <p className='text-destructive text-xs'>Start must be on or before end.</p>
        )}
      </div>

      {!request && periodValid && !timeDimsLoading && candidates.length === 0 && (
        <p className='text-muted-foreground text-xs'>
          No date or datetime dimension declared on this measure's view.
        </p>
      )}

      {request && distribution.isPending && (
        <div className='flex h-32 items-center justify-center'>
          <Spinner />
        </div>
      )}

      {request && distribution.error && (
        <p className='text-destructive text-sm'>
          {distribution.error instanceof Error
            ? distribution.error.message
            : "Failed to load the distribution."}
        </p>
      )}

      {request && distribution.data && (
        <div className='flex min-h-0 flex-1 flex-col gap-2'>
          <p className='text-muted-foreground text-xs'>
            <span className='font-medium text-foreground'>{distribution.data.target}</span> —{" "}
            <span className='t-code text-foreground'>
              {formatNumber(distribution.data.target_current)}
            </span>
          </p>
          <div className='min-h-0 flex-1 overflow-hidden border border-border bg-muted/20'>
            <DistributionGraph result={distribution.data} height='fill' />
          </div>
        </div>
      )}
    </div>
  );
}

interface DateFieldProps {
  id: string;
  label: string;
  value: string;
  onChange: (v: string) => void;
}

function DateField({ id, label, value, onChange }: DateFieldProps) {
  return (
    <div className='flex flex-col gap-1'>
      <Label htmlFor={id} className='text-muted-foreground text-xs'>
        {label}
      </Label>
      <input
        id={id}
        type='date'
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className='h-8 border border-input bg-transparent px-2 text-xs shadow-xs'
      />
    </div>
  );
}

function formatNumber(n: number): string {
  if (Math.abs(n) >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (Math.abs(n) >= 1_000) return `${(n / 1_000).toFixed(2)}k`;
  return n.toFixed(2);
}
