import { useMemo, useRef } from "react";
import useActiveRuns from "@/hooks/api/coordinator/useActiveRuns";
import useRunHistory from "@/hooks/api/coordinator/useRunHistory";
import { useSchedules } from "@/hooks/api/schedules/useSchedules";
import {
  type JobType,
  rangeMs,
  type TimeRange,
  targetKindToJobType
} from "../components/constants";
import type { JobTypeChoice } from "../components/Filters";
import { mergeRuns, type NormalizedRun } from "../components/runModel";
import { cronNextRuns } from "../components/utils";

export interface OverviewMetrics {
  activeJobs: number;
  runsInWindow: number;
  /** done / (done + failed), as a 0–100 integer; null when no terminal runs. */
  successRate: number | null;
  failed: number;
  runningNow: number;
}

/**
 * One slot a schedule was expected to fire in the visible window that we
 * cannot match to any actual run — the "missing run" indicator. Surfaced on
 * the timeline as hollow dashed markers in the appropriate type lane.
 */
export interface MissingSlot {
  scheduleId: string;
  scheduleName: string;
  jobType: JobType;
  atMs: number;
}

/** Match tolerance for "actual fired close enough to the expected slot". */
const MATCH_TOLERANCE_MS = 90 * 1000;
/** Cap expected-slot enumeration per schedule so a 1-minute cron doesn't
 *  generate 10k markers on a 7d window. */
const MAX_EXPECTED_PER_SCHEDULE = 500;

/** Derives every Overview surface from the coordinator + schedules queries. */
export const useOverviewModel = (range: TimeRange, typeFilter: JobTypeChoice) => {
  const active = useActiveRuns();
  // A wide page-1 window — the Overview filters down by time client-side.
  const history = useRunHistory({ limit: 200, offset: 0 });
  const schedules = useSchedules();

  // Bucket to minute resolution so these values only change once per minute,
  // preventing missingSlots from recomputing on every render.
  const nowMs = Math.floor(Date.now() / 60_000) * 60_000;
  const windowStartMs = nowMs - rangeMs(range);

  const runs = useMemo<NormalizedRun[]>(() => {
    const merged = mergeRuns(active.data?.runs ?? [], history.data?.runs ?? []);
    return merged.filter((r) => {
      if (new Date(r.startedAt).getTime() < windowStartMs) return false;
      if (typeFilter !== "all" && r.jobType !== typeFilter) return false;
      return true;
    });
  }, [active.data, history.data, windowStartMs, typeFilter]);

  // Cache per-schedule expected occurrences keyed on (id, cron_expr, timezone,
  // windowStartMs) so a sparse cron (e.g. monthly) doesn't re-walk 528k
  // iterations when only an unrelated schedule changes.
  const expectedCacheRef = useRef<Map<string, { key: string; occurrences: number[] }>>(new Map());

  const missingSlots = useMemo<MissingSlot[]>(() => {
    const enabled = (schedules.data ?? []).filter((s) => s.enabled);
    if (enabled.length === 0) return [];

    // Bucket actual runs by schedule_id with a sorted start-time array per
    // bucket so per-slot matching is O(log M) instead of O(M).
    const actualsBySched = new Map<string, number[]>();
    for (const r of mergeRuns(active.data?.runs ?? [], history.data?.runs ?? [])) {
      if (!r.scheduleId) continue;
      const bucket = actualsBySched.get(r.scheduleId) ?? [];
      bucket.push(new Date(r.startedAt).getTime());
      actualsBySched.set(r.scheduleId, bucket);
    }
    for (const b of actualsBySched.values()) b.sort((a, b) => a - b);

    const cache = expectedCacheRef.current;
    const fromDate = new Date(windowStartMs);
    const out: MissingSlot[] = [];

    for (const s of enabled) {
      const jobType = targetKindToJobType(s.target_kind);
      if (typeFilter !== "all" && jobType !== typeFilter) continue;

      const cacheKey = `${s.cron_expr}|${s.timezone}|${windowStartMs}`;
      let cached = cache.get(s.id);
      if (!cached || cached.key !== cacheKey) {
        const occurrences = cronNextRuns(
          s.cron_expr,
          s.timezone,
          MAX_EXPECTED_PER_SCHEDULE,
          fromDate
        ).map((d) => d.getTime());
        cached = { key: cacheKey, occurrences };
        cache.set(s.id, cached);
      }

      const expected = cached.occurrences.filter((t) => t <= nowMs);
      const bucket = actualsBySched.get(s.id) ?? [];
      for (const target of expected) {
        if (!hasMatchWithin(bucket, target, MATCH_TOLERANCE_MS)) {
          out.push({
            scheduleId: s.id,
            scheduleName: s.name,
            jobType,
            atMs: target
          });
        }
      }
    }
    return out;
  }, [active.data, history.data, schedules.data, typeFilter, windowStartMs, nowMs]);

  const metrics = useMemo<OverviewMetrics>(() => {
    const done = runs.filter((r) => r.status === "done").length;
    const failed = runs.filter((r) => r.status === "failed").length;
    const terminal = done + failed;
    const enabledJobs = (schedules.data ?? []).filter((s) => {
      if (typeFilter === "all") return s.enabled;
      return s.enabled && targetKindToJobType(s.target_kind) === typeFilter;
    }).length;
    return {
      activeJobs: enabledJobs,
      runsInWindow: runs.length,
      successRate: terminal > 0 ? Math.round((done / terminal) * 100) : null,
      failed,
      runningNow: runs.filter((r) => r.live).length
    };
  }, [runs, schedules.data, typeFilter]);

  return {
    runs,
    missingSlots,
    metrics,
    isPending: history.isPending,
    error: history.error,
    // TanStack refetches resolve with the error rather than rejecting; a failed
    // reload shows through each query's own `error`.
    refetch: () => {
      void active.refetch();
      void history.refetch();
      void schedules.refetch();
    }
  };
};

/** Binary-search a sorted-ascending number array for any value within `tol` of `target`. */
function hasMatchWithin(sorted: number[], target: number, tol: number): boolean {
  if (sorted.length === 0) return false;
  let lo = 0;
  let hi = sorted.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const v = sorted[mid];
    if (Math.abs(v - target) <= tol) return true;
    if (v < target) lo = mid + 1;
    else hi = mid - 1;
  }
  // Check neighbors that the loop may have just stepped past.
  const candidates = [sorted[lo], sorted[hi]].filter((v) => typeof v === "number");
  return candidates.some((v) => Math.abs(v - target) <= tol);
}
