import { Info } from "lucide-react";
import type { ReactNode } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { cn } from "@/libs/shadcn/utils";

/**
 * The ⓘ beside a heading, a column or a number: where an explanation lives.
 *
 * The console used to say everything on the page. Each panel had a "Q1 — is it broken"
 * label and a paragraph under it, and a table cell could run to a sentence. Read once,
 * those sentences are useful. Read every visit, they are what an operator scrolls past
 * to find the number. So the number stays on the page, and the sentence moves here: one
 * hover away, never in the way.
 *
 * Put the explanation of *what this is* or *how it is measured* here. Do not put
 * a finding here ("3 deliveries are dead"): a finding is the page.
 */
export const AdminInfo = ({
  children,
  className,
  side = "top",
  "data-testid": dataTestId
}: {
  children: ReactNode;
  className?: string;
  side?: "top" | "right" | "bottom" | "left";
  "data-testid"?: string;
}) => (
  <Tooltip>
    <TooltipTrigger asChild>
      <button
        type='button'
        aria-label='More information'
        data-testid={dataTestId}
        className={cn(
          "inline-flex shrink-0 items-center text-muted-foreground/70 transition-colors hover:text-foreground focus-visible:text-foreground focus-visible:outline-none",
          className
        )}
      >
        <Info className='size-3' />
      </button>
    </TooltipTrigger>
    <TooltipContent side={side} className='max-w-xs text-xs leading-relaxed'>
      {children}
    </TooltipContent>
  </Tooltip>
);
