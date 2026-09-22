// React hooks for the metric-tree analysis ops, exposed to custom-app
// bundles so an app can run drivers / what-if / RCA / opportunity sizing
// without hand-rolling fetch against the semantic model.
//
// These wrap the `/api/projects/{id}/semantic/metric-tree*` endpoints —
// the same airlayer analyses the IDE's World Model and Metric Tree
// surfaces drive — behind the shared `OxyAppProvider` fetcher (session
// cookie in-workspace, dev-proxy token cross-origin). Response shapes
// reuse the wire types in `../metricTree`, so a bundle typed against a
// hook result matches what the server serializes verbatim.
//
// Pattern mirrors `useSemanticQuery`: each hook fetches when enabled and
// its input is present, re-runs on input change (deep-compared via JSON),
// and exposes `refetch`. Read-only inputs (`null`) keep a hook idle — the
// natural fit for "run once the user picks a target measure".

import * as React from "react";
import type {
  BaselineRequest,
  BaselineResponse,
  DistributionRequest,
  ExplainRequest,
  ExplainResult,
  MetricTree,
  OpportunityRequest,
  OpportunityResult,
  PredictChange,
  PredictOptions,
  PredictResult,
  ProjectionRequest,
  ProjectionResponse,
  SensitivityResult,
  TimeDimensionsResponse
} from "../metricTree";
import { asReportableError } from "./errors";
import { getJson, metricTreePath, postJson } from "./metric-tree-fetch";
import { useOxyApp } from "./react";

/** Shared result envelope for every metric-tree hook. */
export interface MetricTreeHookResult<Data> {
  data: Data | null;
  loading: boolean;
  error: Error | null;
  /** Force a re-run, bypassing nothing — the server honors `?refresh`. */
  refetch: () => void;
}

interface EndpointOpts {
  /** Set false to skip the request (e.g. waiting on a user selection). */
  enabled?: boolean;
}

/**
 * Internal engine shared by every metric-tree hook. Runs `run(signal)`
 * whenever `key` changes (and on `refetch`), tracks loading/error, and
 * cancels in-flight work on unmount or input change.
 *
 * `key` is the deep-compare fingerprint of the request; a `null` key
 * means "no request yet" and leaves the hook idle without firing.
 */
function useMetricTreeEndpoint<Data>(
  key: string | null,
  run: (signal: AbortSignal) => Promise<Data>,
  enabled: boolean
): MetricTreeHookResult<Data> {
  const [data, setData] = React.useState<Data | null>(null);
  const [loading, setLoading] = React.useState<boolean>(enabled && key !== null);
  const [error, setError] = React.useState<Error | null>(null);
  const [nonce, setNonce] = React.useState(0);

  // `run` is re-created each render; pin the latest in a ref so the effect
  // depends only on `key`/`enabled`/`nonce` and doesn't re-fire on every
  // parent render.
  const runRef = React.useRef(run);
  runRef.current = run;

  // biome-ignore lint/correctness/useExhaustiveDependencies: nonce is refetch's re-run trigger, not a value the effect reads
  React.useEffect(() => {
    if (!enabled || key === null) {
      setLoading(false);
      return;
    }
    const ctrl = new AbortController();
    let cancelled = false;
    setLoading(true);
    setError(null);

    runRef
      .current(ctrl.signal)
      .then((result) => {
        if (cancelled) return;
        setData(result);
        setLoading(false);
      })
      .catch((err: unknown) => {
        // `cancelled` marks the runs WE tore down (it is set next to
        // `ctrl.abort()`); nothing else earns silence. Matching `AbortError`
        // by name also swallowed aborts we did not cause and pinned the hook
        // on `loading: true` with no error.
        if (cancelled) return;
        setError(asReportableError(err));
        setLoading(false);
      });

    return () => {
      cancelled = true;
      ctrl.abort();
    };
    // `nonce` is what `refetch()` bumps — without it here the effect never
    // re-ran and a failed analysis could not be retried at all.
  }, [key, enabled, nonce]);

  const refetch = React.useCallback(() => setNonce((n) => n + 1), []);
  return { data, loading, error, refetch };
}

// ── useMetricTree ─────────────────────────────────────────────────────────────

export interface UseMetricTreeOpts extends EndpointOpts {
  /** Optional measure id to root the returned subtree at. */
  root?: string;
}

/**
 * The project's metric tree — measures (nodes) and their component /
 * driver relationships (edges) — or the subtree rooted at `opts.root`.
 * The structural backbone every other metric-tree analysis reads against.
 */
export function useMetricTree(opts: UseMetricTreeOpts = {}): MetricTreeHookResult<MetricTree> {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const root = opts.root;
  const key = projectId ? JSON.stringify({ projectId, root }) : null;

  return useMetricTreeEndpoint<MetricTree>(
    key,
    (signal) => {
      const qs = root ? `?root=${encodeURIComponent(root)}` : "";
      return getJson<MetricTree>(fetcher, `${metricTreePath(projectId as string)}${qs}`, signal);
    },
    enabled
  );
}

// ── useSensitivity (drivers) ──────────────────────────────────────────────────

/**
 * Ranked drivers of `measureId`, by influence — the "what moves this
 * measure" question. Pass `null` to stay idle until a measure is chosen.
 */
export function useSensitivity(
  measureId: string | null,
  opts: EndpointOpts = {}
): MetricTreeHookResult<SensitivityResult> {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const key = projectId && measureId ? JSON.stringify({ projectId, measureId }) : null;

  return useMetricTreeEndpoint<SensitivityResult>(
    key,
    (signal) => {
      const path = `${metricTreePath(projectId as string)}/${encodeURIComponent(
        measureId as string
      )}/sensitivity`;
      return getJson<SensitivityResult>(fetcher, path, signal);
    },
    enabled
  );
}

// ── usePredict (what-if) ──────────────────────────────────────────────────────

export interface UsePredictOpts extends EndpointOpts, PredictOptions {}

/**
 * Propagate hypothetical `(measure, delta)` changes upward through the
 * tree and return the estimated impact on every downstream measure — a
 * pure metric-tree walk, no warehouse query. Pass `null` to stay idle.
 *
 * Because it is database-free it can only use the coefficients it is GIVEN.
 * Without `opts.coefficients` from {@link useBaseline}, every driver edge
 * whose `.view.yml` declares no `coefficient:` contributes nothing and its
 * downstream measures are simply absent from `impacts` — no error, no
 * refusal. Without `opts.values`, multiplicative component edges come back
 * `unquantifiable` rather than sized.
 */
export function usePredict(
  changes: PredictChange[] | null,
  opts: UsePredictOpts = {}
): MetricTreeHookResult<PredictResult> {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const { values, coefficients } = opts;
  const body = {
    changes,
    ...(values ? { values } : {}),
    // Sent verbatim, refusals included — the server ignores entries carrying
    // no coefficient, and filtering them here would just be a second place for
    // the two sides to disagree.
    ...(coefficients?.length ? { coefficients } : {})
  };
  const key = projectId && changes ? JSON.stringify({ projectId, body }) : null;

  return useMetricTreeEndpoint<PredictResult>(
    key,
    (signal) =>
      postJson<PredictResult>(
        fetcher,
        `${metricTreePath(projectId as string)}/predict`,
        body,
        signal
      ),
    enabled
  );
}

// ── useBaseline (scenario levels + fitted coefficients) ───────────────────────

/**
 * Value a scenario's starting point, and measure the coefficients it needs.
 *
 * Two warehouse reads: the current value of every node reachable from
 * `request.roots`, and — for driver edges declaring no `coefficient:` — a fit
 * over the window. Both are expensive, which is why they live here and not in
 * {@link usePredict}: predict is database-free by design so it can re-run per
 * keystroke, and it CANNOT measure a coefficient itself.
 *
 * That is the whole reason to call this. Feed `data.values` and `data.fitted`
 * into `usePredict`; omit them and an undeclared edge propagates nothing.
 * Pass `null` to stay idle until levers and a period are chosen.
 */
export function useBaseline(
  request: BaselineRequest | null,
  opts: EndpointOpts = {}
): MetricTreeHookResult<BaselineResponse> {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const key = projectId && request ? JSON.stringify({ projectId, request }) : null;

  return useMetricTreeEndpoint<BaselineResponse>(
    key,
    (signal) =>
      postJson<BaselineResponse>(
        fetcher,
        `${metricTreePath(projectId as string)}/baseline`,
        request,
        signal
      ),
    enabled
  );
}

// ── useProjection (scenario forecasting over time) ────────────────────────────

/**
 * Bucketed history for the levers and everything downstream, plus the forward
 * curve the forecaster expects next — the scenario's time axis.
 *
 * One warehouse query, so it belongs on a window change, not on a lever edit.
 * It returns the BASELINE curve only: the scenario's second curve is
 * arithmetic over this and a `usePredict` result — a proportional shift
 * landing `lag` buckets in — composed client-side precisely so editing a lever
 * costs no query.
 *
 * Treat a series with a `refusal` as a stated absence: it must not render as a
 * flat forward line. Pass `null` to stay idle.
 */
export function useProjection(
  request: ProjectionRequest | null,
  opts: EndpointOpts = {}
): MetricTreeHookResult<ProjectionResponse> {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const key = projectId && request ? JSON.stringify({ projectId, request }) : null;

  return useMetricTreeEndpoint<ProjectionResponse>(
    key,
    (signal) =>
      postJson<ProjectionResponse>(
        fetcher,
        `${metricTreePath(projectId as string)}/projection`,
        request,
        signal
      ),
    enabled
  );
}

// ── useExplain (RCA) ──────────────────────────────────────────────────────────

/**
 * Period-over-period root-cause decomposition: recursively splits the
 * target measure by components and dimensions until the move concentrates.
 * This is the heavy one — it can fire many warehouse queries and the
 * server caps it at 45s. Pass `null` to defer until periods are chosen.
 */
export function useExplain(
  request: ExplainRequest | null,
  opts: EndpointOpts = {}
): MetricTreeHookResult<ExplainResult> {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const key = projectId && request ? JSON.stringify({ projectId, request }) : null;

  return useMetricTreeEndpoint<ExplainResult>(
    key,
    (signal) =>
      postJson<ExplainResult>(
        fetcher,
        `${metricTreePath(projectId as string)}/explain`,
        request,
        signal
      ),
    enabled
  );
}

// ── useDistribution ───────────────────────────────────────────────────────────

/**
 * Single-period distribution of a measure — an {@link ExplainResult}
 * against an auto-derived immediately-prior baseline. Same renderers as
 * `useExplain`; ignore the delta fields for a pure distribution view.
 */
export function useDistribution(
  request: DistributionRequest | null,
  opts: EndpointOpts = {}
): MetricTreeHookResult<ExplainResult> {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const key = projectId && request ? JSON.stringify({ projectId, request }) : null;

  return useMetricTreeEndpoint<ExplainResult>(
    key,
    (signal) =>
      postJson<ExplainResult>(
        fetcher,
        `${metricTreePath(projectId as string)}/distribution`,
        request,
        signal
      ),
    enabled
  );
}

// ── useOpportunity (sizing) ───────────────────────────────────────────────────

/**
 * Segment opportunity sizing for a measure over a period: finds
 * underperforming segments and sizes the addressable upside of closing
 * each rate gap against a benchmark peer. Pass `null` to stay idle until
 * a target + period are chosen.
 */
export function useOpportunity(
  request: OpportunityRequest | null,
  opts: EndpointOpts = {}
): MetricTreeHookResult<OpportunityResult> {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const key = projectId && request ? JSON.stringify({ projectId, request }) : null;

  return useMetricTreeEndpoint<OpportunityResult>(
    key,
    (signal) =>
      postJson<OpportunityResult>(
        fetcher,
        `${metricTreePath(projectId as string)}/opportunity`,
        request,
        signal
      ),
    enabled
  );
}

// ── useTimeDimensions ─────────────────────────────────────────────────────────

/**
 * The queryable time dimensions per view (`view.dim` ids) — what a
 * bundle offers as the period axis for `explain` / `opportunity` /
 * `distribution` instead of hardcoding a curated map.
 */
export function useTimeDimensions(
  opts: EndpointOpts = {}
): MetricTreeHookResult<TimeDimensionsResponse> {
  const { projectId, fetcher } = useOxyApp();
  const enabled = opts.enabled !== false;
  const key = projectId ? JSON.stringify({ projectId, kind: "time-dimensions" }) : null;

  return useMetricTreeEndpoint<TimeDimensionsResponse>(
    key,
    (signal) =>
      getJson<TimeDimensionsResponse>(
        fetcher,
        `${metricTreePath(projectId as string)}/time-dimensions`,
        signal
      ),
    enabled
  );
}
