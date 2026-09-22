// React hooks for the world-model surface, exposed to custom-app bundles
// so an app can render the semantic-model graph, browse an entity's
// instances, and drill into an instance's detail / measure driver-tree.
//
// Wraps the `/api/projects/{id}/semantic/world-model*` endpoints behind the
// shared `OxyAppProvider` fetcher. The two drill-down endpoints
// (`instance-detail`, `measure-breakdown`) stream `kind`-tagged SSE events;
// their hooks fold the stream into accumulated state so a bundle can render
// progressively (skeletons fill in as measure values resolve).

import * as React from "react";
import type {
  WmInstancesResponse,
  WmMeasureBreakdown,
  WmMeasureBreakdownEvent,
  WorldModel
} from "../worldModel";
import { apiErrorFromResponse, asReportableError } from "./errors";
import { useOxyApp } from "./react";
import { readJsonSseStream } from "./sse";

/** Base path for the world-model endpoints of the active project. */
function worldModelPath(projectId: string): string {
  return `/api/projects/${projectId}/semantic/world-model`;
}

// ── useWorldModelGraph (graph) ────────────────────────────────────────────────

export interface UseWorldModelGraphResult {
  data: WorldModel | null;
  loading: boolean;
  error: Error | null;
  refetch: () => void;
}

/**
 * The world-model graph — entities (nodes), their measures/dimensions, and
 * how measures promote across the entity hierarchy (edges). Applies the
 * project's `.world-model.yml` display config server-side.
 *
 * @remarks
 * This returns the raw semantic-model entity graph. For the higher-level
 * node-paradigm interface (`world.metric(id)` speaking `expand` / `explain` /
 * `size`), use {@link useWorldModel} from `./world-node` instead.
 */
export function useWorldModelGraph(opts: { enabled?: boolean } = {}): UseWorldModelGraphResult {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const [data, setData] = React.useState<WorldModel | null>(null);
  const [loading, setLoading] = React.useState<boolean>(enabled && !!projectId);
  const [error, setError] = React.useState<Error | null>(null);
  const [nonce, setNonce] = React.useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: nonce is refetch's re-run trigger, not a value the effect reads
  React.useEffect(() => {
    if (!enabled || !projectId) {
      setLoading(false);
      return;
    }
    const ctrl = new AbortController();
    let cancelled = false;
    setLoading(true);
    setError(null);
    fetcher(worldModelPath(projectId), { method: "GET", signal: ctrl.signal })
      .then(async (resp) => {
        if (!resp.ok) throw await apiErrorFromResponse(resp);
        return (await resp.json()) as WorldModel;
      })
      .then((result) => {
        if (cancelled) return;
        setData(result);
        setLoading(false);
      })
      .catch((err: unknown) => {
        // Only our own teardown is silent, and `cancelled` — set next to
        // `ctrl.abort()` — marks it. Any other abort is a real failure, and
        // matching it by name left the hook loading forever with no error.
        if (cancelled) return;
        setError(asReportableError(err));
        setLoading(false);
      });
    return () => {
      cancelled = true;
      ctrl.abort();
    };
    // `nonce` is what `refetch()` bumps; leaving it out made refetch a no-op.
  }, [enabled, projectId, fetcher, nonce]);

  const refetch = React.useCallback(() => setNonce((n) => n + 1), []);
  return { data, loading, error, refetch };
}

// ── useWorldModelInstances ────────────────────────────────────────────────────

export interface UseWorldModelInstancesOpts {
  /** Substring/prefix search over the entity's display field. */
  search?: string;
  /** Max rows to return (default 50 server-side). */
  limit?: number;
  enabled?: boolean;
  /**
   * `"reach"` — only the instances whose place the viewer reaches, filtered
   * inside the scan so a page is a page of the right set. Needs the entity
   * bound to the org's locations registry (refused otherwise); an instance
   * whose key is unmapped is not in anyone's reach. The bundle's own app is
   * sent along so app-admin standing counts.
   */
  scope?: "reach";
}

export interface UseWorldModelInstancesResult {
  data: WmInstancesResponse | null;
  loading: boolean;
  error: Error | null;
  refetch: () => void;
}

/**
 * List the instances (rows) of `entityId` — a bounded, searchable picker
 * over the entity's primary keys + display label. Pass `null` for `entityId`
 * to stay idle until an entity is chosen.
 */
export function useWorldModelInstances(
  entityId: string | null,
  opts: UseWorldModelInstancesOpts = {}
): UseWorldModelInstancesResult {
  const { projectId, appId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const { search, limit, scope } = opts;
  const [data, setData] = React.useState<WmInstancesResponse | null>(null);
  const [loading, setLoading] = React.useState<boolean>(enabled && !!projectId && !!entityId);
  const [error, setError] = React.useState<Error | null>(null);
  const [nonce, setNonce] = React.useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: nonce is refetch's re-run trigger, not a value the effect reads
  React.useEffect(() => {
    if (!enabled || !projectId || !entityId) {
      setLoading(false);
      return;
    }
    const ctrl = new AbortController();
    let cancelled = false;
    setLoading(true);
    setError(null);
    const params = new URLSearchParams({ entity: entityId });
    if (search) params.set("search", search);
    if (limit != null) params.set("limit", String(limit));
    if (scope) {
      params.set("scope", scope);
      if (appId) params.set("app", appId);
    }
    fetcher(`${worldModelPath(projectId)}/instances?${params}`, {
      method: "GET",
      signal: ctrl.signal
    })
      .then(async (resp) => {
        if (!resp.ok) throw await apiErrorFromResponse(resp);
        return (await resp.json()) as WmInstancesResponse;
      })
      .then((result) => {
        if (cancelled) return;
        setData(result);
        setLoading(false);
      })
      .catch((err: unknown) => {
        // Only our own teardown is silent, and `cancelled` — set next to
        // `ctrl.abort()` — marks it. Any other abort is a real failure, and
        // matching it by name left the hook loading forever with no error.
        if (cancelled) return;
        setError(asReportableError(err));
        setLoading(false);
      });
    return () => {
      cancelled = true;
      ctrl.abort();
    };
    // `nonce` is what `refetch()` bumps; leaving it out made refetch a no-op.
  }, [enabled, projectId, entityId, search, limit, fetcher, scope, appId, nonce]);

  const refetch = React.useCallback(() => setNonce((n) => n + 1), []);
  return { data, loading, error, refetch };
}

// ── useMeasureBreakdown (driver tree, SSE) ────────────────────────────────────

export interface UseMeasureBreakdownResult {
  /** Accumulated breakdown graph; null until the `init` frame. */
  breakdown: WmMeasureBreakdown | null;
  loading: boolean;
  done: boolean;
  error: Error | null;
}

/** Fold one measure-breakdown SSE frame into the accumulated graph. */
function foldBreakdown(
  prev: WmMeasureBreakdown | null,
  ev: WmMeasureBreakdownEvent
): WmMeasureBreakdown | null {
  switch (ev.kind) {
    case "init":
      return {
        root: ev.root,
        nodes: ev.nodes.map((n) => ({ ...n, value: null, unvalued_reason: null })),
        edges: ev.edges
      };
    case "value": {
      if (!prev) return prev;
      return {
        ...prev,
        nodes: prev.nodes.map((n) =>
          n.id === ev.node_id ? { ...n, value: ev.value, unvalued_reason: ev.unvalued_reason } : n
        )
      };
    }
    default:
      return prev;
  }
}

/**
 * Stream the driver-tree breakdown of one instance's measure — the metric
 * decomposition (add/sub/mul/div component graph) with each node's value
 * filling in as it resolves. This is the per-instance RCA view. Pass `null`
 * for `measure` to stay idle.
 */
export function useMeasureBreakdown(
  entityId: string | null,
  keyValue: string | null,
  measure: string | null
): UseMeasureBreakdownResult {
  const { projectId, fetcher } = useOxyApp();
  const [breakdown, setBreakdown] = React.useState<WmMeasureBreakdown | null>(null);
  const [loading, setLoading] = React.useState<boolean>(false);
  const [done, setDone] = React.useState<boolean>(false);
  const [error, setError] = React.useState<Error | null>(null);

  React.useEffect(() => {
    if (!projectId || !entityId || !keyValue || !measure) {
      setLoading(false);
      return;
    }
    const ctrl = new AbortController();
    let cancelled = false;
    setBreakdown(null);
    setLoading(true);
    setDone(false);
    setError(null);

    const params = new URLSearchParams({ entity: entityId, key: keyValue, measure });
    fetcher(`${worldModelPath(projectId)}/measure-breakdown?${params}`, {
      method: "GET",
      signal: ctrl.signal
    })
      .then(async (resp) => {
        if (!resp.ok) throw await apiErrorFromResponse(resp);
        await readJsonSseStream<WmMeasureBreakdownEvent>(resp, (ev) => {
          if (cancelled) return;
          if (ev.kind === "done") {
            setDone(true);
            return;
          }
          setBreakdown((prev) => foldBreakdown(prev, ev));
        });
        if (!cancelled) setLoading(false);
      })
      .catch((err: unknown) => {
        // Only our own teardown is silent, and `cancelled` — set next to
        // `ctrl.abort()` — marks it. Any other abort is a real failure, and
        // matching it by name left the hook loading forever with no error.
        if (cancelled) return;
        setError(asReportableError(err));
        setLoading(false);
      });
    return () => {
      cancelled = true;
      ctrl.abort();
    };
  }, [projectId, entityId, keyValue, measure, fetcher]);

  return { breakdown, loading, done, error };
}
