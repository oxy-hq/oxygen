/**
 * What of the server's answer the canvas draws: the node kinds left in, a search,
 * and which groups are folded. A big org reads as a handful of groups — one per
 * column and workspace, one per database — that open on a click. Pure, so the
 * folding rules are tested without a canvas.
 */
import type { DataFlow, DataFlowEdge, DataFlowNode, DataFlowNodeType } from "@/types/dataFlow";
import { captionOf, type Flow, flagsOf, type NodeFlag } from "./dataFlowModel";

/** A group with this many members or fewer is never folded: one card saves nothing. */
export const FOLD_ABOVE = 3;

/** The kinds a viewer can hide, one per column. */
export type HideableKind = "source" | "pipeline" | "store" | "function" | "app";
export const HIDEABLE_KINDS: { kind: HideableKind; label: string }[] = [
  { kind: "source", label: "Sources" },
  { kind: "pipeline", label: "Pipelines" },
  { kind: "store", label: "Stores" },
  { kind: "function", label: "Functions" },
  { kind: "app", label: "Apps" }
];

const KIND_OF: Record<DataFlowNodeType, HideableKind> = {
  source: "source",
  pipeline: "pipeline",
  store: "store",
  schema: "store",
  table: "store",
  function: "function",
  app: "app"
};

/** `open` is the groups unfolded by hand; `"all"` unfolds every one. */
export type ViewOptions = {
  hidden: HideableKind[];
  search: string;
  open: string[] | "all";
};

/** A group's members, and how many of them carry a flag, at the worst tone among them. */
export type GroupInfo = {
  members: string[];
  flagged: number;
  tone: NodeFlag["tone"] | null;
};

/** A folded group's card. Its `type` is its members' (so its column and icon are). */
export type GroupNode = DataFlowNode & { group: GroupInfo };

export const isGroup = (node: DataFlowNode): node is GroupNode => "group" in node;

/** An edge as drawn: a folded edge stands for `count` declared ones. */
export type ViewEdge = DataFlowEdge & { count?: number };

export type FlowView = Pick<DataFlow, "nodes" | "edges"> & {
  nodes: DataFlowNode[];
  edges: ViewEdge[];
  /** Every group with something to fold, open or not: id → member ids. */
  groups: Map<string, string[]>;
  /** The search matched these (empty when there is no search). */
  matches: Set<string>;
  /** Groups held open by the focus or a search match, whatever `open` says:
   *  folding one of them by hand would change nothing. */
  held: Set<string>;
};

const text = (value: unknown): string | undefined =>
  typeof value === "string" && value !== "" ? value : undefined;

const workspaceKey = (node: DataFlowNode) =>
  text(node.meta.workspace_id) ?? text(node.meta.workspace) ?? "-";

/**
 * The group a node folds into, or `null` for one that never folds (a function: it
 * is only drawn when its app is focused, which is a choice to see it). Stores fold
 * by database, so a named database and its schemas sit in one group; an OLTP
 * schema in the org's one OLTP store.
 */
export function groupOf(node: DataFlowNode): string | null {
  switch (node.type) {
    case "source":
    case "pipeline":
    case "app":
      return `group:${node.type}:${workspaceKey(node)}`;
    case "store":
      return `group:db:${workspaceKey(node)}:${node.label}`;
    case "schema":
    case "table":
      return node.meta.plane === "oltp"
        ? "group:oltp"
        : `group:db:${workspaceKey(node)}:${text(node.meta.database) ?? "airhouse"}`;
    case "function":
      return null;
  }
}

const PLURAL: Record<DataFlowNodeType, string> = {
  source: "sources",
  pipeline: "pipelines",
  store: "databases",
  schema: "schemas",
  table: "tables",
  app: "apps",
  function: "functions"
};

function groupLabel(members: DataFlowNode[]): string {
  const first = members[0];
  const n = members.length;
  switch (first.type) {
    case "source":
    case "pipeline":
    case "app":
      return `${n} ${PLURAL[first.type]}`;
    default: {
      if (first.meta.plane === "oltp") return `OLTP store · ${n}`;
      const db = members.find((m) => m.type === "store")?.label ?? text(first.meta.database);
      return `${db ?? "Airhouse"} · ${n}`;
    }
  }
}

function groupNode(id: string, members: DataFlowNode[]): GroupNode {
  // A database group holds the database and its schemas; it takes the schemas' type
  // so its icon says what is folded inside.
  const typed = members.find((m) => m.type !== "store") ?? members[0];
  const flags = members.map(flagsOf).filter((f) => f.length > 0);
  const tones = new Set(flags.flat().map((f) => f.tone));
  return {
    id,
    type: typed.type,
    label: groupLabel(members),
    meta: { workspace: members[0].meta.workspace, plane: members[0].meta.plane },
    group: {
      members: members.map((m) => m.id),
      flagged: flags.length,
      tone: tones.has("danger") ? "danger" : tones.has("warn") ? "warn" : null
    }
  };
}

const matchesSearch = (node: DataFlowNode, needle: string) =>
  node.label.toLowerCase().includes(needle) || node.id.toLowerCase().includes(needle);

/** The search's matches and the nodes one edge away from them. */
function searchScope(flow: Flow, needle: string) {
  const matches = new Set(flow.nodes.filter((n) => matchesSearch(n, needle)).map((n) => n.id));
  const keep = new Set(matches);
  for (const e of flow.edges) {
    if (matches.has(e.from)) keep.add(e.to);
    if (matches.has(e.to)) keep.add(e.from);
  }
  // A kept function keeps its app, so it is not drawn adrift.
  for (const n of flow.nodes) if (keep.has(n.id) && n.parent) keep.add(n.parent);
  return { matches, keep };
}

const union = <T extends string>(a?: T[], b?: T[]) =>
  a || b ? [...new Set([...(a ?? []), ...(b ?? [])])] : undefined;

/** One unapproved upload in a bundle is enough to draw it in the warning colour. */
const bothApproved = (a?: boolean, b?: boolean) =>
  a === undefined && b === undefined ? undefined : a !== false && b !== false;

/** Folded edges between the same two cards are one line that says how many it stands for. */
function mergeEdges(edges: DataFlowEdge[]): ViewEdge[] {
  const merged = new Map<string, ViewEdge>();
  for (const e of edges) {
    const key = `${e.kind}:${e.from}->${e.to}`;
    const prev = merged.get(key);
    merged.set(
      key,
      prev
        ? {
            ...prev,
            via: union(prev.via, e.via) ?? [],
            tables: union(prev.tables, e.tables),
            excluded: union(prev.excluded, e.excluded),
            from_functions: union(prev.from_functions, e.from_functions),
            to_functions: union(prev.to_functions, e.to_functions),
            events: union(prev.events, e.events),
            problems: union(prev.problems, e.problems),
            approved: bothApproved(prev.approved, e.approved),
            count: (prev.count ?? 1) + 1
          }
        : { ...e }
    );
  }
  return [...merged.values()];
}

/** `flow` keeping only `nodes` that pass, and the edges between those left. */
function keepNodes(flow: Flow, keep: (n: DataFlowNode) => boolean): Flow {
  const nodes = flow.nodes.filter(keep);
  const ids = new Set(nodes.map((n) => n.id));
  return { nodes, edges: flow.edges.filter((e) => ids.has(e.from) && ids.has(e.to)) };
}

/** Every group of `nodes` big enough to fold: id → its members, in order. */
export function foldableGroups(nodes: DataFlowNode[]): Map<string, DataFlowNode[]> {
  const members = new Map<string, DataFlowNode[]>();
  for (const n of nodes) {
    const g = groupOf(n);
    if (g) members.set(g, [...(members.get(g) ?? []), n]);
  }
  for (const [g, list] of members) if (list.length <= FOLD_ABOVE) members.delete(g);
  return members;
}

/** Each folded group as one card, in the place its first member held. */
function drawFolded(flow: Flow, folded: Map<string, string>, groups: Map<string, DataFlowNode[]>) {
  const drawn: DataFlowNode[] = [];
  const placed = new Set<string>();
  for (const n of flow.nodes) {
    const g = folded.get(n.id);
    if (!g) {
      // A function whose app folded hangs off nothing; it keeps its own card, unparented.
      drawn.push(n.parent && folded.has(n.parent) ? { ...n, parent: undefined } : n);
    } else if (!placed.has(g)) {
      placed.add(g);
      drawn.push(groupNode(g, groups.get(g) ?? [n]));
    }
  }
  const end = (id: string) => folded.get(id) ?? id;
  const edges = flow.edges
    .map((e) => ({ ...e, from: end(e.from), to: end(e.to) }))
    .filter((e) => e.from !== e.to);
  return { nodes: drawn, edges: mergeEdges(edges) };
}

/**
 * The flow as drawn. Hidden kinds go first (and their edges with them), then the
 * search keeps its matches and their neighbours, then each group larger than
 * `FOLD_ABOVE` folds unless it is open, holds the focus, or holds a match.
 */
export function viewFlow(flow: Flow, options: ViewOptions, focus: string | null = null): FlowView {
  const hidden = new Set(options.hidden);
  let shown = keepNodes(flow, (n) => !hidden.has(KIND_OF[n.type]));
  const needle = options.search.trim().toLowerCase();
  let matches = new Set<string>();
  if (needle) {
    const scope = searchScope(shown, needle);
    matches = scope.matches;
    shown = keepNodes(shown, (n) => scope.keep.has(n.id));
  }
  const groups = foldableGroups(shown.nodes);
  const held = new Set<string>();
  const folded = new Map<string, string>(); // member id → its folded group's id
  for (const [g, list] of groups) {
    if (list.some((m) => m.id === focus || matches.has(m.id))) held.add(g);
    const open = options.open === "all" || options.open.includes(g) || held.has(g);
    if (!open) for (const m of list) folded.set(m.id, g);
  }
  const ids = new Map([...groups].map(([g, list]) => [g, list.map((m) => m.id)] as const));
  const drawn = folded.size === 0 ? shown : drawFolded(shown, folded, groups);
  return { ...drawn, groups: ids, matches, held };
}

/** The group `id` sits in, when that group is one that folds. */
export function foldableGroupOf(view: Pick<FlowView, "groups">, id: string): string | null {
  for (const [g, list] of view.groups) if (list.includes(id)) return g;
  return null;
}

/** A card's second line: a group says where it lives and that it opens. */
export function captionFor(node: DataFlowNode): string {
  if (!isGroup(node)) return captionOf(node);
  const workspace = text(node.meta.workspace);
  return [workspace, "click to expand"].filter(Boolean).join(" · ");
}

/** A group carries its members' worst flag, so a fold never hides or softens a problem. */
export function flagsFor(node: DataFlowNode): NodeFlag[] {
  if (!isGroup(node)) return flagsOf(node);
  const { flagged: n, tone } = node.group;
  return n > 0 && tone
    ? [
        {
          tone,
          label: `${n} need${n === 1 ? "s" : ""} a look`,
          hint: "A node folded into this group has a declaration problem. Expand it to see which."
        }
      ]
    : [];
}
