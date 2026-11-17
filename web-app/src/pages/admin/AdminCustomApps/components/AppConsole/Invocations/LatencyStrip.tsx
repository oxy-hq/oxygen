import { cn } from "@/libs/shadcn/utils";
import type { AppInvocation } from "@/types/apps";
import { formatLatency, latencyDistribution } from "./invocationStats";

/**
 * How long the listed invocations took, as a histogram on a log axis with the p95
 * marked. Drawn from the rows the page holds, so when the window holds more than
 * the cap the caption says which rows those are: "the newest 200 of 1,412".
 *
 * Neutral throughout. A slow call is not, on its own, something wrong — a failure
 * is, and the failures are coloured in the count above and on their rows below.
 */
export const LatencyStrip = ({
  rows,
  total,
  capped
}: {
  rows: AppInvocation[];
  /** Every row in the window, when known. */
  total: number | null;
  /** The page holds fewer rows than the window. */
  capped: boolean;
}) => {
  const d = latencyDistribution(rows);
  if (d.measured === 0) return null;
  const tallest = Math.max(...d.buckets.map((b) => b.count));
  const of = capped
    ? `of the newest ${rows.length}${total != null ? ` of ${total.toLocaleString()}` : ""}`
    : `of all ${d.measured.toLocaleString()} in the window`;
  return (
    <figure className='flex flex-col gap-1' data-testid='admin-app-invocations-latency'>
      <figcaption className='flex flex-wrap items-baseline justify-between gap-x-3 text-muted-foreground text-xs'>
        <span>
          Latency{" "}
          <span data-testid='admin-app-invocations-latency-of'>
            {of}
            {d.unmeasured > 0 ? ` (${d.unmeasured} still running or unmeasured)` : ""}
          </span>
        </span>
        <span className='text-foreground tabular-nums'>
          p50 {d.p50 != null ? formatLatency(d.p50) : "—"} · p95{" "}
          <span className='font-medium' data-testid='admin-app-invocations-p95'>
            {d.p95 != null ? formatLatency(d.p95) : "—"}
          </span>
        </span>
      </figcaption>
      <div
        className='relative flex h-10 items-end gap-px'
        role='img'
        aria-label={`Latency histogram; p95 ${d.p95 != null ? formatLatency(d.p95) : "unknown"}`}
      >
        {d.buckets.map((b) => (
          <div
            key={b.from}
            className='flex-1 rounded-t-[1px] bg-foreground/25'
            style={{ height: b.count === 0 ? 0 : `${Math.max(6, (b.count / tallest) * 100)}%` }}
            title={`${formatLatency(b.from)}–${formatLatency(b.to)}: ${b.count}`}
          />
        ))}
        {d.p95At != null && (
          <div
            className='pointer-events-none absolute inset-y-0 w-px bg-foreground'
            style={{ left: `${d.p95At * 100}%` }}
            data-testid='admin-app-invocations-p95-marker'
          >
            {/* Beside the line, on whichever side has room. */}
            <span
              className={cn(
                "absolute -top-0.5 whitespace-nowrap font-medium text-[10px] leading-none",
                d.p95At > 0.85 ? "right-1" : "left-1"
              )}
            >
              p95
            </span>
          </div>
        )}
      </div>
      <div className='flex justify-between text-[10px] text-muted-foreground tabular-nums'>
        <span>{formatLatency(d.lo)}</span>
        <span>{formatLatency(d.hi)}</span>
      </div>
    </figure>
  );
};
