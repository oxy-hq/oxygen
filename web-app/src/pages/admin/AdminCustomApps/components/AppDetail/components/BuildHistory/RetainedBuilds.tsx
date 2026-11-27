import { ChevronRight } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
import { useAppPublishAttempts } from "@/hooks/api/customApps/useAppPublishAttempts";
import type { AppBuild } from "@/types/apps";
import { FailedPublishes } from "./FailedPublishes";
import { BuildHistory } from "./index";

/**
 * Every retained build, the ledger a rollback chooses from, behind its own
 * disclosure: the release timeline above shows the window's moves, and this the
 * builds that never moved in it.
 *
 * The publishes that failed sit above it, outside the disclosure: a failed publish
 * left no build for the ledger to list, and is the one thing here that means
 * something is wrong.
 */
export const RetainedBuilds = ({
  appId,
  onOpenChecks
}: {
  appId: string;
  /** Offered per build where the surface can show a build's checks. */
  onOpenChecks?: (build: AppBuild) => void;
}) => {
  const attempts = useAppPublishAttempts(appId);
  return (
    <>
      <FailedPublishes attempts={attempts.data?.attempts} />
      <Collapsible data-testid='admin-app-retained-builds'>
        <CollapsibleTrigger className='group flex items-center gap-1.5 text-left text-muted-foreground text-xs hover:text-foreground'>
          <ChevronRight className='size-3 shrink-0 transition-transform group-data-[state=open]:rotate-90' />
          All retained builds
        </CollapsibleTrigger>
        <CollapsibleContent className='pt-2'>
          <BuildHistory appId={appId} onOpenChecks={onOpenChecks} />
        </CollapsibleContent>
      </Collapsible>
    </>
  );
};
