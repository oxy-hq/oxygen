import { useMutation, useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { MetricTreeService } from "@/services/api/metricTree";
import type {
  BaselineRequest,
  DistributionRequest,
  DrillRequest,
  FittedDriver,
  MeasureValues,
  OpportunityRequest,
  PredictChange,
  ProjectionRequest
} from "@/types/metricTree";
import queryKeys from "./queryKey";

/** Options accepted by the discovery hooks, which callers may want to hold off
 *  until a surface is actually opened. */
interface DiscoveryOptions {
  enabled?: boolean;
}

/** The metric tree for the current project, optionally rooted at `root`. */
export function useMetricTree(root?: string, options: DiscoveryOptions = {}) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;

  return useQuery({
    queryKey: queryKeys.metricTree.tree(projectId, branchName, root),
    queryFn: () => MetricTreeService.getTree(projectId, root, branchName),
    enabled: options.enabled ?? true,
    retry: false
  });
}

/** Ranked drivers of `measureId`. Disabled until a measure is selected. */
export function useSensitivity(measureId: string | undefined) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;

  return useQuery({
    queryKey: queryKeys.metricTree.sensitivity(projectId, branchName, measureId),
    queryFn: () => {
      if (!measureId) throw new Error("A measure is required");
      return MetricTreeService.getSensitivity(projectId, measureId, branchName);
    },
    enabled: !!measureId,
    retry: false
  });
}

/** Per-view time dimensions discovered from the semantic model. Cached for
 *  the session — schema rarely changes within a single visit. */
export function useTimeDimensions(options: DiscoveryOptions = {}) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;

  return useQuery({
    queryKey: queryKeys.metricTree.timeDimensions(projectId, branchName),
    queryFn: () => MetricTreeService.timeDimensions(projectId, branchName),
    enabled: options.enabled ?? true,
    staleTime: 5 * 60 * 1000,
    retry: false
  });
}

/** Single-period distribution for a measure. The baseline window is
 *  auto-derived server-side. Pass `null` to disable. */
export function useDistributionQuery(request: DistributionRequest | null, enabled = true) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;

  return useQuery({
    queryKey: request
      ? queryKeys.metricTree.distribution(
          projectId,
          branchName,
          request.target,
          request.time_dimension,
          request.period
        )
      : ([...queryKeys.metricTree.all, "distribution", "idle"] as const),
    queryFn: () => {
      if (!request) throw new Error("distribution request is required");
      return MetricTreeService.distribution(projectId, request, branchName);
    },
    enabled: enabled && !!request,
    staleTime: 5 * 60 * 1000,
    retry: false
  });
}

/** Propagate a hypothetical `(measure, delta)` up the driver graph — the
 *  "what-if" lever. A pure metric-tree walk server-side (no warehouse query),
 *  so it is cheap and safe to re-run on every input change. Passing `values`
 *  sizes multiplicative edges that would otherwise come back `unquantifiable`. */
export function usePredict() {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;

  return useMutation({
    // The branch is load-bearing, not decoration: the workspace extractor
    // resolves a different working copy per branch, so predicting without it
    // walks a different semantic layer than the tree the user is looking at —
    // measure ids silently stop matching on any non-default branch.
    mutationFn: ({
      changes,
      values,
      coefficients
    }: {
      changes: PredictChange[];
      values?: MeasureValues;
      coefficients?: FittedDriver[];
    }) => MetricTreeService.predict(projectId, changes, branchName, { values, coefficients })
  });
}

/** Baseline values for a lever set. Disabled until there is a request. */
export function useBaseline(request: BaselineRequest | null) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;

  return useQuery({
    queryKey: request
      ? queryKeys.metricTree.baseline(
          projectId,
          branchName,
          request.roots,
          request.time_dimension,
          request.period,
          request.instance ? `${request.instance.entity}:${request.instance.key}` : null
        )
      : ([...queryKeys.metricTree.all, "baseline", "idle"] as const),
    queryFn: () => {
      if (!request) throw new Error("a baseline request is required");
      return MetricTreeService.baseline(projectId, request, branchName);
    },
    enabled: !!request && request.roots.length > 0,
    // The warehouse query is expensive and the underlying window is
    // historical, so it does not go stale within a working session.
    staleTime: 5 * 60 * 1000,
    retry: false
  });
}

/** Bucketed history + forward forecast for a lever set. Disabled until there
 *  is a request — the projection panel is opt-in, and a closed panel must not
 *  spend a warehouse query. */
export function useProjection(request: ProjectionRequest | null) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;

  return useQuery({
    queryKey: request
      ? queryKeys.metricTree.projection(
          projectId,
          branchName,
          request.roots,
          request.time_dimension,
          request.period,
          request.instance ? `${request.instance.entity}:${request.instance.key}` : null,
          request.granularity ?? "day",
          request.horizon
        )
      : ([...queryKeys.metricTree.all, "projection", "idle"] as const),
    queryFn: () => {
      if (!request) throw new Error("a projection request is required");
      return MetricTreeService.projection(projectId, request, branchName);
    },
    enabled: !!request && request.roots.length > 0,
    // Same reasoning as `useBaseline`: an expensive query over a historical
    // window, which does not go stale within a working session.
    staleTime: 5 * 60 * 1000,
    retry: false
  });
}

/** Per-segment opportunity sizing of a measure against a benchmark peer, over a
 *  period. Pass `null` to disable — callers gate on having a time dimension and
 *  period.
 *
 *  Read the response by `weight_basis`, which tells you what the numbers mean:
 *  - `"rows"` (sum-like measure, view has a `count` measure): sized on a
 *    per-unit RATE. `current_value` / `benchmark` / `gap` are rates and
 *    `upside` = rate gap × the segment's own row count — an actionable, volume-
 *    aware quantity. This is the honest additive path.
 *  - `"equal"` (rate measure): a spread diagnostic — trust `current_value` /
 *    `benchmark` / `gap` only, not `upside` (equal-weighted, not sized).
 *  - `"value_share"` (avg/min/max): legacy value-share weighting; treat `upside`
 *    as indicative only.
 *  A sum-like measure whose view declares no `count` measure comes back with
 *  empty `dimensions` and a count-related `skipped_dimensions` reason.
 *
 *  Each run issues one warehouse aggregate per *discovered* dimension — the
 *  view's own plus one hop through every foreign entity, which is ~20 on
 *  `orders`, not the 5 that come back. `TOP_K_DIMENSIONS` ranks and truncates
 *  after the scan, it does not bound it. Hence opt-in, and cached for 5 min. */
export function useOpportunityQuery(request: OpportunityRequest | null, enabled = true) {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;

  return useQuery({
    queryKey: request
      ? queryKeys.metricTree.opportunity(
          projectId,
          request.target,
          request.time_dimension,
          request.period,
          request.instance ?? null
        )
      : ([...queryKeys.metricTree.all, "opportunity", "idle"] as const),
    queryFn: () => {
      if (!request) throw new Error("opportunity request is required");
      return MetricTreeService.opportunity(projectId, request);
    },
    enabled: enabled && !!request,
    staleTime: 5 * 60 * 1000,
    retry: false
  });
}

/** Recursive opportunity decomposition (the "drill") for a measure over a
 *  period — walks the gap down through successive component/dimension
 *  candidates until it bottoms out. Pass `null` to disable; the world-model
 *  panel gates this on-expand, since the recursive scan issues a bounded but
 *  non-trivial number of warehouse queries (max_depth × candidates). */
export function useDrillQuery(request: DrillRequest | null, enabled = true) {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;

  return useQuery({
    queryKey: request
      ? queryKeys.metricTree.drill(
          projectId,
          request.target,
          request.time_dimension,
          request.period,
          request.instance ?? null,
          request.root ?? null
        )
      : ([...queryKeys.metricTree.all, "drill", "idle"] as const),
    queryFn: () => {
      if (!request) throw new Error("drill request is required");
      return MetricTreeService.drill(projectId, request);
    },
    enabled: enabled && !!request,
    staleTime: 5 * 60 * 1000,
    retry: false
  });
}
