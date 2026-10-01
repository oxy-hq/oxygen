import { useEffect, useRef, useState } from "react";
import type { QueueStatsResponse, QueueStatusCounts } from "@/services/api/internalJobs";

/**
 * Rolling client-side sample buffer fed by the 5s `useQueueStats` poll.
 * The backend exposes a single snapshot per request; the operator console
 * accumulates them in memory so KPI tiles can render sparklines and the
 * throughput chart can plot per-status deltas over time.
 *
 * Why client-side instead of a backend timeseries endpoint:
 *   - The "is the queue draining RIGHT NOW?" question is the only one
 *     this surface needs to answer; a fixed-size in-memory window is
 *     plenty.
 *   - No backend migration / new endpoint required.
 *   - For longer history operators use Prometheus / Grafana.
 *
 * Capacity = SAMPLES_KEEP (default 60). At 5s polling that is the
 * trailing 5 minutes — enough to spot a draining trend without being
 * noisy.
 */
const SAMPLES_KEEP = 60;

export interface HistorySample {
  /** Epoch millis when the snapshot was received. */
  t: number;
  total: QueueStatusCounts;
  last_1h: QueueStatusCounts;
  last_24h: QueueStatusCounts;
}

export interface ThroughputPoint {
  t: number;
  /** completed in this interval (delta vs previous sample). */
  completed: number;
  /** failed + dead in this interval. */
  failed: number;
}

/**
 * Accumulate snapshots into a fixed-size ring buffer. The hook is
 * driven by the data identity (`updatedAt` from React Query) so a
 * background re-render that doesn't produce new data is a no-op.
 */
export function useInternalJobsHistory(
  data: QueueStatsResponse | undefined,
  updatedAt: number | undefined
) {
  const [samples, setSamples] = useState<HistorySample[]>([]);
  const lastSeenRef = useRef<number | undefined>(undefined);

  useEffect(() => {
    if (!data || !updatedAt || lastSeenRef.current === updatedAt) return;
    lastSeenRef.current = updatedAt;
    setSamples((prev) => {
      const next: HistorySample = {
        t: updatedAt,
        total: data.total,
        last_1h: data.last_1h,
        last_24h: data.last_24h
      };
      const merged = [...prev, next];
      // Drop the head if we overflow the keep-window. Avoids the
      // common slice() footgun by allocating exactly once.
      return merged.length > SAMPLES_KEEP ? merged.slice(merged.length - SAMPLES_KEEP) : merged;
    });
  }, [data, updatedAt]);

  return samples;
}

/**
 * Compute completed/failed deltas between consecutive samples so the
 * throughput chart can show "jobs/sec" rather than "queue depth". The
 * first sample is emitted as `0, 0` because there is no prior point
 * to diff against.
 */
export function throughputSeries(samples: HistorySample[]): ThroughputPoint[] {
  if (samples.length === 0) return [];
  const result: ThroughputPoint[] = [];
  for (let i = 0; i < samples.length; i++) {
    if (i === 0) {
      result.push({ t: samples[i].t, completed: 0, failed: 0 });
      continue;
    }
    const prev = samples[i - 1].total;
    const cur = samples[i].total;
    // Clamp each delta independently. Combined clamping let a dead-row
    // drop (e.g. an operator bulk-deleting from this very page) cancel
    // out a genuine failed-row increase in the same tick — exactly the
    // scenario the bulk-delete affordance induces. Treat decreases as
    // counter resets / vacuums and floor at 0 per source.
    result.push({
      t: samples[i].t,
      completed: Math.max(0, cur.completed - prev.completed),
      failed: Math.max(0, cur.failed - prev.failed) + Math.max(0, cur.dead - prev.dead)
    });
  }
  return result;
}
