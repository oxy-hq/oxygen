import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildChecks } from "@/types/appBuildChecks";
import { checksProgress, type StepState } from "../buildChecksModel";
import { StatusIcon } from "./StatusIcon";

/** Each step state as the row status whose glyph it borrows. */
const GLYPH: Record<StepState, string> = {
  done: "passed",
  active: "running",
  todo: "queued",
  failed: "failed"
};

/** What a screen reader hears where the eye sees the glyph. */
const SPOKEN: Record<StepState, string> = {
  done: "done",
  active: "in progress",
  todo: "to do",
  failed: "failed"
};

/**
 * Where the run stands, before any row is read: Queued, Runner, Checking, Done. The
 * checks run on a runner that takes a while to come up, so the strip is what says
 * "nobody has answered yet" apart from "it is working". Nothing for a build never
 * checked.
 */
export const ChecksProgress = ({ data }: { data: BuildChecks }) => {
  const steps = checksProgress(data);
  if (!steps) return null;
  return (
    <ol
      className='grid grid-cols-4 rounded-md border border-border/60 bg-muted/30 px-3 py-2'
      aria-label='Progress of the run'
      data-testid='build-checks-progress'
    >
      {steps.map((step, i) => (
        <li
          key={step.key}
          className='flex min-w-0 flex-col gap-0.5'
          aria-current={step.state === "active" ? "step" : undefined}
          data-testid={`build-checks-step-${step.key}`}
          data-state={step.state}
        >
          <span className='flex items-center gap-1 pr-1'>
            <StatusIcon status={GLYPH[step.state]} />
            {i < steps.length - 1 && (
              <span
                className={cn(
                  "h-0.5 flex-1 rounded-full",
                  step.state === "done" ? ADMIN_TONE.ok.dot : "bg-border"
                )}
                aria-hidden
              />
            )}
          </span>
          <span
            className={cn(
              "truncate text-xs",
              step.state === "todo" ? "text-muted-foreground" : "font-medium"
            )}
          >
            {step.label}
          </span>
          <span className='sr-only'>{SPOKEN[step.state]}</span>
          <span className='truncate text-muted-foreground text-xs tabular-nums'>
            {step.value || " "}
          </span>
        </li>
      ))}
    </ol>
  );
};
