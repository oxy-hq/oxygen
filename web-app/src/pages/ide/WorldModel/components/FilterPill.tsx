import { AlertCircle, CheckCircle2, Loader2, X } from "lucide-react";
import type { WmFilterSeed } from "@/types/worldModel";

interface FilterPillProps {
  seed: WmFilterSeed;
  isCountLoading?: boolean;
  /** The filter-count stream failed: the counts on the cards are missing or partial. */
  countFailed?: boolean;
  /** The seed's instance-detail stream failed: the measure values on its card are
   *  missing or partial. */
  measuresFailed?: boolean;
  onClear: () => void;
}

const failureText = (countFailed: boolean, measuresFailed: boolean): string | null => {
  if (countFailed && measuresFailed) return "Failed to load counts and measures";
  if (countFailed) return "Failed to load counts";
  if (measuresFailed) return "Failed to load measures";
  return null;
};

export function FilterPill({
  seed,
  isCountLoading = false,
  countFailed = false,
  measuresFailed = false,
  onClear
}: FilterPillProps) {
  const failure = failureText(countFailed, measuresFailed);
  return (
    <div className='absolute top-3 left-1/2 z-10 flex -translate-x-1/2 items-center gap-2 border border-info/60 bg-card px-3 py-1 font-mono text-xs shadow-[0_4px_16px_rgba(0,0,0,0.4)]'>
      <span className='text-[9px] text-muted-foreground uppercase tracking-wider'>Filtered by</span>
      <span className='text-info'>{seed.label}</span>
      <span className='text-muted-foreground'>@</span>
      <span className='text-muted-foreground'>{seed.entityId}</span>
      <span className='text-muted-foreground'>·</span>
      <span className='text-muted-foreground'>{seed.keyValue}</span>
      <span className='mx-0.5 text-muted-foreground/40'>|</span>
      {isCountLoading ? (
        <Loader2 size={10} className='animate-spin text-info' />
      ) : failure ? (
        <span className='flex items-center gap-1 text-destructive'>
          <AlertCircle size={10} />
          {failure}
        </span>
      ) : (
        <CheckCircle2 size={10} className='text-success' />
      )}
      <button
        type='button'
        onClick={onClear}
        className='ml-0.5 flex h-4 w-4 items-center justify-center text-muted-foreground transition-colors hover:text-foreground'
        aria-label='Clear filter'
      >
        <X size={10} />
      </button>
    </div>
  );
}
