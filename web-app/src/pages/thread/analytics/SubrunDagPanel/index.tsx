import { CheckCircle2, Circle, Repeat, XCircle, Zap } from "lucide-react";
import { useMemo } from "react";
import { Panel, PanelContent, PanelHeader } from "@/components/ui/panel";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import type { SseEvent } from "@/hooks/useAnalyticsRun";
import { cn } from "@/libs/shadcn/utils";

// ── Types ─────────────────────────────────────────────────────────────────────

type AutomationStepStatus = "idle" | "running" | "done" | "failed";

type StepState = {
  status: AutomationStepStatus;
  taskType: string;
  subStepsStarted: number;
  subStepsCompleted: number;
  isPreagg: boolean;
};

// ── Status derivation ─────────────────────────────────────────────────────────

/**
 * Walk the event list and produce a state for every known step name.
 * Steps that have no events yet remain "idle".
 * For loop_sequential tasks, sub-step progress is also tracked.
 */
function deriveStepStatuses(
  steps: Array<{ name: string; task_type: string }>,
  events: SseEvent[]
): Record<string, StepState> {
  const topLevelNames = new Set(steps.map((s) => s.name));
  const statuses: Record<string, StepState> = {};
  for (const s of steps) {
    statuses[s.name] = {
      status: "idle",
      taskType: s.task_type,
      subStepsStarted: 0,
      subStepsCompleted: 0,
      isPreagg: false
    };
  }
  let activeLoopStep: string | null = null;
  for (const ev of events) {
    if (ev.type === "subrun_step_started") {
      const step = (ev.data as { step: string }).step;
      if (topLevelNames.has(step)) {
        statuses[step].status = "running";
        if (statuses[step].taskType === "loop_sequential") {
          activeLoopStep = step;
        }
      } else if (activeLoopStep && statuses[activeLoopStep]) {
        statuses[activeLoopStep].subStepsStarted++;
      }
    } else if (ev.type === "subrun_step_completed") {
      const { step, success, is_preagg } = ev.data as {
        step: string;
        success: boolean;
        is_preagg?: boolean;
      };
      if (topLevelNames.has(step)) {
        statuses[step].status = success ? "done" : "failed";
        if (is_preagg) statuses[step].isPreagg = true;
        if (step === activeLoopStep) {
          activeLoopStep = null;
        }
      } else if (activeLoopStep && statuses[activeLoopStep]) {
        if (success) statuses[activeLoopStep].subStepsCompleted++;
      }
    }
  }
  return statuses;
}

// ── Sub-components ────────────────────────────────────────────────────────────

const STATUS_ICON: Record<AutomationStepStatus, React.ReactNode> = {
  idle: <Circle className='h-3.5 w-3.5 text-muted-foreground' />,
  running: <Spinner className='size-3 text-primary' />,
  done: <CheckCircle2 className='h-3.5 w-3.5 text-success' />,
  failed: <XCircle className='h-3.5 w-3.5 text-destructive' />
};

const STATUS_LABEL: Record<AutomationStepStatus, string | null> = {
  idle: null,
  running: "Running…",
  done: "Done",
  failed: "Failed"
};

const ICON_BG: Record<AutomationStepStatus, string> = {
  idle: "bg-secondary",
  running: "bg-primary/20",
  done: "bg-success/20",
  failed: "bg-destructive/20"
};

const NODE_BORDER: Record<AutomationStepStatus, string> = {
  idle: "border-border bg-card opacity-60",
  running:
    "border-primary bg-primary/10 shadow-[0_0_12px_color-mix(in_srgb,var(--blue-500)_15%,transparent)]",
  done: "border-success/40 bg-success/5",
  failed: "border-destructive/40 bg-destructive/5"
};

// ── Props ─────────────────────────────────────────────────────────────────────

interface SubrunDagPanelProps {
  /** Automation name shown in the panel header (file stem). */
  automationName: string;
  /** Ordered list of top-level task descriptors from the `subrun_started` event. */
  steps: Array<{ name: string; task_type: string }>;
  /** All SSE events for the active run — step statuses are derived from these. */
  events: SseEvent[];
  /** Whether the run is still active (controls the header subtitle). */
  isRunning: boolean;
  onClose: () => void;
}

// ── Component ─────────────────────────────────────────────────────────────────

const SubrunDagPanel = ({
  automationName,
  steps,
  events,
  isRunning,
  onClose
}: SubrunDagPanelProps) => {
  const stepStatuses = useMemo(() => deriveStepStatuses(steps, events), [steps, events]);

  const completionEv = useMemo(
    () => [...events].reverse().find((ev) => ev.type === "subrun_completed") ?? null,
    [events]
  );

  const subtitle = isRunning
    ? "Running…"
    : completionEv
      ? (completionEv.data as { success: boolean }).success
        ? "Completed"
        : "Failed"
      : undefined;

  return (
    <Panel>
      <PanelHeader
        title={automationName || "Automation Run"}
        subtitle={subtitle}
        onClose={onClose}
      />

      <PanelContent scrollable={false} padding={false} className='overflow-y-auto p-4'>
        <div className='flex flex-col items-center gap-0'>
          {steps.map((step, i) => {
            const state = stepStatuses[step.name] ?? {
              status: "idle",
              taskType: step.task_type,
              subStepsStarted: 0,
              subStepsCompleted: 0,
              isPreagg: false
            };
            const status = state.status;
            const isHighlighted = status === "running";

            return (
              <div key={step.name} className='flex w-full flex-col items-center'>
                {i > 0 && (
                  <div
                    className={cn(
                      "h-6 w-px transition-colors duration-300",
                      isHighlighted ? "bg-primary" : "bg-border"
                    )}
                  />
                )}

                <div
                  className={cn(
                    "relative flex w-full cursor-default items-center gap-2.5 rounded-lg border px-3 py-2.5 transition-all duration-300",
                    NODE_BORDER[status]
                  )}
                >
                  {/* Icon */}
                  <div
                    className={cn(
                      "flex h-7 w-7 shrink-0 items-center justify-center rounded-md",
                      ICON_BG[status]
                    )}
                  >
                    {STATUS_ICON[status]}
                  </div>

                  {/* Label */}
                  <div className='min-w-0 flex-1'>
                    <div className='flex items-center gap-1'>
                      <div
                        className={cn(
                          "truncate font-mono text-[10px]",
                          status === "idle" ? "text-muted-foreground" : "text-foreground"
                        )}
                      >
                        {step.name}
                      </div>
                      {state.isPreagg && (
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Zap className='h-2.5 w-2.5 shrink-0 text-primary' />
                          </TooltipTrigger>
                          <TooltipContent side='top'>
                            Served from pre-aggregation cache
                          </TooltipContent>
                        </Tooltip>
                      )}
                    </div>
                    {STATUS_LABEL[status] && (
                      <div className='mt-0.5 text-[10px] text-muted-foreground'>
                        {STATUS_LABEL[status]}
                      </div>
                    )}
                    {step.task_type === "loop_sequential" && state.subStepsStarted > 0 && (
                      <div className='mt-0.5 flex items-center gap-1 text-[10px] text-muted-foreground'>
                        <Repeat className='h-2.5 w-2.5' />
                        <span>
                          {state.subStepsCompleted} / {state.subStepsStarted} sub-steps
                        </span>
                      </div>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </PanelContent>
    </Panel>
  );
};

export default SubrunDagPanel;
