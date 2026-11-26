import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";

const PERIOD_PRESETS = [
  { days: 30, label: "Last 30d" },
  { days: 90, label: "Last 90d" },
  { days: 180, label: "Last 180d" },
  { days: 365, label: "Last 365d" }
] as const;

/** Short name of a `view.dim` time-dimension id, for a compact inline label. */
function shortTimeDim(id: string): string {
  return id.split(".").slice(1).join(".") || id;
}

/**
 * What the Opportunities scan was run over, and the two knobs that change it:
 * the trailing window, and (only when the view declares more than one) which
 * time dimension to compare across.
 *
 * The boundary line below the selectors is load-bearing copy, not decoration.
 * The scan walks THIS view's segmentable dimensions plus one hop through each
 * foreign entity — it is not exhaustive over the warehouse, and a reader who
 * believes it is reads a missing lever as evidence that no lever exists.
 */
export function ScanControls({
  view,
  periodDays,
  onPeriodDaysChange,
  timeDim,
  candidates,
  onTimeDimChange
}: {
  view: string;
  periodDays: number;
  onPeriodDaysChange: (days: number) => void;
  /** The resolved time dimension the scan actually ran over. */
  timeDim: string;
  /** Every time dimension declared on `view`. */
  candidates: string[];
  onTimeDimChange: (id: string) => void;
}) {
  return (
    <>
      <div className='flex items-center gap-1.5'>
        <Select value={String(periodDays)} onValueChange={(v) => onPeriodDaysChange(Number(v))}>
          <SelectTrigger size='sm' className='h-7 flex-1 font-mono text-xs'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            {PERIOD_PRESETS.map((p) => (
              <SelectItem key={p.days} value={String(p.days)} className='font-mono text-xs'>
                {p.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {candidates.length > 1 && (
          <Select value={timeDim} onValueChange={onTimeDimChange}>
            <SelectTrigger
              size='sm'
              aria-label='Time dimension'
              className='h-7 min-w-0 flex-1 font-mono text-xs'
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent className={DESIGN_REFERENCE}>
              {candidates.map((c) => (
                <SelectItem key={c} value={c} className='font-mono text-xs'>
                  {c}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>

      {/* With a single time dimension there is no selector, so name the one
          being compared over rather than leave it implicit. */}
      {candidates.length === 1 && (
        <p className='font-mono text-[9.5px] text-muted-foreground'>
          over <span className='text-foreground'>{shortTimeDim(timeDim)}</span> · scans dimensions
          on <span className='text-foreground'>{view}</span> and one join hop · top 5 shown
        </p>
      )}
      {candidates.length > 1 && (
        <p className='font-mono text-[9.5px] text-muted-foreground'>
          scans dimensions on <span className='text-foreground'>{view}</span> and one join hop · top
          5 shown
        </p>
      )}
    </>
  );
}
