import { ChevronRight } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
import { CheckList } from "@/pages/ide/Experiments/components/glance/CheckList";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import type { ResultResponse } from "@/types/experiment";
import { detailChecks, toReview } from "./detailChecks";

/**
 * The report's secondary detail as one collapsed checklist: a status per check,
 * coloured, with the server's sentences beneath. The header counts what needs a
 * look, so a closed section still says whether to open it. Content stays mounted
 * while closed.
 */
export default function ReportDetails({ result }: { result: ResultResponse }) {
  const checks = detailChecks(result);
  const review = toReview(checks);
  return (
    <Collapsible
      className='group/details rounded-lg border'
      data-testid='experiment-report-details'
    >
      <CollapsibleTrigger className='flex w-full items-center gap-2 p-4 text-left text-sm hover:bg-muted/40'>
        <ChevronRight className='size-4 transition-transform group-data-[state=open]/details:rotate-90' />
        <span className='font-medium'>Details</span>
        <span className='text-muted-foreground'>{checks.length} checks</span>
        <span className='ml-auto' data-testid='experiment-report-details-summary'>
          {review > 0 ? (
            <ToneBadge tone='warning'>{review} to review</ToneBadge>
          ) : (
            <ToneBadge tone='success'>All clear</ToneBadge>
          )}
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent forceMount className='border-t data-[state=closed]:hidden'>
        <CheckList checks={checks} />
      </CollapsibleContent>
    </Collapsible>
  );
}
