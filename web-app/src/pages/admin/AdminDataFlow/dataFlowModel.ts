/**
 * The Data flow page's reading of `GET /customer-apps/data-flow`: which column a
 * node sits in, which way each link is drawn and laid out, and the words and
 * styles a node or edge gets. Pure, so the rules are tested without a canvas.
 */
import type {
  DataFlow,
  DataFlowEdge,
  DataFlowEdgeKind,
  DataFlowNode,
  DataFlowNodeType
} from "@/types/dataFlow";

/** Left to right: where data enters, what moves it, where it rests, who uses it. */
export const COLUMN: Record<DataFlowNodeType, number> = {
  source: 0,
  pipeline: 1,
  store: 2,
  schema: 2,
  table: 2,
  function: 3,
  app: 4
};

export const COLUMN_LABELS = ["Sources", "Pipelines", "Stores", "Functions", "Apps"] as const;

export const columnOf = (node: Pick<DataFlowNode, "type">) => COLUMN[node.type];

/** One drawn line: a data edge, or a function's membership of its app. */
export type FlowLink = {
  id: string;
  source: string;
  target: string;
  /** The data edge; absent on a membership link. */
  edge?: DataFlowEdge;
  /** True when the data runs right to left (an app writing a store sits right of it). */
  backward: boolean;
};

export const edgeId = (edge: Pick<DataFlowEdge, "from" | "to" | "kind">) =>
  `${edge.kind}:${edge.from}->${edge.to}`;

/**
 * Every line the canvas draws: each data edge in its own direction, and one from
 * each unfolded function to its app so the two sit together. An edge whose end is
 * not in the answer is dropped rather than drawn to nowhere.
 */
export function buildLinks(flow: Pick<DataFlow, "nodes" | "edges">): FlowLink[] {
  const byId = new Map(flow.nodes.map((n) => [n.id, n] as const));
  const links: FlowLink[] = [];
  for (const edge of flow.edges) {
    const from = byId.get(edge.from);
    const to = byId.get(edge.to);
    if (!from || !to) continue;
    links.push({
      id: edgeId(edge),
      source: edge.from,
      target: edge.to,
      edge,
      backward: columnOf(from) > columnOf(to)
    });
  }
  for (const node of flow.nodes) {
    if (node.type === "function" && node.parent && byId.has(node.parent)) {
      links.push({
        id: `member:${node.id}`,
        source: node.id,
        target: node.parent,
        backward: false
      });
    }
  }
  return links;
}

/**
 * The links as the layout sees them: each pointed from its lower column to its
 * higher, so no edge runs against ELK's column partitions. The drawing keeps the
 * real direction; only the placement is computed this way.
 */
export function layoutEdges(links: FlowLink[]): { id: string; source: string; target: string }[] {
  return links.map(({ id, source, target, backward }) =>
    backward ? { id, source: target, target: source } : { id, source, target }
  );
}

const text = (value: unknown): string | undefined =>
  typeof value === "string" && value.trim() !== "" ? value : undefined;

const joined = (...parts: (string | undefined)[]) => parts.filter(Boolean).join(" · ");

/** A node's second line: what it is and where it lives. */
export function captionOf(node: DataFlowNode): string {
  const m = node.meta;
  const workspace = text(m.workspace);
  switch (node.type) {
    case "source":
      return joined("source", workspace);
    case "pipeline": {
      const into = text(m.dataset);
      const kind = text(m.source_kind);
      return kind && into ? `${kind} → ${into}` : joined("pipeline", workspace);
    }
    case "schema":
    case "table":
      return m.plane === "oltp"
        ? "org OLTP store"
        : joined(text(m.database) ?? "Airhouse", workspace);
    case "store":
      return joined(m.customer_warehouse === true ? "customer warehouse" : "database", workspace);
    case "app": {
      const env = text(m.environment);
      return joined(text(m.slug), env === "production" ? undefined : `${env ?? "no"} build`);
    }
    case "function":
      return "function";
  }
}

export type NodeFlag = { tone: "warn" | "danger"; label: string; hint: string };

/** What is wrong with a node's declaration, if anything. */
export function flagsOf(node: DataFlowNode): NodeFlag[] {
  const flags: NodeFlag[] = [];
  if (node.meta.invalid === true) {
    flags.push({
      tone: "danger",
      label: "does not parse",
      hint: "The compiled .airway.yml does not read as a pipeline, so nothing it declares is drawn."
    });
  }
  if (node.meta.missing === true) {
    flags.push({
      tone: "warn",
      label: "not compiled",
      hint: "An app uploads into this pipeline, but no workspace of the org compiles one by this name."
    });
  }
  if (node.meta.ambiguous === true) {
    flags.push({
      tone: "warn",
      label: "name held twice",
      hint: "More than one workspace compiles a pipeline by this name, so an upload naming it is refused."
    });
  }
  return flags;
}

/** A line's look. Colours are theme variables, so both themes hold. */
export type LinkStyle = { stroke: string; dash?: string; arrow: boolean };

const DASH: Record<DataFlowEdgeKind, string | undefined> = {
  reads: undefined,
  writes: undefined,
  triggers: "6 4",
  ships: "2 3"
};

export function linkStyle(link: FlowLink): LinkStyle {
  if (!link.edge) return { stroke: "var(--border)", dash: "2 3", arrow: false };
  if (link.edge.approved === false) {
    return { stroke: "var(--warning)", dash: "6 4", arrow: true };
  }
  const stroke = link.edge.kind === "ships" ? "var(--primary)" : "var(--muted-foreground)";
  return { stroke, dash: DASH[link.edge.kind], arrow: true };
}

export const EDGE_KIND_WORDS: Record<DataFlowEdgeKind, string> = {
  reads: "reads",
  writes: "writes",
  triggers: "triggers",
  ships: "ships"
};

/** One edge in words, for the details list: what declared it and what it names. */
export function describeEdge(edge: DataFlowEdge): string {
  const parts = [`via ${edge.via.join(", ")}`];
  if (edge.tables?.length) parts.push(`tables: ${edge.tables.join(", ")}`);
  if (edge.functions?.length) parts.push(`functions: ${edge.functions.join(", ")}`);
  if (edge.approved === false) parts.push("not approved by an org admin");
  return parts.join(" · ");
}

/** The edges into and out of `id`. */
export function neighboursOf(flow: Pick<DataFlow, "edges">, id: string) {
  return {
    incoming: flow.edges.filter((e) => e.to === id),
    outgoing: flow.edges.filter((e) => e.from === id)
  };
}

/** The page's lead numbers. */
export function countFlow(flow: Pick<DataFlow, "nodes" | "edges">) {
  const of = (...types: DataFlowNodeType[]) =>
    flow.nodes.filter((n) => types.includes(n.type)).length;
  return {
    sources: of("source"),
    pipelines: of("pipeline"),
    stores: of("store", "schema", "table"),
    apps: of("app"),
    edges: flow.edges.length
  };
}

/** `app:<id>` → the app's id, for the console link. */
export const appIdOf = (node: DataFlowNode) =>
  node.type === "app" ? node.id.slice("app:".length) : undefined;

/**
 * A placement that cannot fail: each node in its column, stacked in the order the
 * server sent them. What the canvas draws when ELK refuses a graph, so a layout
 * failure costs tidiness, not the page.
 */
export function columnPlacement(
  nodes: Pick<DataFlowNode, "id" | "type">[],
  box: { width: number; height: number; gapX: number; gapY: number }
): Map<string, { x: number; y: number }> {
  const rows = new Map<number, number>();
  const placed = new Map<string, { x: number; y: number }>();
  for (const node of nodes) {
    const column = columnOf(node);
    const row = rows.get(column) ?? 0;
    rows.set(column, row + 1);
    placed.set(node.id, {
      x: column * (box.width + box.gapX),
      y: row * (box.height + box.gapY)
    });
  }
  return placed;
}
