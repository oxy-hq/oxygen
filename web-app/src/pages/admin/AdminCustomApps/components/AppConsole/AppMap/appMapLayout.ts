import { type AppMap, type MapEdge, TARGET_GROUP_LABEL, type TargetGroup } from "./appMapModel";

/**
 * Where each node of an {@link AppMap} sits, in pixels, for a container `width` wide:
 * three columns, rows of one height, edges as horizontal-tangent curves between a
 * node's right edge and the next node's left. Pure, so the geometry is tested
 * without a browser — jsdom lays nothing out.
 *
 * Rows rather than a graph layout: entry points sit in their function's band, so a
 * trigger edge is short and never crosses another; only the right-hand side is
 * shared, and the model already orders it beside the functions that reach it.
 */

/** Below this the three columns cannot hold a label each; the panel lists instead. */
export const MIN_DIAGRAM_WIDTH = 560;

export const NODE_HEIGHT = 22;
/** A node and the gap under it. */
export const ROW = 28;
/** The column headings' row. */
const HEADING = 22;
/** A target group's heading. */
const GROUP_HEADING = 20;
/** Breathing room between two functions' bands. */
const BAND_GAP = 6;

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface GroupHeading {
  group: TargetGroup;
  label: string;
  x: number;
  y: number;
}

export interface EdgeLabel {
  /** The target it sits beside: one label per target, where its edges converge. */
  target: string;
  text: string;
  failed: boolean;
  /** Right end of the text (it is right-aligned against the target). */
  x: number;
  y: number;
}

export interface Columns {
  entry: { x: number; width: number };
  fn: { x: number; width: number };
  target: { x: number; width: number };
}

export interface AppMapLayout {
  width: number;
  height: number;
  columns: Columns;
  /** By node id. */
  boxes: Map<string, Box>;
  headings: GroupHeading[];
  /** By edge id. */
  paths: Map<string, string>;
  labels: EdgeLabel[];
}

export function columnsFor(width: number): Columns {
  const entry = Math.round(width * 0.22);
  const fn = Math.round(width * 0.26);
  const target = Math.round(width * 0.29);
  // The gap before the targets is the wider one: their edge labels live in it.
  const firstGap = Math.round(width * 0.05);
  return {
    entry: { x: 0, width: entry },
    fn: { x: entry + firstGap, width: fn },
    target: { x: width - target, width: target }
  };
}

/** A curve leaving `from` and arriving at `to` horizontally. */
export function curve(from: { x: number; y: number }, to: { x: number; y: number }): string {
  const bend = (to.x - from.x) / 2;
  return `M ${from.x} ${from.y} C ${from.x + bend} ${from.y}, ${to.x - bend} ${to.y}, ${to.x} ${to.y}`;
}

const middle = (box: Box) => box.y + box.height / 2;

/** Entry points stacked in their function's band; the function centred in it. */
function placeLeft(map: AppMap, columns: Columns, boxes: Map<string, Box>): number {
  let y = HEADING;
  for (const fn of map.functions) {
    const entries = map.entries.filter((e) => e.fn === fn.name);
    const rows = Math.max(1, entries.length);
    entries.forEach((entry, i) => {
      boxes.set(entry.id, { ...columns.entry, y: y + i * ROW, height: NODE_HEIGHT });
    });
    boxes.set(fn.id, {
      ...columns.fn,
      y: y + ((rows - 1) * ROW) / 2,
      height: NODE_HEIGHT
    });
    y += rows * ROW + BAND_GAP;
  }
  return y;
}

/** Targets in the model's order, under a heading per group. */
function placeRight(
  map: AppMap,
  columns: Columns,
  boxes: Map<string, Box>,
  headings: GroupHeading[]
): number {
  let y = HEADING;
  let group: TargetGroup | null = null;
  for (const target of map.targets) {
    if (target.group !== group) {
      group = target.group;
      headings.push({ group, label: TARGET_GROUP_LABEL[group], x: columns.target.x, y });
      y += GROUP_HEADING;
    }
    boxes.set(target.id, { ...columns.target, y, height: NODE_HEIGHT });
    y += ROW;
  }
  return y;
}

/** What one target's converging edges are labelled: their families, or
 *  `declared` for a manifest destination no recorded run has written to. */
function labelText(edges: MapEdge[]): string {
  const families: string[] = [];
  for (const edge of edges) {
    for (const family of edge.families) if (!families.includes(family)) families.push(family);
  }
  return families.length > 0 ? families.join(", ") : "declared";
}

function edgeLabels(map: AppMap, boxes: Map<string, Box>): EdgeLabel[] {
  const into = new Map<string, MapEdge[]>();
  for (const edge of map.edges) {
    if (edge.kind === "touch") into.set(edge.to, [...(into.get(edge.to) ?? []), edge]);
  }
  const labels: EdgeLabel[] = [];
  for (const [target, edges] of into) {
    const box = boxes.get(target);
    if (!box) continue;
    labels.push({
      target,
      text: labelText(edges),
      failed: edges.some((e) => e.failedCalls > 0),
      x: box.x - 6,
      // Just above where the curves arrive, which they do level.
      y: middle(box) - 4
    });
  }
  return labels;
}

export function layoutAppMap(map: AppMap, width: number): AppMapLayout {
  const columns = columnsFor(width);
  const boxes = new Map<string, Box>();
  const headings: GroupHeading[] = [];
  const left = placeLeft(map, columns, boxes);
  const right = placeRight(map, columns, boxes, headings);

  const paths = new Map<string, string>();
  for (const edge of map.edges) {
    const from = boxes.get(edge.from);
    const to = boxes.get(edge.to);
    if (!from || !to) continue;
    paths.set(
      edge.id,
      curve({ x: from.x + from.width, y: middle(from) }, { x: to.x, y: middle(to) })
    );
  }
  return {
    width,
    height: Math.max(left, right),
    columns,
    boxes,
    headings,
    paths,
    labels: edgeLabels(map, boxes)
  };
}
