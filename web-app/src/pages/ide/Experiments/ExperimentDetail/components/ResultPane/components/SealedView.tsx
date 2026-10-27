import { Lock } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { COLUMN } from "@/pages/ide/Experiments/ExperimentDetail/column";
import { statusLabel } from "@/pages/ide/Experiments/viewForState";

/**
 * `GET /result` answered 409 `{ sealed: true }`: the platform will not release
 * an effect yet. It is the expected answer until the run has reported, so it is
 * a calm state — never an error, never a toast.
 */
export default function SealedView({ state }: { state: string }) {
  return (
    <div
      className={cn(COLUMN, "flex flex-wrap items-center gap-2 rounded-lg border p-3 text-sm")}
      data-testid='experiment-sealed'
    >
      <ToneBadge tone='neutral'>
        <Lock aria-hidden className='size-3' />
        The result is sealed
      </ToneBadge>
      <span className='text-muted-foreground'>
        No report is released for this experiment (state: {statusLabel(state)}). The effect is
        sealed until the run reports; this page checks for it on its own.
      </span>
    </div>
  );
}
