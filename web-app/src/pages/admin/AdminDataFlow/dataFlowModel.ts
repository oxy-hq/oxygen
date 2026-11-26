/**
 * The Data flow page's reading of `GET /customer-apps/data-flow`: which column a
 * node sits in, which way each link is drawn and laid out, and the words and
 * styles a node or edge gets. Pure, so the rules are tested without a canvas.
 */
import type {
  DataFlow,
  DataFlowEdge,
  DataFlowEdgeKind,
  DataFlowEdgeProblem,
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

/** What the canvas draws: the server's answer, or the view folded out of it. */
export type Flow = Pick<DataFlow, "nodes" | "edges">;

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

/** What a table the change stream ships is in Airhouse (`meta.object`). */
const SHIPPED_AS: Record<string, string> = {
  changes: "change table",
  view: "current-state view"
};

/** A schema's or table's second line: the OLTP store, or where in a warehouse it
 *  sits and, for a table the change stream ships into, which of the two it is. */
function placeCaption(m: DataFlowNode["meta"], workspace: string | undefined): string {
  if (m.plane === "oltp") return "org OLTP store";
  const object = text(m.object);
  return joined(object && SHIPPED_AS[object], text(m.database) ?? "Airhouse", workspace);
}

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
      return placeCaption(m, workspace);
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

const MISSING: Record<"app" | "pipeline", NodeFlag> = {
  pipeline: {
    tone: "warn",
    label: "not compiled",
    hint: "An app uploads into this pipeline, but no workspace of the org compiles one by this name."
  },
  app: {
    tone: "warn",
    label: "no such app",
    hint: "Another app calls or listens to an app by this slug, but no app of the org holds it."
  }
};

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
  if (node.meta.missing === true) flags.push(MISSING[node.type === "app" ? "app" : "pipeline"]);
  if (node.meta.ambiguous === true) {
    flags.push({
      tone: "warn",
      label: "name held twice",
      hint: "More than one workspace compiles a pipeline by this name, so an upload naming it is refused."
    });
  }
  return flags;
}

/**
 * A line's look. Colours are theme variables, so both themes hold. `moving` lines
 * carry a travelling dot: the ones where something is sent between systems (the
 * change stream, a call, an event), and any line lit by a selection.
 */
export type LinkStyle = { stroke: string; dash?: string; arrow: boolean; moving: boolean };

const DASH: Record<DataFlowEdgeKind, string | undefined> = {
  reads: undefined,
  writes: undefined,
  triggers: "6 4",
  ships: "2 3",
  calls: undefined,
  publishes: "8 3 2 3"
};

/** Between systems rather than into a store: drawn moving. */
const SENT: ReadonlySet<DataFlowEdgeKind> = new Set(["ships", "calls", "publishes"]);

const STROKE: Partial<Record<DataFlowEdgeKind, string>> = {
  ships: "var(--primary)",
  calls: "var(--foreground)",
  publishes: "var(--foreground)"
};

export function linkStyle(link: FlowLink): LinkStyle {
  if (!link.edge) return { stroke: "var(--border)", dash: "2 3", arrow: false, moving: false };
  const { kind, approved, problems } = link.edge;
  // Refused as declared: an unapproved upload, a call or event the platform would not run.
  if (approved === false || problems?.length) {
    return { stroke: "var(--warning)", dash: "6 4", arrow: true, moving: false };
  }
  return {
    stroke: STROKE[kind] ?? "var(--muted-foreground)",
    dash: DASH[kind],
    arrow: true,
    moving: SENT.has(kind)
  };
}

/**
 * An edge in words from the selected node's side: `out` when it is the edge's
 * source, `in` when it is the target, so "called by billing" never reads as
 * "calls billing".
 */
export const EDGE_KIND_WORDS: Record<"in" | "out", Record<DataFlowEdgeKind, string>> = {
  out: {
    reads: "read by",
    writes: "writes",
    triggers: "triggers",
    ships: "ships to",
    calls: "calls",
    publishes: "publishes to"
  },
  in: {
    reads: "reads",
    writes: "written by",
    triggers: "triggered by",
    ships: "shipped from",
    calls: "called by",
    publishes: "hears from"
  }
};

const PROBLEM_WORDS: Record<DataFlowEdgeProblem, string> = {
  app_not_found: "no app of the org has this slug",
  function_not_found: "the callee has no function by this name",
  not_callable: "the function is not marked callable",
  not_in_catalog: "the event is not in the publisher's catalogue",
  never_published: "no function of the publisher may publish it"
};

/**
 * How each end's folded functions are named: a `publishes` edge carries the
 * publishers on one side and the subscribers on the other, and one mixed list
 * would not say which is which.
 */
const FUNCTION_SIDE_DEFAULT: [string, string] = ["from functions", "to functions"];
const FUNCTION_SIDE_WORDS: Partial<Record<DataFlowEdgeKind, [string, string]>> = {
  publishes: ["published by", "heard by"],
  calls: ["called from", "functions called"],
  writes: ["written by", "written to"],
  reads: ["read from", "read by"],
  triggers: ["triggered from", "functions run"]
};

/** One edge in words, for the details list: what declared it and what it names. */
export function describeEdge(edge: DataFlowEdge): string {
  const parts = [`via ${edge.via.join(", ")}`];
  if (edge.kind === "calls") parts.push("call (now) or send (queued)");
  const [fromWord, toWord] = FUNCTION_SIDE_WORDS[edge.kind] ?? FUNCTION_SIDE_DEFAULT;
  const named: [string, string[] | undefined][] = [
    ["events", edge.events],
    ["tables", edge.tables],
    ["left in the store", edge.excluded],
    [fromWord, edge.from_functions],
    [toWord, edge.to_functions]
  ];
  for (const [word, items] of named) {
    if (items?.length) parts.push(`${word}: ${items.join(", ")}`);
  }
  if (edge.approved === false) parts.push("not approved by an org admin");
  for (const p of edge.problems ?? []) parts.push(`refused: ${PROBLEM_WORDS[p]}`);
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
