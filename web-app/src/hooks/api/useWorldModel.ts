import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { WorldModelService } from "@/services/api/worldModel";
import type {
  WmComputedMeasure,
  WmEntityCount,
  WmInstanceDetail,
  WmInstanceDetailEvent,
  WmMeasureBreakdown,
  WmMeasureBreakdownEvent,
  WmMeasureName
} from "@/types/worldModel";
import queryKeys from "./queryKey";

export function useWorldModel() {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;
  return useQuery({
    queryKey: queryKeys.worldModel.graph(projectId, branchName),
    queryFn: () => WorldModelService.getWorldModel(projectId, branchName),
    staleTime: 5 * 60 * 1000,
    retry: false,
    // Harden against a partial response (e.g. a workspace whose semantic model
    // has no relationships, where `edges` comes back null/absent): the graph
    // iterates `entities`/`edges`, so default them to arrays rather than
    // crashing with "model.edges is not iterable".
    select: (data) => ({
      ...data,
      entities: data?.entities ?? [],
      edges: data?.edges ?? []
    })
  });
}

export function useWmInstances(entityId: string | null, search: string) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;
  return useQuery({
    queryKey: queryKeys.worldModel.instances(projectId, branchName, entityId ?? "", search),
    queryFn: () => {
      if (!entityId) throw new Error("An entity is required");
      return WorldModelService.getInstances(projectId, entityId, search, 50, branchName);
    },
    enabled: !!entityId,
    staleTime: 60 * 1000,
    retry: false
  });
}

/** Page size for the sample browser (mirrors the backend `default_limit`). */
export const WM_FILTER_INSTANCES_PAGE_SIZE = 50;

/**
 * Paginated, searchable list of the rows of `entityId` reachable from the
 * selected instance (`seedEntityId` / `seedKey`). Backs the node-card "+N more"
 * sample browser. `page` (0-based) drives the backend `offset`; `has_more` in
 * the response indicates a next page exists. Disabled until seed + target known.
 */
export function useWmFilterInstances(
  seedEntityId: string | null,
  seedKey: string | null,
  entityId: string | null,
  search: string,
  page = 0
) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;
  const offset = page * WM_FILTER_INSTANCES_PAGE_SIZE;
  return useQuery({
    queryKey: queryKeys.worldModel.filterInstances(
      projectId,
      branchName,
      seedEntityId ?? "",
      seedKey ?? "",
      entityId ?? "",
      search,
      offset
    ),
    queryFn: () => {
      if (!seedEntityId || !seedKey || !entityId) {
        throw new Error("A seed instance and target entity are required");
      }
      return WorldModelService.getFilterInstances(
        projectId,
        seedEntityId,
        seedKey,
        entityId,
        search,
        WM_FILTER_INSTANCES_PAGE_SIZE,
        offset,
        branchName
      );
    },
    enabled: !!seedEntityId && !!seedKey && !!entityId,
    staleTime: 60 * 1000,
    retry: false
  });
}

export function useWmFilterCounts(entityId: string | null, keyValue: string | null) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;

  const [counts, setCounts] = useState<Record<string, WmEntityCount> | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  // Buffer totals until matched arrives so nodes never flash "0 / N".
  const bufferRef = useRef<
    Record<string, { matched?: number; total?: number; sample?: string[]; sample_keys?: string[] }>
  >({});

  const reset = useCallback(() => {
    abortRef.current?.abort();
    bufferRef.current = {};
    setCounts(null);
    setIsLoading(false);
    setError(null);
  }, []);

  useEffect(() => {
    if (!entityId || !keyValue) {
      reset();
      return;
    }

    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    bufferRef.current = {};
    setCounts(null);
    setIsLoading(true);
    setError(null);

    WorldModelService.streamFilterCounts(
      projectId,
      entityId,
      keyValue,
      (event) => {
        if (event.done) {
          setIsLoading(false);
          return;
        }
        const buf = bufferRef.current;
        const entry = buf[event.entity_name] ?? {};
        if (event.total !== undefined) entry.total = event.total;
        if (event.matched !== undefined) entry.matched = event.matched;
        if (event.sample !== undefined) entry.sample = event.sample;
        if (event.sample_keys !== undefined) entry.sample_keys = event.sample_keys;
        buf[event.entity_name] = entry;
        // Only surface the count once matched is known — avoids the "0 / N" flash.
        // Samples arrive on the same (matched) event, so this gate doesn't delay them.
        const { matched, total } = entry;
        if (matched !== undefined && total !== undefined) {
          setCounts((prev) => ({
            ...prev,
            [event.entity_name]: {
              matched,
              total,
              sample: entry.sample,
              sample_keys: entry.sample_keys
            }
          }));
        }
      },
      () => setIsLoading(false),
      (streamError) => {
        // An abort never reports a failure, but one that settled just before this
        // stream was superseded must not land on the seed that replaced it.
        if (controller.signal.aborted) return;
        setError(streamError);
        setIsLoading(false);
      },
      controller.signal,
      branchName
    );

    return () => {
      controller.abort();
    };
  }, [projectId, branchName, entityId, keyValue, reset]);

  return { counts, isLoading, error };
}

/**
 * Accumulator for the instance-detail SSE stream. Besides the assembled `data`,
 * it carries two buffers for events that arrive before `init`:
 *  - `pendingMeasureNames` — the schema-only `measure_names` event (always pre-init).
 *  - `pendingMeasures` — `measure` events that raced ahead of `init`. A measure
 *    group whose SQL failed to compile is emitted instantly (no DB round-trip),
 *    so it can beat the attrs-query-gated `init`. Applying a measure while `data`
 *    is still null is a no-op, so without buffering those values are lost and
 *    their rows pulse as skeletons forever.
 */
export interface WmInstanceDetailState {
  data: WmInstanceDetail | null;
  pendingMeasureNames: WmMeasureName[] | null;
  pendingMeasures: WmComputedMeasure[];
  initialized: boolean;
}

const EMPTY_INSTANCE_DETAIL_STATE: WmInstanceDetailState = {
  data: null,
  pendingMeasureNames: null,
  pendingMeasures: [],
  initialized: false
};

// Replace measures by name with the arrived values, preserving the original order.
function mergeMeasures(
  measures: WmComputedMeasure[],
  arrived: WmComputedMeasure[]
): WmComputedMeasure[] {
  const byName = new Map(arrived.map((m) => [m.name, m]));
  return measures.map((m) => byName.get(m.name) ?? m);
}

/**
 * Pure reducer folding one instance-detail SSE event into the accumulator.
 * `done` is handled by the caller (it only flips the loading flag). Events that
 * arrive before `init` are buffered rather than dropped — see {@link WmInstanceDetailState}.
 */
export function applyInstanceDetailEvent(
  state: WmInstanceDetailState,
  event: WmInstanceDetailEvent
): WmInstanceDetailState {
  switch (event.kind) {
    case "measure_names":
      return { ...state, pendingMeasureNames: event.measure_names };
    case "init": {
      const buffered = new Map(state.pendingMeasures.map((m) => [m.name, m]));
      const computed_measures = (state.pendingMeasureNames ?? []).map(
        (n): WmComputedMeasure =>
          buffered.get(n.name) ?? {
            name: n.name,
            measure_type: n.measure_type,
            value: null,
            fiber_count: 0,
            label: n.label
          }
      );
      return {
        ...state,
        initialized: true,
        pendingMeasures: [],
        data: {
          entity_id: event.entity_id,
          key_value: event.key_value,
          display: event.display,
          attributes: event.attributes,
          promotes_to: [],
          receives_from: [],
          computed_measures
        }
      };
    }
    case "parent":
      return state.data
        ? { ...state, data: { ...state.data, promotes_to: event.promotes_to } }
        : state;
    case "child":
      return state.data
        ? {
            ...state,
            data: { ...state.data, receives_from: [...state.data.receives_from, event.child] }
          }
        : state;
    case "measure":
      // Raced ahead of init — buffer until init seeds the skeleton rows.
      if (!state.initialized || !state.data) {
        return {
          ...state,
          pendingMeasures: [...state.pendingMeasures, ...event.computed_measures]
        };
      }
      return {
        ...state,
        data: {
          ...state.data,
          computed_measures: mergeMeasures(state.data.computed_measures, event.computed_measures)
        }
      };
    default:
      return state;
  }
}

/**
 * Stream one instance's detail as a **keyed React Query subscription**, the way
 * {@link useWmMeasureBreakdown} streams a breakdown. The page mounts this hook
 * for the filter seed (measure chips on the entity cards) and the detail panel
 * mounts it for the selected instance — usually the same one. React Query puts
 * both on a single query, so the stream runs once and both read the same
 * `data`, the same `isLoading` and the same `error`; each used to open a stream
 * of its own, and the page dropped its copy's failure.
 *
 * Each event is folded into the cache via `setQueryData`, so subscribers
 * re-render as sections arrive, and `isLoading` stays true until the stream is
 * done. A stream that fails, or ends before `done`, rejects the query: `error`
 * is set, `data` keeps what had arrived, and the partial instance is not cached
 * as a finished one.
 */
export function useWmInstanceDetail(entityId: string | null, keyValue: string | null) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;
  const queryClient = useQueryClient();

  const queryKey = queryKeys.worldModel.instanceDetail(
    projectId,
    branchName,
    entityId ?? "",
    keyValue ?? ""
  );

  const query = useQuery<WmInstanceDetail | null>({
    queryKey,
    enabled: !!entityId && !!keyValue,
    // The stream runs several warehouse queries; keep the assembled instance
    // briefly so a consumer mounting late reuses it instead of re-streaming.
    staleTime: 60 * 1000,
    retry: false,
    // An instance is streamed when it is chosen. Coming back to the window must
    // not re-run its queries and blank the measures that had already arrived.
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    // React Query aborts `signal` when the last consumer unmounts mid-flight or
    // the instance changes, which ends the stream. Nothing a cancelled stream
    // reports after that reaches the instance that replaced it.
    queryFn: ({ signal }) =>
      new Promise<WmInstanceDetail | null>((resolve, reject) => {
        let state = EMPTY_INSTANCE_DETAIL_STATE;
        WorldModelService.streamInstanceDetail(
          projectId,
          entityId as string,
          keyValue as string,
          (event) => {
            if (event.kind === "done") {
              resolve(state.data);
              return;
            }
            state = applyInstanceDetailEvent(state, event);
            // Publish what has arrived to every subscriber of this instance.
            // Before `init` there is no instance yet, only buffered events.
            if (state.data) queryClient.setQueryData(queryKey, state.data);
          },
          // The server always ends with `done`, which has resolved by now (settling
          // twice is a no-op). A close without it is a stream cut short.
          () => reject(new Error("Instance detail stream ended before it finished")),
          reject,
          signal,
          branchName
        );
      })
  });

  return { data: query.data ?? null, isLoading: query.isFetching, error: query.error };
}

/**
 * Pure reducer folding measure-breakdown SSE events into the assembled tree.
 * `init` seeds every node with a null value; `value` fills one node by id.
 */
export function applyBreakdownEvent(
  state: WmMeasureBreakdown | null,
  event: WmMeasureBreakdownEvent
): WmMeasureBreakdown {
  if (event.kind === "init") {
    return {
      root: event.root,
      edges: event.edges,
      nodes: event.nodes.map((n) => ({ ...n, value: null, unvalued_reason: null }))
    };
  }
  if (event.kind === "value" && state) {
    return {
      ...state,
      nodes: state.nodes.map((n) =>
        n.id === event.node_id
          ? { ...n, value: event.value, unvalued_reason: event.unvalued_reason }
          : n
      )
    };
  }
  return state ?? { root: "", nodes: [], edges: [] };
}

/**
 * Stream a measure's breakdown, valued at one instance, as a **keyed React Query
 * subscription**. The graph node and the detail-panel driver tree both mount this
 * hook with the same `(entityId, keyValue, measure)`; React Query dedups them onto
 * a single in-flight query, so the (expensive) breakdown SSE stream runs once and
 * both consumers share it. Progressive SSE events are folded into the cache via
 * `setQueryData` so subscribers re-render as values arrive, and the query resolves
 * with the fully assembled tree on `done`. A stream that fails, or ends before
 * `done`, rejects the query: `error` is set and the partial tree is not cached as
 * a finished one (`data` may still hold what arrived, its unvalued nodes null).
 */
export function useWmMeasureBreakdown(
  entityId: string | null,
  keyValue: string | null,
  measureName: string | null
) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;
  const queryClient = useQueryClient();

  const enabled = !!entityId && !!keyValue && !!measureName;
  const queryKey = queryKeys.worldModel.measureBreakdown(
    projectId,
    branchName,
    entityId ?? "",
    keyValue ?? "",
    measureName ?? ""
  );

  const query = useQuery<WmMeasureBreakdown | null>({
    queryKey,
    enabled,
    // The stream is instance-valued and non-trivially expensive; keep the
    // assembled tree cached briefly so a re-mount (or the second consumer
    // mounting late) reuses it instead of re-streaming.
    staleTime: 60 * 1000,
    retry: false,
    queryFn: ({ signal }) =>
      new Promise<WmMeasureBreakdown | null>((resolve, reject) => {
        // React Query cancels the query (aborts `signal`) when the last
        // observer unmounts mid-flight or the key changes — forward that to
        // the underlying SSE fetch so we don't leak a stream.
        const controller = new AbortController();
        if (signal.aborted) controller.abort();
        else signal.addEventListener("abort", () => controller.abort());

        let assembled: WmMeasureBreakdown | null = null;
        WorldModelService.streamMeasureBreakdown(
          projectId,
          entityId as string,
          keyValue as string,
          measureName as string,
          (event) => {
            if (event.kind === "done") {
              resolve(assembled);
              return;
            }
            assembled = applyBreakdownEvent(assembled, event);
            // Publish each partial tree to every subscriber of this key.
            queryClient.setQueryData(queryKey, assembled);
          },
          // The server always ends with `done`, which has resolved by now (settling
          // twice is a no-op). A close without it is a stream cut short.
          () => reject(new Error("Measure breakdown stream ended before it finished")),
          reject,
          controller.signal,
          branchName
        );
      })
  });

  return { data: query.data ?? null, isLoading: query.isLoading, error: query.error };
}
