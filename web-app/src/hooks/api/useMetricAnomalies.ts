import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { toast } from "sonner";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { MetricAnomaliesService } from "@/services/api/metricAnomalies";
import type {
  AnomalyStatus,
  ListMonitorsResponse,
  MonitorCoverage,
  MonitorEntry,
  StatusWriteGroup
} from "@/types/metricAnomalies";
import type { ExplainResult } from "@/types/metricTree";
import queryKeys from "./queryKey";

/** One page of anomalies for the current workspace, optionally filtered by
 *  status. Returns the whole response — `total` is the page-count denominator
 *  and counts the same unit the page does (events by default, rows for
 *  `order="recent"`).
 *
 *  Pass `order="recent"` for latest-first (`detected_at DESC`) — e.g. the
 *  Monitors tab's "last anomaly" column, which must not be biased by the
 *  Inbox's severity ranking. Omit it for the worst-first Inbox ordering.
 *
 *  Omit `page` to take the server default (100 events), for callers that only
 *  need a count or the top of the list.
 *
 *  `placeholderData` keeps the previous *page* on screen while the next one
 *  loads, so stepping through pages doesn't flash the table's loading state.
 *  It deliberately does not span a status change — see below. */
export function useMetricAnomalies(
  status?: AnomalyStatus,
  order?: "recent",
  page?: { limit: number; offset: number }
) {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  return useQuery({
    queryKey: queryKeys.metricAnomalies.list(projectId, status, order, page),
    queryFn: () => MetricAnomaliesService.list(projectId, status, order, page),
    // Previous-page data, NOT previous-*list* data. Blanket `keepPreviousData`
    // also spans the status dimension of the key, so switching to "Dismissed"
    // would render the previous filter's rows — live and actionable — under
    // the new filter's heading. Held only when everything but the page matches.
    placeholderData: (previous, previousQuery) =>
      isSameList(previousQuery?.queryKey, projectId, status, order) ? previous : undefined,
    retry: false
  });
}

/** Does a cached key describe the same list as this one, differing at most in
 *  which page it holds? Mirrors `queryKeys.metricAnomalies.list`, whose tail is
 *  `[projectId, status, order, page]`. */
function isSameList(
  key: readonly unknown[] | undefined,
  projectId: string,
  status: AnomalyStatus | undefined,
  order: "recent" | undefined
): boolean {
  if (!key) return false;
  const [, , keyProject, keyStatus, keyOrder] = key;
  return keyProject === projectId && keyStatus === (status ?? null) && keyOrder === (order ?? null);
}

/** Trigger a scan; surfaces a success toast with monitor counts on completion.
 *  Pass `asOf` (YYYY-MM-DD) to scan against a past date — useful when the
 *  demo dataset doesn't reach today.
 *
 *  When `pending: true` is returned the scan is still running in the
 *  background. We poll the anomalies list at increasing intervals so the
 *  inbox updates automatically once results land. */
export function useScanMetricAnomalies() {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const qc = useQueryClient();
  // Cleared on unmount. A backgrounded scan schedules polls up to 75s out, and
  // leaving the IDE or switching projects in that window otherwise leaves them
  // firing against the project that was current when the scan started.
  const polls = useRef<ReturnType<typeof setTimeout>[]>([]);
  useEffect(() => () => polls.current.forEach(clearTimeout), []);
  return useMutation({
    mutationFn: (asOf?: string) => MetricAnomaliesService.scan(projectId, asOf),
    onSuccess: (data) => {
      if (data.pending) {
        toast.info("Scan started — anomalies will appear shortly.");
        // Poll at 5 s, 15 s, 35 s, and 75 s to pick up background results —
        // coverage as well as anomalies. A scan writes both, and the Monitors
        // tab holds coverage for 60 s with no refetch interval, so a scan that
        // outran the server's synchronous window would leave it showing
        // pre-scan coverage until an unrelated remount.
        // Replaced, not appended: each scan schedules its own four, and a
        // long session that scans often would otherwise keep every dead handle
        // it ever made until unmount.
        polls.current.forEach(clearTimeout);
        polls.current = [];
        for (const delayMs of [5_000, 15_000, 35_000, 75_000]) {
          const timer = setTimeout(() => {
            qc.invalidateQueries({ queryKey: queryKeys.metricAnomalies.lists(projectId) });
            qc.invalidateQueries({ queryKey: queryKeys.metricAnomalies.monitors(projectId) });
          }, delayMs);
          polls.current.push(timer);
        }
        return;
      }
      const monitors = data.monitors_scanned;
      const failed = data.monitors_failed;
      const persisted = data.anomalies_persisted;
      if (failed > 0) {
        const failures = data.failures ?? [];
        // Name the first few failed monitors + their error in the toast; the
        // inbox banner carries the full list. Sonner collapses newlines, so
        // join with a middot rather than "\n".
        const preview = failures
          .slice(0, 3)
          .map((f) => {
            const name = f.label || f.measure;
            const seg = f.dimension_key ? ` [${f.dimension_key}]` : "";
            return `${name}${seg}: ${clampError(f.error)}`;
          })
          .join(" · ");
        const more = failures.length > 3 ? ` · +${failures.length - 3} more` : "";
        toast.warning(
          `Scanned ${monitors} monitor${monitors === 1 ? "" : "s"} (${failed} failed). ${persisted} anomalies persisted.`,
          { description: preview ? `${preview}${more}` : undefined, duration: 10_000 }
        );
      } else {
        toast.success(
          `Scanned ${monitors} monitor${monitors === 1 ? "" : "s"}. ${persisted} anomalies persisted.`
        );
      }
      // Lists and coverage: a scan writes anomalies and monitor coverage, and
      // leaves cached explains alone.
      qc.invalidateQueries({ queryKey: queryKeys.metricAnomalies.lists(projectId) });
      qc.invalidateQueries({ queryKey: queryKeys.metricAnomalies.monitors(projectId) });
    },
    onError: (e) => {
      toast.error(`Scan failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
}

/** Clamp a single error message for the toast preview so one verbose
 *  `ScanError` chain doesn't swamp the middot-joined list. The inbox banner
 *  carries the full, untruncated error. */
function clampError(error: string, max = 120): string {
  return error.length > max ? `${error.slice(0, max - 1)}…` : error;
}

/** Mark anomalies acknowledged / dismissed / new — one row action or a whole
 *  batch, both through the same bulk endpoint.
 *
 *  Takes the single group `targetOf` produces (or `null` when nothing is
 *  selected): events named by id, so the server writes buckets a capped list
 *  response never shipped, bounded to the statuses the current view can act on
 *  so a write can't reach buckets the user couldn't see. That bound comes from
 *  the view, not from each row, so even a selection spanning statuses is one
 *  request — one pending flag, one error, one toast.
 *
 *  `events` is how many anomalies the groups represent, for the toast; it does
 *  not change what is written. `variables.status` is what a caller reads to
 *  put the spinner on the button that was actually clicked. */
export function useUpdateAnomalyStatus() {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      group,
      status
    }: {
      group: StatusWriteGroup | null;
      status: AnomalyStatus;
      events?: number;
      /** Which row asked, when a table shares one mutation across its rows.
       *  Read back off `variables` to place the spinner; never sent. */
      rowKey?: string;
    }) =>
      // Nothing selected is a no-op, not a request. `targetOf` returns one
      // group per call — the scope depends on the view, not on each row — so
      // this is a single write, and a failure is a failure of the whole thing.
      group
        ? MetricAnomaliesService.updateStatus(projectId, group, status)
        : Promise.resolve({ updated: 0, events_updated: 0 }),
    onSuccess: (data, { status, events = 1 }) => {
      // Report what the server wrote, not what we asked it to. A selection can
      // go stale between the list and the click, and a success toast over a
      // write that didn't fully land is the exact false confidence this whole
      // change set is meant to remove.
      if (data.updated === 0) {
        // The rows moved on between the list and the click. The other way to
        // land here — clicking again over rows already in the target status —
        // is closed by the caller keeping its actions disabled through the
        // refetch this write triggers.
        toast.warning("Nothing to update — those anomalies are no longer in this view.");
        return;
      }
      toast.success(statusToast(status, events, data));
    },
    onError: (e) => {
      toast.error(`Failed to update status: ${e instanceof Error ? e.message : String(e)}`);
    },
    // Settled, not success: a failed request can still have committed, so the
    // list has to be refetched either way or the table shows a stale status.
    //
    // Lists only. `all` also covers the per-anomaly explains, and a status
    // change cannot invalidate a decomposition — but it would discard one,
    // sending the open drawer back for another 20-30s recomputation every time
    // a row behind it is acked.
    //
    // Returned, not fired and forgotten: React Query awaits a promise from
    // `onSettled` before the mutation leaves `isPending`, which is what keeps
    // the actions disabled until the rows they wrote have actually come back.
    // Without it there is a window where the write has settled but the table
    // still shows pre-write rows, and a second click re-sends a write the
    // server no-ops — reported as "no longer in this view" over an action that
    // just landed. Gating on the query's own `isFetching` would close the same
    // window, but it would also freeze the table on every window-focus refetch
    // and every scan poll, which have nothing to do with this write.
    onSettled: () => qc.invalidateQueries({ queryKey: queryKeys.metricAnomalies.lists(projectId) })
  });
}

const STATUS_VERB: Record<AnomalyStatus, string> = {
  new: "reopened",
  acknowledged: "acknowledged",
  dismissed: "dismissed"
};

/** What the write did, as a clause with no trailing punctuation — e.g.
 *  `"3 anomalies acknowledged (7 buckets)"` or `"3 of 5 anomalies
 *  acknowledged"`.
 *
 *  Both counts come from the server: targeting an event by id means the client
 *  never knew how many buckets it was about to write, and a selection can go
 *  stale between the list and the click. Claiming the selection size would
 *  contradict the table the moment it refetches. */
function landedClause(
  status: AnomalyStatus,
  selected: number,
  written: { updated: number; events_updated: number }
): string {
  const verb = STATUS_VERB[status];
  const landed = written.events_updated;
  if (landed < selected) return `${landed} of ${selected} anomalies ${verb}`;
  const subject = landed === 1 ? "Anomaly" : `${landed} anomalies`;
  const spread = written.updated > landed ? ` (${written.updated} buckets)` : "";
  return `${subject} ${verb}${spread}`;
}

/** The clean-apply sentence. A shortfall here means the missing anomalies moved
 *  on — which is only true when nothing errored, so the explanation lives here
 *  rather than in `landedClause`. */
function statusToast(
  status: AnomalyStatus,
  selected: number,
  written: { updated: number; events_updated: number }
): string {
  const clause = landedClause(status, selected, written);
  const shortfall = written.events_updated < selected;
  return shortfall ? `${clause} — the rest are no longer in this view.` : `${clause}.`;
}

/** Cached server-side explain for a single anomaly. The first call runs
 *  airlayer's recursive decomposition and writes the result onto the
 *  row; every later call (including after a page refresh) returns the
 *  cached payload instantly. Pair with `useRefreshAnomalyExplain` for
 *  the bust-cache button. */
export function useAnomalyExplain(anomalyId: string | null) {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  return useQuery<ExplainResult>({
    queryKey: anomalyId
      ? queryKeys.metricAnomalies.explain(projectId, anomalyId)
      : ([...queryKeys.metricAnomalies.all, "explain", "idle"] as const),
    queryFn: () => {
      if (!anomalyId) throw new Error("anomalyId required");
      return MetricAnomaliesService.explain(projectId, anomalyId);
    },
    enabled: !!anomalyId,
    // No staleTime — the server is the source of truth. React Query keeps
    // the cached payload in memory for the session, then re-fetches on
    // mount (server returns cached row, so still effectively instant).
    retry: false
  });
}

/** Shared fetch behind {@link useMonitors} and {@link useMonitorCoverage}.
 *
 *  One endpoint, one cache entry, two selectors — React Query dedupes on the
 *  shared key, so the tab does not fetch this twice.
 *
 *  The config half only changes on deploy/edit, but the coverage half moves
 *  with every scan, so this is no longer `staleTime: Infinity`. */
function useMonitorsQuery<T>(select: (data: ListMonitorsResponse) => T) {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  return useQuery({
    queryKey: queryKeys.metricAnomalies.monitors(projectId),
    queryFn: () => MetricAnomaliesService.listMonitors(projectId),
    staleTime: 60_000,
    select
  });
}

/** Monitor configurations from `.monitor.yml`. Returns an empty array when no
 *  file is configured. */
export function useMonitors() {
  return useMonitorsQuery((data) => data.monitors ?? ([] as MonitorEntry[]));
}

/** Per-segment scan coverage — which monitors are being scored and which are
 *  still accumulating history. Empty until the workspace has been scanned. */
export function useMonitorCoverage() {
  return useMonitorsQuery((data) => data.coverage ?? ([] as MonitorCoverage[]));
}

/** The file's `notify:` block, or `null` when new insights go nowhere but the
 *  inbox. */
export function useMonitorNotify() {
  return useMonitorsQuery((data) => data.notify ?? null);
}

/** Force-refresh the cached explain for an anomaly. Calls the same
 *  endpoint with `?refresh=true`, then writes the fresh payload into
 *  the React Query cache so subscribers re-render. */
export function useRefreshAnomalyExplain() {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (anomalyId: string) => MetricAnomaliesService.explain(projectId, anomalyId, true),
    onSuccess: (data, anomalyId) => {
      qc.setQueryData(queryKeys.metricAnomalies.explain(projectId, anomalyId), data);
    },
    onError: (e) => {
      toast.error(`Refresh failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
}
