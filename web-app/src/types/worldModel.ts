// Types mirroring the /semantic/world-model response.

type AdditivityClass = "additive" | "non_additive" | "passthrough";

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

export interface WorldModelInducedMeasure extends WorldModelMeasure {
  promoted_from: string;
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

export interface WorldModelEdge {
  from: string;
  to: string;
  functional: boolean;
}

export interface WorldModel {
  entities: WorldModelEntity[];
  edges: WorldModelEdge[];
}

// ── Selection state ───────────────────────────────────────────────────────────

export type WmSelection =
  | { kind: "entity"; entityId: string }
  | { kind: "promotion"; from: string; to: string }
  | { kind: "dimension"; entityId: string; dimensionName: string }
  | {
      kind: "measure";
      entityId: string;
      measureName: string;
      induced: boolean;
      promotedFrom?: string;
    }
  | { kind: "instance"; entityId: string; keyValue: string; label: string }
  | null;

// ── Filter / Instance types ───────────────────────────────────────────────────

export interface WmFilterSeed {
  entityId: string;
  keyValue: string;
  label: string;
}

export interface WmInstance {
  key: string;
  display: string;
}

export interface WmInstancesResponse {
  total: number;
  has_more: boolean;
  items: WmInstance[];
}

export interface WmEntityCount {
  matched: number;
  total: number;
  /** Sample of reachable descendant rows at this grain (display strings). */
  sample?: string[];
  /** Navigation keys aligned with `sample`. */
  sample_keys?: string[];
}

export interface WmFilterCountEvent {
  entity_name: string;
  total?: number;
  matched?: number;
  sample?: string[];
  sample_keys?: string[];
  done: boolean;
}

export interface WmMeasureName {
  name: string;
  measure_type: string;
  label?: string | null;
}

export type WmInstanceDetailEvent =
  | {
      kind: "init";
      entity_id: string;
      key_value: string;
      display: string;
      attributes: WmAttrValue[];
    }
  | { kind: "parent"; promotes_to: WmParentRef[] }
  | { kind: "child"; child: WmChildSample }
  | { kind: "measure_names"; measure_names: WmMeasureName[] }
  | { kind: "measure"; computed_measures: WmComputedMeasure[] }
  | { kind: "done" };

interface WmAttrValue {
  name: string;
  value: string;
  label?: string | null;
}

interface WmParentRef {
  promotion: string;
  key: string;
  display: string;
}

interface WmChildSample {
  promotion: string;
  fiber_count: number;
  sample: string[];
  /** Canonical navigation key per sample row — plain value for single-PK entities,
   *  JSON array string (e.g. `["70978","177411"]`) for composite-PK entities. */
  sample_keys: string[];
}

export interface WmComputedMeasure {
  name: string;
  measure_type: string;
  /** null while the measure query is in-flight (skeleton state). */
  value: string | null;
  fiber_count: number;
  label?: string | null;
}

export interface WmInstanceDetail {
  entity_id: string;
  key_value: string;
  display: string;
  attributes: WmAttrValue[];
  promotes_to: WmParentRef[];
  receives_from: WmChildSample[];
  computed_measures: WmComputedMeasure[];
}

// ── Instance measure breakdown (driver tree) ───────────────────────────────────

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
  /** Filled by `value` events; null while pending. */
  value: string | null;
  unvalued_reason: string | null;
}

export interface WmBreakdownEdge {
  from: string;
  to: string;
  operator: "add" | "sub" | "mul" | "div";
  sign: number;
}

export type WmMeasureBreakdownEvent =
  | {
      kind: "init";
      root: string;
      nodes: Omit<WmBreakdownNode, "value" | "unvalued_reason">[];
      edges: WmBreakdownEdge[];
    }
  | { kind: "value"; node_id: string; value: string | null; unvalued_reason: string | null }
  | { kind: "done" };

export interface WmMeasureBreakdown {
  root: string;
  nodes: WmBreakdownNode[];
  edges: WmBreakdownEdge[];
}
