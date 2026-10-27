import { Info } from "lucide-react";
import type { ReactElement, ReactNode } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";

/** Hovering (or focusing) `children` shows `hint`. */
export function Hinted({ hint, children }: { hint: ReactNode; children: ReactElement }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent className='max-w-xs'>{hint}</TooltipContent>
    </Tooltip>
  );
}

/** A small ⓘ beside a label, for a figure that is not itself a tile. */
export function HintIcon({ hint, testId }: { hint: ReactNode; testId: string }) {
  return (
    <Hinted hint={hint}>
      <button
        type='button'
        aria-label='What does this mean?'
        className='inline-flex cursor-help text-muted-foreground hover:text-foreground'
        data-testid={testId}
      >
        <Info className='size-3.5' />
      </button>
    </Hinted>
  );
}
