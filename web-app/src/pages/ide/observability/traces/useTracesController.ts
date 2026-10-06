import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import useTraces from "@/hooks/api/traces/useTraces";
import type { Trace } from "@/services/api/traces";
import { MAX_COMPARE } from "./constants";
import { readTracesViewState, type TracesViewState, writeTracesViewState } from "./tracesViewState";
import { type StatusFilter, statusFilterToApi, type TimeRange, type TraceView } from "./types";

const PAGE_SIZE = 10;
const CHART_LIMIT = 500;
const LIVE_INTERVAL_MS = 5000;
const SEARCH_DEBOUNCE_MS = 300;

interface UseTracesControllerArgs {
  enabled: boolean;
}

/**
 * Owns all Traces-surface UI state (Theme 3). The filters that say *where* the
 * reader is — time range, search, status, page — live in the URL (see
 * `tracesViewState.ts`); live-tail, view mode and the compare selection stay
 * local. Feeds two `useTraces` queries (paged list + wider chart window) that
 * share every filter so the charts are drawn from the same set as the list.
 */
export function useTracesController({ enabled }: UseTracesControllerArgs) {
  const [searchParams, setSearchParams] = useSearchParams();
  const {
    timeRange,
    search,
    status,
    page: currentPage
  } = useMemo(() => readTracesViewState(searchParams), [searchParams]);

  // The box holds what is being typed; the URL holds what is being searched.
  const [searchInput, setSearchInput] = useState(search);
  const [live, setLive] = useState(false);
  const [view, setView] = useState<TraceView>("card");
  const [selectedIds, setSelectedIds] = useState<string[]>([]);

  // `replace`, like the coordinator's Runs filters: Back leaves the page rather
  // than replaying filter edits, and returning from a trace lands on the view
  // that was left. Any change reshapes the result set, so it drops the selection.
  const patchView = useCallback(
    (patch: Partial<TracesViewState>) => {
      setSearchParams((prev) => writeTracesViewState(prev, patch), { replace: true });
      setSelectedIds([]);
    },
    [setSearchParams]
  );

  // What this hook last put in the URL. A `search` that is not that came from
  // outside — the sidebar's own Traces link, a pasted address — and the box has
  // to follow it, or the debounce below would write the stale text straight back.
  const writtenSearch = useRef(search);
  useEffect(() => {
    if (search === writtenSearch.current) return;
    writtenSearch.current = search;
    setSearchInput(search);
  }, [search]);

  // Debounce the search box so keystrokes don't hammer the API.
  useEffect(() => {
    const next = searchInput.trim();
    if (next === search) return;
    const timer = setTimeout(() => {
      writtenSearch.current = next;
      patchView({ search: next, page: 1 });
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [searchInput, search, patchView]);

  const apiStatus = statusFilterToApi(status) ?? "all";
  const range =
    timeRange.kind === "custom"
      ? { duration: undefined, from: timeRange.from, to: timeRange.to }
      : { duration: timeRange.value, from: undefined, to: undefined };
  // Pause live-tail while a compare selection is in progress: an incoming trace
  // must not scroll a selected row off the page and strand the selection (the
  // bar would count a trace the Compare action can no longer resolve).
  const refetchInterval: number | false =
    live && selectedIds.length === 0 ? LIVE_INTERVAL_MS : false;
  const offset = (currentPage - 1) * PAGE_SIZE;

  const sharedFilters = {
    status: apiStatus,
    enabled,
    duration: range.duration,
    from: range.from,
    to: range.to,
    search,
    refetchInterval
  };

  const listQuery = useTraces({ ...sharedFilters, limit: PAGE_SIZE, offset });
  const chartQuery = useTraces({ ...sharedFilters, limit: CHART_LIMIT, offset: 0 });

  const traces = listQuery.data?.items;
  const total = listQuery.data?.total ?? 0;

  // A link can name a page the result set no longer has — the window rolled, or
  // the list shrank. Land on the last real page instead of an empty one that
  // reads as "no traces" under a pager that says otherwise. Only once the list
  // has answered: before that there is no total to clamp against.
  const loaded = listQuery.data !== undefined;
  const lastPage = Math.max(1, Math.ceil(total / PAGE_SIZE));
  useEffect(() => {
    if (loaded && currentPage > lastPage) patchView({ page: lastPage });
  }, [loaded, currentPage, lastPage, patchView]);

  // Any refetch (window focus, a late in-flight poll) can drop a selected trace
  // off the current page. Prune selection to what's actually visible so
  // selectedIds, compareTraces, and the selection cap never disagree.
  useEffect(() => {
    if (!traces) return;
    setSelectedIds((prev) => {
      const visible = prev.filter((id) => traces.some((t) => t.traceId === id));
      return visible.length === prev.length ? prev : visible;
    });
  }, [traces]);

  const toggleSelect = (id: string) =>
    setSelectedIds((prev) => {
      if (prev.includes(id)) return prev.filter((x) => x !== id);
      if (prev.length >= MAX_COMPARE) return prev;
      return [...prev, id];
    });

  const compareTraces = useMemo<Trace[]>(
    () => traces?.filter((t) => selectedIds.includes(t.traceId)) ?? [],
    [traces, selectedIds]
  );

  const filtersActive = search.length > 0 || status !== "all" || timeRange.kind === "custom";

  return {
    timeRange,
    // A filter change reshapes the result set → back to page 1.
    setTimeRange: (next: TimeRange) => patchView({ timeRange: next, page: 1 }),
    searchInput,
    setSearchInput,
    status,
    setStatus: (next: StatusFilter) => patchView({ status: next, page: 1 }),
    live,
    setLive,
    view,
    setView,
    traces,
    total,
    isLoading: listQuery.isLoading,
    chartTraces: chartQuery.data?.items,
    chartTotal: chartQuery.data?.total,
    isChartLoading: chartQuery.isLoading,
    currentPage,
    pageSize: PAGE_SIZE,
    handlePageChange: (page: number) => patchView({ page }),
    selectedIds,
    toggleSelect,
    clearSelection: () => setSelectedIds([]),
    compareTraces,
    filtersActive
  };
}
