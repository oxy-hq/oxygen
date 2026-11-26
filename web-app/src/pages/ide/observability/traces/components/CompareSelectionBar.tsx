import { GitCompareArrows, X } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";

interface CompareSelectionBarProps {
  count: number;
  onCompare: () => void;
  onClear: () => void;
}

/** Contextual bar shown while selecting traces to compare (Theme 3e). */
export function CompareSelectionBar({ count, onCompare, onClear }: CompareSelectionBarProps) {
  if (count === 0) return null;
  return (
    <div className='mb-3 flex items-center gap-3 border border-primary/40 bg-primary/5 px-3 py-2'>
      <GitCompareArrows className='size-4 text-primary' />
      <span className='font-medium text-sm'>{count} of 2 selected to compare</span>
      <div className='ml-auto flex items-center gap-2'>
        <Button size='sm' onClick={onCompare} disabled={count !== 2}>
          Compare
        </Button>
        <Button size='sm' variant='ghost' onClick={onClear} className='gap-1'>
          <X className='size-3.5' />
          Clear
        </Button>
      </div>
    </div>
  );
}
