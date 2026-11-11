// World-model types. Mirror the `/<project_id>/semantic/world-model*`
// wire shapes the world-model graph endpoints emit (snake_case) — the
// same graph the IDE's World Model surface renders. Kept in sync with the
// server's own structs by hand; field
// names match the JSON verbatim.
//
// UI-only selection/state types from the web app are deliberately not
// ported — a bundle drives its own interaction model over this data.

// ── Graph ──────────────────────────────────────────────────────────────────────

/** How a measure aggregates across the entity hierarchy. */
export type AdditivityClass = "additive" | "non_additive" | "passthrough";

export interface WorldModelMeasure {
  name: string;
  measure_type: string;
  additivity: AdditivityClass;
  description?: string | null;
  expr?: string | null;
  label?: string | null;
  /** True when the measure decomposes into a metric-tree driver breakdown. */
  has_breakdown?: boolean;
}

/** A measure promoted onto this entity from a descendant view. */
export interface WorldModelInducedMeasure extends WorldModelMeasure {
  /** View the measure is actually declared on. */
  promoted_from: string;
  /** Promotion path from the declaring view up to this entity. */
  path: string[];
}

export interface WorldModelDimension {
  name: string;
  dim_type: string;
  description?: string | null;
  label?: string | null;
}

export interface WorldModelEntity {
  id: string;
  label: string;
  view: string;
  description?: string | null;
  depth: number;
  dimensions: WorldModelDimension[];
  own_measures: WorldModelMeasure[];
  induced_measures: WorldModelInducedMeasure[];
  display_field?: string | null;
}

/** A promotion edge: measures on `from` promote up to `to`. */
export interface WorldModelEdge {
  from: string;
  to: string;
  functional: boolean;
}

export interface WorldModel {
  entities: WorldModelEntity[];
  edges: WorldModelEdge[];
}

// ── Instances ───────────────────────────────────────────────────────────────

export interface WmInstance {
  key: string;
  display: string;
  /**
   * The org location this instance is, when the entity is bound to the
   * locations registry and the key is mapped; absent otherwise.
   */
  location?: { id: string; name: string; kind?: string | null; parent_id?: string | null };
}

export interface WmInstancesResponse {
  total: number;
  has_more: boolean;
  items: WmInstance[];
}

// ── Filter counts ─────────────────────────────────────────────────────────────

export interface WmEntityCount {
  matched: number;
  total: number;
  /** Sample of reachable descendant rows at this grain (display strings). */
  sample?: string[];
  /** Navigation keys aligned with `sample`. */
  sample_keys?: string[];
}

export interface WmFilterCountsResponse {
  counts: Record<string, WmEntityCount>;
}

// ── Measure breakdown / driver tree (SSE) ───────────────────────────────────

export interface WmBreakdownNode {
  /** Metric node id `view.measure`. */
  id: string;
  view: string;
  measure: string;
  label: string;
  measure_type: string;
  is_composite: boolean;
  is_root: boolean;
  expr?: string | null;
  /** Filled by `value` frames; null while pending. */
  value: string | null;
  unvalued_reason: string | null;
}

export interface WmBreakdownEdge {
  from: string;
  to: string;
  operator: "add" | "sub" | "mul" | "div";
  sign: number;
}

/** One frame of the measure-breakdown stream. The `init` frame carries the
 *  graph shape; `value` frames fill node values in as they resolve. */
export type WmMeasureBreakdownEvent =
  | {
      kind: "init";
      root: string;
      nodes: Omit<WmBreakdownNode, "value" | "unvalued_reason">[];
      edges: WmBreakdownEdge[];
    }
  | { kind: "value"; node_id: string; value: string | null; unvalued_reason: string | null }
  | { kind: "done" };

/** Accumulated breakdown — the shape `useMeasureBreakdown` folds the
 *  stream into. */
export interface WmMeasureBreakdown {
  root: string;
  nodes: WmBreakdownNode[];
  edges: WmBreakdownEdge[];
}
