import type { WorldModel } from "@/types/worldModel";

/** A node of the home's small world model map, placed in percent of the drawing. */
export interface PreviewNode {
  id: string;
  label: string;
  /** Percent of the drawing's width / height. */
  x: number;
  y: number;
}

export interface PreviewEdge {
  key: string;
  from: PreviewNode;
  to: PreviewNode;
}

export interface PreviewLayout {
  nodes: PreviewNode[];
  edges: PreviewEdge[];
  /** Entities left off the drawing for room. The full page shows them all. */
  hidden: number;
}

export interface PreviewOptions {
  maxColumns?: number;
  maxPerColumn?: number;
}

/** The counts the card states under the drawing. */
export function worldModelCounts(model: WorldModel) {
  const entities = [...new Map(model.entities.map((e) => [e.id, e])).values()];
  return {
    entities: entities.length,
    connections: new Set(model.edges.map((e) => `${e.from}->${e.to}`)).size,
    measures: entities.reduce((n, e) => n + e.own_measures.length, 0)
  };
}

/**
 * Where each entity sits on the home's small map: one column per depth (the
 * server's `depth`, how far an entity is from the roots of the model), the most
 * connected entities first within a column, and at most `maxPerColumn` in each.
 * A model whose entities share one depth is spread across the columns instead, so
 * a flat model still reads as a map rather than a list.
 *
 * Pure, so what the card draws is testable without rendering an SVG.
 */
export function previewLayout(
  model: WorldModel,
  { maxColumns = 4, maxPerColumn = 4 }: PreviewOptions = {}
): PreviewLayout {
  const degree = new Map<string, number>();
  for (const e of model.edges) {
    degree.set(e.from, (degree.get(e.from) ?? 0) + 1);
    degree.set(e.to, (degree.get(e.to) ?? 0) + 1);
  }
  // One node per entity id: a model can list the same entity twice.
  const unique = [...new Map(model.entities.map((e) => [e.id, e])).values()];
  const byDegree = [...unique].sort(
    (a, b) => (degree.get(b.id) ?? 0) - (degree.get(a.id) ?? 0) || a.label.localeCompare(b.label)
  );

  const depths = [...new Set(unique.map((e) => e.depth))].sort((a, b) => a - b);
  const columns: (typeof unique)[] = [];
  if (depths.length <= 1) {
    const count = Math.min(maxColumns, Math.max(1, Math.ceil(byDegree.length / maxPerColumn)));
    for (let i = 0; i < count; i++) columns.push([]);
    byDegree.forEach((entity, i) => {
      columns[i % count].push(entity);
    });
  } else {
    const count = Math.min(maxColumns, depths.length);
    for (let i = 0; i < count; i++) columns.push([]);
    for (const entity of byDegree) {
      const column = Math.min(depths.indexOf(entity.depth), count - 1);
      columns[column].push(entity);
    }
  }

  const nodes: PreviewNode[] = [];
  let hidden = 0;
  columns.forEach((column, c) => {
    const shown = column.slice(0, maxPerColumn);
    hidden += column.length - shown.length;
    // Columns spread over the width, leaving room on the right for the last
    // column's labels; rows centred vertically.
    // The last column keeps about a quarter of the width for its labels; with only
    // two columns that is half, since each column's labels are wider.
    const span = columns.length <= 2 ? 50 : 72;
    const x = columns.length === 1 ? 8 : 4 + (c * span) / (columns.length - 1);
    shown.forEach((entity, r) => {
      const y = ((r + 1) * 100) / (shown.length + 1);
      nodes.push({ id: entity.id, label: entity.label, x, y });
    });
  });

  const placed = new Map(nodes.map((n) => [n.id, n]));
  const edges = new Map<string, PreviewEdge>();
  for (const e of model.edges) {
    const from = placed.get(e.from);
    const to = placed.get(e.to);
    const key = `${e.from}->${e.to}`;
    if (from && to && from !== to) edges.set(key, { key, from, to });
  }
  return { nodes, edges: [...edges.values()], hidden };
}
