import { type NodeHandle, Position, type Edge as RFEdge, type Node as RFNode } from "@xyflow/react";
import type {
  WmBreakdownEdge,
  WmBreakdownNode,
  WmComputedMeasure,
  WmMeasureBreakdown,
  WmSelection,
  WorldModel,
  WorldModelEntity
} from "@/types/worldModel";
import { NODE_HEIGHT_COLLAPSED, NODE_WIDTH, type NodeSize } from "../components/semanticGraph";

// Card geometry and the ELK runner are shared with the Metric Tree — see
// `pages/ide/components/semanticGraph`. Re-exported here so this module stays
// the one import site for everything World Model's own layout code needs.
export {
  layoutWithElk,
  NODE_HEIGHT_COLLAPSED,
  NODE_WIDTH,
  type NodeSize,
  type WaypointMap
} from "../components/semanticGraph";

/** Width of the in-place expanded entity node (measure-tree mode). */
export const EXPANDED_NODE_WIDTH = 360;

export const OP_GLYPH: Record<WmBreakdownEdge["operator"], string> = {
  add: "+",
  sub: "−",
  mul: "×",
  div: "÷"
};

/** Static handle bounds mirroring WorldModelEntityNode's four <Handle>s.
 *
 * React Flow refuses to mount an edge until BOTH endpoint nodes report handle
 * bounds — `getEdgePosition` returns null otherwise and the edge is silently
 * dropped. Normally bounds arrive only after the DOM handles are measured by a
 * ResizeObserver, which races against edge rendering: on some loads a node
 * hadn't been measured yet, its edges resolved to null, and never re-rendered
 * ("sometimes edges are missing"). Because our custom edge draws an absolute
 * ELK waypoint path, it can't self-heal from geometry once dropped.
 *
 * Declaring `handles` on the node object makes `parseHandles` populate
 * `internals.handleBounds` at adopt time — before any measurement — so every
 * node is edge-ready from the first frame. The x/y here only feed the
 * straight-line fallback (waypoints override it); source-before-target order
 * matches sourcePosition=Bottom / targetPosition=Top. */
export const NODE_HANDLES: NodeHandle[] = [
  {
    id: "bottom-out",
    type: "source",
    position: Position.Bottom,
    x: NODE_WIDTH / 2,
    y: NODE_HEIGHT_COLLAPSED
  },
  { id: "top-in", type: "target", position: Position.Top, x: NODE_WIDTH / 2, y: 0 },
  { id: "top-out", type: "source", position: Position.Top, x: NODE_WIDTH / 2, y: 0 },
  {
    id: "bottom-in",
    type: "target",
    position: Position.Bottom,
    x: NODE_WIDTH / 2,
    y: NODE_HEIGHT_COLLAPSED
  }
];

/** Map a measure_type string to a display symbol. */
export function measureSymbol(type: string): string {
  switch (type) {
    case "sum":
      return "Σ";
    case "count":
    case "count_distinct":
    case "count_distinct_approx":
      return "#";
    case "average":
    case "median":
      return "⌀";
    case "min":
      return "↓";
    case "max":
      return "↑";
    default:
      return "ƒ";
  }
}

/** Map a measure_type string to a Tailwind text-color class. */
export function measureSymbolColor(_type: string): string {
  return "text-[color:var(--vis-purple)]";
}

/** Derive which entity IDs and edge keys are "active" given the current selection. */
function deriveActive(
  model: WorldModel,
  selection: WmSelection
): { activeEntityIds: Set<string> | null; activeEdgeKeys: Set<string> } {
  if (!selection) return { activeEntityIds: null, activeEdgeKeys: new Set() };

  const activeEntityIds = new Set<string>();
  const activeEdgeKeys = new Set<string>();

  if (
    selection.kind === "entity" ||
    selection.kind === "dimension" ||
    selection.kind === "measure" ||
    selection.kind === "instance"
  ) {
    const id = selection.entityId;
    activeEntityIds.add(id);

    if (selection.kind === "instance") {
      // BFS: traverse the full connected subgraph so the entire hierarchy
      // chain is active, not just immediate neighbors of the seed entity.
      const queue = [id];
      while (queue.length > 0) {
        const current = queue.shift()!;
        for (const e of model.edges) {
          if (e.from === current || e.to === current) {
            activeEdgeKeys.add(`${e.from}->${e.to}`);
            const neighbor = e.from === current ? e.to : e.from;
            if (!activeEntityIds.has(neighbor)) {
              activeEntityIds.add(neighbor);
              queue.push(neighbor);
            }
          }
        }
      }
    } else {
      for (const e of model.edges) {
        if (e.from === id || e.to === id) {
          activeEdgeKeys.add(`${e.from}->${e.to}`);
          activeEntityIds.add(e.from);
          activeEntityIds.add(e.to);
        }
      }
    }
  } else if (selection.kind === "promotion") {
    activeEntityIds.add(selection.from);
    activeEntityIds.add(selection.to);
    activeEdgeKeys.add(`${selection.from}->${selection.to}`);
  }

  return { activeEntityIds, activeEdgeKeys };
}

/** Convert a WorldModel to unpositioned React Flow nodes and edges.
 *  filterCounts is intentionally excluded — apply it separately after layout
 *  so count updates don't trigger expensive ELK re-layout. */
export function worldModelToFlow(
  model: WorldModel,
  selection: WmSelection
): { nodes: RFNode[]; edges: RFEdge[] } {
  const { activeEntityIds, activeEdgeKeys } = deriveActive(model, selection);
  const hasSelection = activeEntityIds !== null;

  const primaryId =
    selection?.kind === "entity"
      ? selection.entityId
      : selection?.kind === "dimension" || selection?.kind === "measure"
        ? selection.entityId
        : selection?.kind === "instance"
          ? selection.entityId
          : null;

  const nodes: RFNode[] = model.entities.map((entity) => {
    const isActive = !hasSelection || (activeEntityIds?.has(entity.id) ?? true);
    return {
      id: entity.id,
      type: "wm-entity",
      position: { x: 0, y: 0 },
      sourcePosition: Position.Bottom,
      targetPosition: Position.Top,
      // Pre-declared so edges are renderable before handle measurement — see NODE_HANDLES.
      handles: NODE_HANDLES,
      data: {
        entity,
        selected: entity.id === primaryId,
        dimmed: !isActive,
        filterCount: null
      },
      width: NODE_WIDTH,
      height: NODE_HEIGHT_COLLAPSED,
      // Initial dimensions so React Flow renders the node immediately instead of
      // holding it `visibility: hidden` until the ResizeObserver measures it.
      // Auto-height nodes (sample cards set `height: undefined` downstream) would
      // otherwise stay invisible forever when a "ResizeObserver loop … undelivered
      // notifications" drops their measurement. `nodeHasDimensions` accepts
      // `initialWidth`/`initialHeight`, so the node shows at once and the observer
      // just refines it. See WorldModelGraph.displayNodes.
      initialWidth: NODE_WIDTH,
      initialHeight: NODE_HEIGHT_COLLAPSED
    };
  });

  const edges: RFEdge[] = model.edges.map((edge, i) => {
    const key = `${edge.from}->${edge.to}`;
    const isActive = !hasSelection || activeEdgeKeys.has(key);
    const isHighlighted =
      selection?.kind === "promotion" && selection.from === edge.from && selection.to === edge.to;
    const isFanout = !edge.functional;

    const stroke = isFanout
      ? "var(--destructive)"
      : isHighlighted || isActive
        ? "var(--info)"
        : "var(--info)";

    return {
      id: `${key}#${i}`,
      source: edge.from,
      target: edge.to,
      type: "smoothstep",
      ...(isHighlighted
        ? {
            label: isFanout ? "⇉" : "Σp",
            labelStyle: {
              fontFamily: "ui-monospace, monospace",
              fontSize: 11,
              fill: isFanout ? "var(--destructive)" : "var(--info)",
              fontWeight: 500
            },
            labelBgStyle: { fill: "var(--card)" },
            labelBgPadding: [3, 5] as [number, number],
            labelBgBorderRadius: 2
          }
        : {}),
      // Flow the direction on edges connected to the current selection — both
      // when an edge is clicked (promotion) and when a node is clicked.
      animated: isHighlighted || (hasSelection && isActive),
      style: {
        stroke,
        strokeWidth: isHighlighted ? 2 : 1,
        strokeDasharray: isFanout ? "4 3" : undefined,
        opacity: isHighlighted ? 0.95 : isActive ? 0.6 : 0.08,
        transition: "opacity 0.2s, stroke 0.2s"
      }
    };
  });

  return { nodes, edges };
}
/** A breakdown node's value, reshaped to the same contract entity cards
 *  already render (`WmComputedMeasure`) so a contributor can be shown via the
 *  entity's normal measure-chip UI instead of a bespoke rendering path. */
export function breakdownNodeToComputedMeasure(node: WmBreakdownNode): WmComputedMeasure {
  return {
    name: node.measure,
    measure_type: node.measure_type,
    value: node.value,
    fiber_count: 0,
    label: node.label
  };
}

/** A measure's `view` (the semantic model view name) is not the same as the
 *  graph node's `id` — an entity can represent that view at a particular
 *  grain. Map each view to the node id(s) that actually render it, usually
 *  a single id, so breakdown data (keyed by view) can be attached to the
 *  right card(s). */
export function buildViewToEntityIds(entities: WorldModelEntity[]): Map<string, string[]> {
  const map = new Map<string, string[]>();
  for (const entity of entities) {
    const existing = map.get(entity.view);
    if (existing) existing.push(entity.id);
    else map.set(entity.view, [entity.id]);
  }
  return map;
}

/** Group a measure breakdown's non-root nodes by the graph node (entity) that
 *  actually owns each contributor measure. A contributor is shown on its own
 *  entity's card — including the expanded entity itself, when a component
 *  happens to live on the same view as the measure being broken down. */
export function groupBreakdownContributorsByEntity(
  breakdown: WmMeasureBreakdown | null,
  viewToEntityIds: Map<string, string[]>
): Map<string, WmComputedMeasure[]> {
  const byEntity = new Map<string, WmComputedMeasure[]>();
  if (!breakdown) return byEntity;
  for (const node of breakdown.nodes) {
    if (node.id === breakdown.root) continue;
    const entityIds = viewToEntityIds.get(node.view);
    if (!entityIds) continue;
    const measure = breakdownNodeToComputedMeasure(node);
    for (const entityId of entityIds) {
      const existing = byEntity.get(entityId);
      if (existing) existing.push(measure);
      else byEntity.set(entityId, [measure]);
    }
  }
  return byEntity;
}

/** React Flow handle id for a contributor measure's row on its own entity card
 *  (edge SOURCE) and for a composite measure's row on the expanded card (edge
 *  TARGET). Kept as helpers so the node components and the edge builder agree. */
export const contributorHandleId = (measure: string) => `bkd-src-${measure}`;
export const composedHandleId = (measure: string) => `bkd-tgt-${measure}`;

/** Handle ids for a SAME-CARD composition edge — one composite row on the
 *  expanded card feeds another row on the same card (e.g. net_revenue =
 *  total_order_value − total_shipping_costs, all three rows on the Order card).
 *  These live on the RIGHT gutter so they never collide with the left-side
 *  cross-entity contributor edges (`composedHandleId`). Every expanded row
 *  carries both, since a mid-tree composite is a source for its parent and a
 *  target for its own components. */
export const composedSelfSourceHandleId = (measure: string) => `bkd-self-src-${measure}`;
export const composedSelfTargetHandleId = (measure: string) => `bkd-self-tgt-${measure}`;

/** Pre-declared per-measure breakdown handles, mirroring the conditional
 *  <Handle>s that MetricChipsSection (contributor rows, source, right edge) and
 *  FlatMeasureList (composite rows, target, left edge) render only while a
 *  breakdown is on screen.
 *
 *  These carry the SAME race as the static NODE_HANDLES (see the note above):
 *  a breakdown edge points at `bkd-src-<measure>` / `bkd-tgt-<measure>`, and
 *  React Flow drops the edge until BOTH endpoint handles report bounds. The
 *  handle only exists in the DOM after the card re-renders into contributor /
 *  expanded mode, so its bounds arrive a frame late and the edge intermittently
 *  fails to render ("sometimes the contributor→metric edge is missing").
 *  Seeding bounds at adopt time makes the edge renderable on its first frame;
 *  the ResizeObserver still refines the exact x/y once measured. */
export function contributorSourceHandles(measures: WmComputedMeasure[]): NodeHandle[] {
  return measures.map((m, i) => ({
    id: contributorHandleId(m.name),
    type: "source",
    position: Position.Right,
    x: NODE_WIDTH,
    // Rough per-row offset below the card header; refined on measurement.
    y: NODE_HEIGHT_COLLAPSED + i * 12
  }));
}

export function composedTargetHandles(measures: WmComputedMeasure[]): NodeHandle[] {
  return measures.map((m, i) => ({
    id: composedHandleId(m.name),
    type: "target",
    position: Position.Left,
    x: 0,
    y: NODE_HEIGHT_COLLAPSED + i * 40
  }));
}

/** Right-gutter source + target handles for the same-card composition edges.
 *  Pre-declared for the same reason as `composedTargetHandles` — the DOM
 *  handles are rendered conditionally, so seeding bounds keeps the self-edge
 *  from racing the ResizeObserver. */
export function composedSelfHandles(measures: WmComputedMeasure[]): NodeHandle[] {
  return measures.flatMap((m, i) => {
    const y = NODE_HEIGHT_COLLAPSED + i * 40;
    return [
      {
        id: composedSelfSourceHandleId(m.name),
        type: "source" as const,
        position: Position.Right,
        x: EXPANDED_NODE_WIDTH,
        y
      },
      {
        id: composedSelfTargetHandleId(m.name),
        type: "target" as const,
        position: Position.Right,
        x: EXPANDED_NODE_WIDTH,
        y
      }
    ];
  });
}

/** Shared dashed-purple styling for every breakdown edge (cross-entity or
 *  same-card), so the two kinds read as one decomposition. */
function breakdownEdge(
  parts: Pick<RFEdge, "id" | "source" | "sourceHandle" | "target" | "targetHandle"> & {
    label: string;
  }
): RFEdge {
  return {
    ...parts,
    type: "wm-edge",
    labelStyle: {
      fontFamily: "ui-monospace, monospace",
      fontSize: 11,
      fill: "var(--vis-purple)",
      fontWeight: 500
    },
    labelBgStyle: { fill: "var(--card)" },
    labelBgPadding: [3, 5] as [number, number],
    labelBgBorderRadius: 2,
    zIndex: 15,
    data: { isBreakdownEdge: true },
    style: {
      stroke: "var(--vis-purple)",
      strokeWidth: 1.5,
      strokeDasharray: "3 3",
      opacity: 0.9
    }
  };
}

/** Edges for a measure breakdown, anchored to per-measure handles (rather than
 *  the node as a whole) so each edge points at the actual contributor number,
 *  mirroring the driver tree. Two kinds:
 *
 *  - **cross-entity**: a contributor measure on another entity card → the row
 *    of the composite it rolls into on the expanded card (left gutter).
 *  - **same-card**: a composite that lives on the expanded card itself → the
 *    row of the composite it feeds, also on the expanded card (right gutter).
 *    (e.g. total_order_value − total_shipping_costs → net_revenue). These used
 *    to be omitted, leaving the in-card composition with no visible edge.
 *
 *  Both endpoints are real, already-positioned graph nodes, so no extra layout
 *  pass is needed. */
export function buildBreakdownEdges(
  anchorId: string,
  breakdown: WmMeasureBreakdown | null,
  viewToEntityIds: Map<string, string[]>
): RFEdge[] {
  if (!breakdown) return [];

  const nodeById = new Map(breakdown.nodes.map((n) => [n.id, n]));
  // The component edge (from → to) tells us which composite each contributor
  // rolls into, and with which operator.
  const parentEdgeOf = new Map<string, WmBreakdownEdge>();
  for (const edge of breakdown.edges) parentEdgeOf.set(edge.from, edge);

  const edges: RFEdge[] = [];
  for (const node of breakdown.nodes) {
    if (node.id === breakdown.root) continue;
    const entityIds = viewToEntityIds.get(node.view);
    if (!entityIds) continue;

    // Resolve the composite this contributor feeds; anchor the edge's target to
    // that composite's row when it lives on the expanded card, else the card.
    const parentEdge = parentEdgeOf.get(node.id);
    const parentNode = parentEdge ? nodeById.get(parentEdge.to) : undefined;
    const parentOnAnchor =
      !!parentNode && (viewToEntityIds.get(parentNode.view) ?? []).includes(anchorId);
    const targetHandle =
      parentOnAnchor && parentNode ? composedHandleId(parentNode.measure) : undefined;
    const label = parentEdge ? OP_GLYPH[parentEdge.operator] : "Σ";

    // Same-card: this component and the composite it feeds are both rows on the
    // expanded card — draw a right-gutter self-edge between the two rows.
    if (entityIds.includes(anchorId) && parentOnAnchor && parentNode) {
      edges.push(
        breakdownEdge({
          id: `bkd-edge-self-${anchorId}-${node.measure}`,
          source: anchorId,
          sourceHandle: composedSelfSourceHandleId(node.measure),
          target: anchorId,
          targetHandle: composedSelfTargetHandleId(parentNode.measure),
          label
        })
      );
    }

    // Cross-entity: from every OTHER card hosting this contributor.
    for (const entityId of entityIds) {
      if (entityId === anchorId) continue;
      edges.push(
        breakdownEdge({
          id: `bkd-edge-${anchorId}-${entityId}-${node.measure}`,
          source: entityId,
          sourceHandle: contributorHandleId(node.measure),
          target: anchorId,
          targetHandle,
          label
        })
      );
    }
  }
  return edges;
}

/* ----------------------------------------------------------------------------
 * Layout-time card size estimation.
 *
 * ELK positions nodes at their box size; a card that GROWS on selection /
 * expansion (measure chips, sample chips, or the wide breakdown card) needs its
 * real size reserved, or neighbors laid out at the collapsed size get
 * overlapped. Rather than round-trip through a DOM measurement (a two-step
 * jump), we estimate each card's height from the same data the node components
 * render. Estimates mirror WorldModelEntityNode / WorldModelExpandedEntityNode
 * markup; a few px of slack is harmless — ELK's inter-node spacing absorbs it.
 * Keep these in sync when the card layouts change.
 * -------------------------------------------------------------------------- */

/** Card rows 1–2 (name + grain) plus paddings/gaps, excluding the row-3 body. */
const CARD_HEADER_H = 66;
/** One metric-chip row in MetricChipsSection. */
const CHIP_ROW_H = 14;
/** border-t + pt-1 above the metric chips. */
const CHIPS_HEADER_H = 6;
/** The "filter n/total" badge. */
const FILTER_BADGE_H = 22;
/** One descendant sample-chip button (incl. gap). */
const SAMPLE_ROW_H = 22;
/** The "+N more" browse button (incl. gap). */
const MORE_BTN_H = 22;
/** The "filtering…" loading row. */
const LOADING_ROW_H = 20;
/** Expanded breakdown card header (label + grain lines + paddings). */
const EXPANDED_HEADER_H = 46;
/** One measure row in the expanded card's FlatMeasureList. */
const EXPANDED_ROW_H = 42;
/** Expanded card body before the breakdown loads (placeholder text). */
const EXPANDED_PLACEHOLDER_H = 34;

interface FilterCountEntry {
  matched: number;
  total: number;
  sample?: string[];
  sample_keys?: string[];
}

/** Everything needed to estimate a node's laid-out size in the current state. */
export interface WmSizingState {
  /** The in-place expanded (breakdown) entity, if any. */
  expandedEntityId: string | null;
  /** Row count on the expanded card (root + same-card contributors), or null
   *  while the breakdown is still loading (placeholder height). */
  expandedRowCount: number | null;
  /** The filter-seed entity whose own measure chips are shown. */
  filterSeedEntityId: string | null;
  /** The filter seed's computed measure chips. */
  seedComputedMeasures: WmComputedMeasure[] | null;
  /** Breakdown contributor chips shown on their own entity cards. */
  contributorsByEntity: Map<string, WmComputedMeasure[]>;
  /** Streamed per-entity filter counts + descendant samples. */
  filterCounts: Record<string, FilterCountEntry> | null;
  /** Whether filter counts are still streaming (loading row on empty cards). */
  isCountLoading: boolean;
}

/** Estimate the box size ELK should reserve for one entity, mirroring the
 *  precedence WorldModelGraph uses to pick what a card renders. */
export function layoutSizeForEntity(id: string, s: WmSizingState): NodeSize {
  if (id === s.expandedEntityId) {
    const body =
      s.expandedRowCount === null
        ? EXPANDED_PLACEHOLDER_H
        : Math.max(1, s.expandedRowCount) * EXPANDED_ROW_H;
    return { width: EXPANDED_NODE_WIDTH, height: EXPANDED_HEADER_H + body };
  }

  // A contributor card takes priority over the seed's own chips (mid-drilldown).
  const contributor = s.contributorsByEntity.get(id) ?? null;
  const chips = contributor ?? (id === s.filterSeedEntityId ? s.seedComputedMeasures : null);
  if (chips) {
    return {
      width: NODE_WIDTH,
      height: CARD_HEADER_H + CHIPS_HEADER_H + Math.max(1, chips.length) * CHIP_ROW_H
    };
  }

  const fc = s.filterCounts?.[id];
  if (fc) {
    let height = CARD_HEADER_H + FILTER_BADGE_H;
    const samples = fc.sample?.length ?? 0;
    if (samples > 0) {
      height += samples * SAMPLE_ROW_H;
      if (fc.matched > samples) height += MORE_BTN_H;
    }
    return { width: NODE_WIDTH, height };
  }

  if (s.isCountLoading) {
    return { width: NODE_WIDTH, height: CARD_HEADER_H + LOADING_ROW_H };
  }

  return { width: NODE_WIDTH, height: NODE_HEIGHT_COLLAPSED };
}

/** Build the per-entity layout size map for the current state. */
export function buildLayoutSizeMap(entityIds: string[], s: WmSizingState): Map<string, NodeSize> {
  return new Map(entityIds.map((id) => [id, layoutSizeForEntity(id, s)]));
}
