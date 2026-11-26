import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { PublishAttempt, PublishStageTiming } from "@/types/appPublishAttempts";
import { formatMs, stageSegments, totalMs } from "./publishStages";

/**
 * How long each stage of one publish took: a thin bar split by each stage's share
 * of the whole, and every stage's time beneath it in the order the stages ran.
 *
 * Neutral on purpose — on this surface colour means something is wrong — so the
 * only coloured segment is the stage a refused publish failed at. A stage that took
 * no measurable time draws no segment and keeps its label.
 */
export const StagesBar = ({
  stages,
  failedStage,
  testId
}: {
  stages: PublishStageTiming[];
  failedStage?: string | null;
  testId: string;
}) => {
  if (stages.length === 0) return null;
  const segments = stageSegments(stages, failedStage);
  const total = formatMs(totalMs(stages));
  const summary = segments.map((s) => `${s.stage} ${formatMs(s.ms)}`).join(", ");

  return (
    <div className='flex min-w-0 flex-col gap-1' data-testid={testId}>
      <div
        className='flex h-1.5 w-full gap-px overflow-hidden'
        role='img'
        aria-label={`Stages: ${summary}; total ${total}`}
      >
        {segments
          .filter((s) => s.share > 0)
          .map((s) => (
            <div
              key={s.stage}
              className={cn(
                "h-full min-w-[2px]",
                s.failed ? ADMIN_TONE.danger.dot : "bg-muted-foreground/35"
              )}
              style={{ flexGrow: s.share, flexBasis: 0 }}
              title={`${s.stage} — ${formatMs(s.ms)}`}
              data-testid={`${testId}-segment-${s.stage}`}
            />
          ))}
      </div>
      <ul className='flex flex-wrap gap-x-3 gap-y-0.5 text-muted-foreground text-xs'>
        {segments.map((s) => (
          <li
            key={s.stage}
            className={cn(s.failed && cn("font-medium", ADMIN_TONE.danger.text))}
            data-testid={`${testId}-label-${s.stage}`}
          >
            {s.stage} <span className='tabular-nums'>{formatMs(s.ms)}</span>
          </li>
        ))}
        <li className='text-foreground' data-testid={`${testId}-total`}>
          total <span className='tabular-nums'>{total}</span>
        </li>
      </ul>
    </div>
  );
};

/** A ledger row's bar: the stages of the publish that landed the build, or nothing
 *  for a build recorded before stages were timed (or whose attempt retention let go). */
export const BuildStages = ({
  attempt,
  testId
}: {
  attempt: PublishAttempt | undefined;
  testId: string;
}) => (attempt ? <StagesBar stages={attempt.stages} testId={testId} /> : null);
