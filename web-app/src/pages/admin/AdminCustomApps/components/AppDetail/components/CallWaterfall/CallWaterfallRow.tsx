import type { LucideIcon } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { InvocationCall } from "@/types/apps";
import { familyIcon } from "./familyIcon";
import {
  barGeometry,
  callBarClass,
  callLabel,
  callOutcome,
  callTitle,
  formatMs,
  ISOLATE_BAR_CLASS
} from "./model";

/** One row of the waterfall: what ran, its bar on the shared track, and how
 *  long it took. A subgrid of the waterfall's columns, so every row's bar
 *  starts at the same x. */
const WaterfallRow = ({
  icon: Icon,
  word,
  label,
  outcome,
  outcomeTone,
  title,
  startMs,
  durationMs,
  totalMs,
  barClass,
  testId,
  op
}: {
  icon: LucideIcon;
  word: string;
  label: string;
  outcome: string | null;
  outcomeTone: string;
  title: string;
  startMs: number;
  durationMs: number;
  totalMs: number;
  barClass: string;
  testId: string;
  op?: string;
}) => {
  const { leftPct, widthPct } = barGeometry(startMs, durationMs, totalMs);
  return (
    <div
      className='col-span-full grid grid-cols-subgrid items-center gap-y-0.5 py-1 text-xs'
      title={title}
      data-testid={testId}
      data-op={op}
    >
      <div className='col-span-full flex min-w-0 items-center gap-1.5'>
        <Icon className='size-3 shrink-0 text-muted-foreground' aria-hidden />
        <span className='shrink-0 text-muted-foreground'>{word}</span>
        <span className='min-w-0 truncate font-mono'>{label}</span>
        {outcome && (
          <span className={cn("min-w-0 shrink truncate font-medium", outcomeTone)}>{outcome}</span>
        )}
      </div>
      <div className='relative h-3 overflow-hidden bg-muted/30'>
        <div
          className={cn("absolute inset-y-0.5 min-w-1", barClass)}
          style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
        />
      </div>
      <span className='text-right font-mono text-muted-foreground tabular-nums'>
        {formatMs(durationMs)}
      </span>
    </div>
  );
};

export const CallWaterfallRow = ({ call, totalMs }: { call: InvocationCall; totalMs: number }) => {
  const outcome = callOutcome(call);
  return (
    <WaterfallRow
      icon={familyIcon(call.family)}
      word={call.family}
      label={callLabel(call)}
      outcome={outcome}
      outcomeTone={call.status === "error" ? ADMIN_TONE.danger.text : ADMIN_TONE.muted.text}
      title={callTitle(call)}
      startMs={call.start_ms}
      durationMs={call.duration_ms}
      totalMs={totalMs}
      barClass={callBarClass(call.status)}
      testId='admin-app-invocation-calls-row'
      op={call.op}
    />
  );
};

/** The platform's time before tenant code ran: thread, isolate, compile. */
export const IsolateStartRow = ({
  initMs,
  totalMs,
  icon
}: {
  initMs: number;
  totalMs: number;
  icon: LucideIcon;
}) => (
  <WaterfallRow
    icon={icon}
    word='isolate'
    label='start-up'
    outcome={null}
    outcomeTone=''
    title={`Isolate start-up before the function's code ran — ${formatMs(initMs)}`}
    startMs={0}
    durationMs={initMs}
    totalMs={totalMs}
    barClass={ISOLATE_BAR_CLASS}
    testId='admin-app-invocation-calls-isolate'
  />
);
