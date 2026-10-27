import { ChevronRight } from "lucide-react";
import type { ReactNode } from "react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";

/**
 * Method prose, folded away. Closed by default; the content stays mounted (only
 * hidden), so every sentence is still on the page for search and for tests.
 */
export default function HowCalculated({
  title = "How this was calculated",
  children
}: {
  title?: string;
  children: ReactNode;
}) {
  return (
    <Collapsible className='group/how'>
      <CollapsibleTrigger className='flex items-center gap-1 text-muted-foreground text-xs hover:text-foreground'>
        <ChevronRight className='size-3.5 transition-transform group-data-[state=open]/how:rotate-90' />
        {title}
      </CollapsibleTrigger>
      <CollapsibleContent
        forceMount
        className='mt-2 space-y-1.5 border-l-2 pl-3 text-muted-foreground text-xs data-[state=closed]:hidden'
      >
        {children}
      </CollapsibleContent>
    </Collapsible>
  );
}
