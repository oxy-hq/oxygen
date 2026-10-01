import { Badge } from "@/components/ui/shadcn/badge";
import { Spinner } from "@/components/ui/shadcn/spinner";
import type { PreviewRunOutcome, PreviewRunState } from "@/types/workspace";

/** `state` while a run hasn't finished; `outcome` once it has. Exactly one applies. */
export default function PreviewRunStateBadge({
  state,
  outcome
}: {
  state: PreviewRunState;
  outcome: PreviewRunOutcome | null;
}) {
  if (state !== "finished") {
    return (
      <Badge variant='secondary' data-testid='preview-run-state'>
        {state === "queued" ? "Queued" : <Spinner className='size-3' />}
        {state === "running" && "Running"}
      </Badge>
    );
  }
  if (outcome === "failed") {
    return (
      <Badge variant='destructive' data-testid='preview-run-state'>
        Failed
      </Badge>
    );
  }
  if (outcome === "cancelled") {
    return (
      <Badge variant='outline' data-testid='preview-run-state'>
        Cancelled
      </Badge>
    );
  }
  return (
    <Badge variant='default' data-testid='preview-run-state'>
      Succeeded
    </Badge>
  );
}
