import { useMemo } from "react";
import useActiveRuns from "@/hooks/api/coordinator/useActiveRuns";
import useRunHistory from "@/hooks/api/coordinator/useRunHistory";
import { rangeMs, type TimeRange } from "../components/constants";
import { mergeRuns, type NormalizedRun } from "../components/runModel";
import type { RunFilters } from "./components/RunsFilterBar";

/**
 * The run log — run history overlaid with the live active-runs feed so
 * in-flight rows stay fresh. Status / type / source / range / search are all
 * applied client-side over a growing page window.
 */
export const useRunsModel = (filters: RunFilters, limit: number) => {
  const active = useActiveRuns({ include_system: filters.includeSystem });
  const history = useRunHistory({
    limit,
    offset: 0,
    include_system: filters.includeSystem
  });

  const runs = useMemo<NormalizedRun[]>(() => {
    let merged = mergeRuns(active.data?.runs ?? [], history.data?.runs ?? []);
    if (filters.status !== "all") merged = merged.filter((r) => r.status === filters.status);
    if (filters.type !== "all") merged = merged.filter((r) => r.jobType === filters.type);
    if (filters.source !== "all") merged = merged.filter((r) => r.source === filters.source);
    if (filters.range !== "all") {
      const cutoff = Date.now() - rangeMs(filters.range as TimeRange);
      merged = merged.filter((r) => new Date(r.startedAt).getTime() >= cutoff);
    }
    const q = filters.search.trim().toLowerCase();
    if (q) {
      merged = merged.filter(
        (r) => r.title.toLowerCase().includes(q) || r.runId.toLowerCase().includes(q)
      );
    }
    return merged;
  }, [active.data, history.data, filters]);

  const fetched = history.data?.runs.length ?? 0;
  return {
    runs,
    total: history.data?.total ?? 0,
    hasMore: (history.data?.total ?? 0) > fetched,
    isPending: history.isPending,
    error: history.error,
    // TanStack refetches resolve with the error rather than rejecting; a failed
    // reload shows through each query's own `error`.
    refetch: () => {
      void active.refetch();
      void history.refetch();
    }
  };
};
