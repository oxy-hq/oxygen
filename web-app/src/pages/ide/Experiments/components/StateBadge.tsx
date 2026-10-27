import { Badge } from "@/components/ui/shadcn/badge";
import { badgeVariantFor, statusLabel } from "../viewForState";

export default function StateBadge({
  state,
  retrospective = false
}: {
  state: string;
  retrospective?: boolean;
}) {
  return (
    <span className='inline-flex items-center gap-1.5'>
      <Badge
        variant={badgeVariantFor(state)}
        data-testid='experiment-state-badge'
        data-state={state}
      >
        {statusLabel(state)}
      </Badge>
      {retrospective && (
        <Badge variant='outline' data-testid='experiment-retrospective-badge'>
          Retrospective
        </Badge>
      )}
    </span>
  );
}
