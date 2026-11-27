import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildCheckDetail } from "@/types/appBuildChecks";
import { newErrors, type RollbackReading, rollbackOf, screenName } from "../screensModel";
import { ScreensEvidence } from "./ScreensEvidence";

/** What the platform did about the failed check: it rolled production back, or says why not. */
const RollbackLine = ({ rollback }: { rollback: RollbackReading }) => (
  <span
    className={cn("text-xs", rollback.rolledBack ? ADMIN_TONE.warn.text : "text-muted-foreground")}
    title={rollback.at ? new Date(rollback.at).toLocaleString() : undefined}
    data-testid='build-check-rollback'
  >
    {rollback.rolledBack
      ? `Production was rolled back automatically${rollback.to ? ` to build ${rollback.to}` : ""}`
      : `Not rolled back automatically${rollback.reason ? `: ${rollback.reason}` : ""}`}
  </span>
);

/**
 * Production opened after a move: what the platform did about a failure first, then
 * the errors it shows now and did not before, each with its screen and viewport, then
 * what they were compared with, then every screen as the staging smoke lists them.
 */
export const ProductionSmokeEvidence = ({ detail }: { detail: BuildCheckDetail }) => {
  const errors = newErrors(detail);
  const baseline = typeof detail.baseline_at === "string" ? detail.baseline_at : null;
  const rollback = rollbackOf(detail);
  return (
    <div className='flex flex-col gap-2' data-testid='build-check-production-smoke'>
      {rollback && <RollbackLine rollback={rollback} />}
      {errors.length > 0 && (
        <div className='flex flex-col gap-1'>
          <span className='text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
            New errors
          </span>
          <ul className='flex flex-col gap-1' data-testid='build-check-new-errors'>
            {errors.map((e, i) => (
              <li
                // biome-ignore lint/suspicious/noArrayIndexKey: a screen can throw the same error twice
                key={i}
                className='flex min-w-0 items-baseline gap-2 text-xs'
                data-testid='build-check-new-error'
              >
                <span className='shrink-0 font-mono'>{screenName(e.path)}</span>
                {e.viewport && <span className='shrink-0 text-muted-foreground'>{e.viewport}</span>}
                <span className={cn("min-w-0 break-words font-mono", ADMIN_TONE.danger.text)}>
                  {e.error}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <span className='text-muted-foreground text-xs' data-testid='build-check-baseline'>
        {baseline ? (
          <span title={new Date(baseline).toLocaleString()}>
            Compared with production {timeAgo(baseline)}
          </span>
        ) : (
          "Nothing to compare with"
        )}
      </span>
      <ScreensEvidence detail={detail} />
    </div>
  );
};
