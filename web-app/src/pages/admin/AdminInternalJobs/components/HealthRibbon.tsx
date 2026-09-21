import { cn } from "@/libs/utils/cn";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { QueueStatusCounts } from "@/services/api/internalJobs";
import type { HistorySample } from "../useInternalJobsHistory";
import { throughputSeries } from "../useInternalJobsHistory";
import { Sparkline } from "./Sparkline";

/**
 * Compact, single-strip health readout that replaces the old full-height KPI
 * grid + throughput chart. Realtime is context here, not the headline — the
 * jobs console below is where the work happens. Six dense status cells +
 * one trailing throughput sparkline give the at-a-glance "is it draining?"
 * answer in a fraction of the vertical space.
 */
export const HealthRibbon = ({
  total,
  history
}: {
  total: QueueStatusCounts;
  history: HistorySample[];
}) => {
  const throughput = throughputSeries(history);
  const completedSeries = throughput.map((p) => p.completed);
  const failedSeries = throughput.map((p) => p.failed);
  const completedWin = completedSeries.reduce((a, b) => a + b, 0);
  const failedWin = failedSeries.reduce((a, b) => a + b, 0);

  return (
    <div className='flex flex-wrap items-stretch gap-px overflow-hidden rounded-lg border border-border/60 bg-border/60'>
      {CELLS.map((c) => (
        <Cell
          key={c.key}
          label={c.label}
          value={total[c.key]}
          tone={c.tone}
          emphasize={c.emphasize}
        />
      ))}
      <div className='flex flex-1 items-center justify-between gap-4 bg-card px-4 py-2'>
        <div className='flex flex-col'>
          <span className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
            Throughput · 5m
          </span>
          <div className='flex items-center gap-2 text-[11px] tabular-nums'>
            <span className={ADMIN_TONE.ok.text}>+{completedWin} done</span>
            <span className='text-muted-foreground/40'>/</span>
            <span className={ADMIN_TONE.warn.text}>+{failedWin} failed</span>
          </div>
        </div>
        <div className='flex items-center gap-1'>
          <Sparkline data={completedSeries} toneClass={ADMIN_TONE.ok.text} width={72} height={26} />
          <Sparkline data={failedSeries} toneClass={ADMIN_TONE.warn.text} width={72} height={26} />
        </div>
      </div>
    </div>
  );
};

const Cell = ({
  label,
  value,
  tone,
  emphasize
}: {
  label: string;
  value: number;
  tone: string;
  emphasize?: boolean;
}) => (
  <div className='flex min-w-20 flex-col justify-center bg-card px-4 py-2'>
    <span className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.12em]'>
      {label}
    </span>
    <span
      className={cn(
        "font-semibold text-sm tabular-nums tracking-tight",
        emphasize && value > 0 ? tone : "text-foreground"
      )}
    >
      {value.toLocaleString()}
    </span>
  </div>
);

const CELLS: Array<{
  key: keyof QueueStatusCounts;
  label: string;
  tone: string;
  emphasize?: boolean;
}> = [
  { key: "queued", label: "Queued", tone: "text-foreground" },
  { key: "claimed", label: "Claimed", tone: ADMIN_TONE.info.text, emphasize: true },
  { key: "completed", label: "Completed", tone: ADMIN_TONE.ok.text },
  { key: "failed", label: "Failed", tone: ADMIN_TONE.warn.text, emphasize: true },
  { key: "cancelled", label: "Cancelled", tone: ADMIN_TONE.muted.text },
  { key: "dead", label: "Dead", tone: ADMIN_TONE.danger.text, emphasize: true }
];
