import { describe, expect, it } from "vitest";
import type { DataFlowEdge, DataFlowNode } from "@/types/dataFlow";
import {
  captionFor,
  FOLD_ABOVE,
  flagsFor,
  foldableGroupOf,
  foldableGroups,
  groupOf,
  isGroup,
  type ViewOptions,
  viewFlow
} from "./dataFlowView";

const node = (
  id: string,
  type: DataFlowNode["type"],
  meta: Record<string, unknown> = {},
  parent?: string
): DataFlowNode => ({ id, type, label: id, parent, meta: { workspace_id: "w", ...meta } });

const edge = (from: string, to: string, kind: DataFlowEdge["kind"], extra = {}): DataFlowEdge => ({
  from,
  to,
  kind,
  declared: true,
  via: ["airway"],
  ...extra
});

const range = (n: number) => Array.from({ length: n }, (_, i) => i);
const BIG = FOLD_ABOVE + 1;

// BIG sources, each read by its own pipeline, each writing its own Airhouse schema;
// one app writing an OLTP schema; one store with one schema.
const nodes = [
  ...range(BIG).map((i) => node(`source:w:s${i}`, "source")),
  ...range(BIG).map((i) => node(`pipe:w:s${i}`, "pipeline", i === 0 ? { invalid: true } : {})),
  ...range(BIG).map((i) => node(`schema:w:s${i}`, "schema", { plane: "warehouse" })),
  node("store:w:snow", "store"),
  node("schema:w:snow:raw", "schema", { plane: "warehouse", database: "snow" }),
  node("oltp:app_x", "schema", { plane: "oltp" }),
  node("app:a", "app", { slug: "x", environment: "production" })
];
const edges = [
  ...range(BIG).map((i) => edge(`source:w:s${i}`, `pipe:w:s${i}`, "reads")),
  ...range(BIG).map((i) => edge(`pipe:w:s${i}`, `schema:w:s${i}`, "writes", { tables: [`t${i}`] })),
  edge("app:a", "oltp:app_x", "writes", { via: ["migrations"] }),
  edge("schema:w:s0", "app:a", "triggers", { via: ["on"], approved: false })
];
const flow = { nodes, edges };

const opts = (o: Partial<ViewOptions> = {}): ViewOptions => ({
  hidden: [],
  search: "",
  open: [],
  ...o
});
const ids = (v: { nodes: DataFlowNode[] }) => v.nodes.map((n) => n.id);

describe("groupOf", () => {
  it("groups by column and workspace, stores by database, OLTP as one", () => {
    expect(groupOf(node("source:w:a", "source"))).toBe("group:source:w");
    expect(groupOf(node("app:a", "app", { workspace_id: "v" }))).toBe("group:app:v");
    expect(groupOf(node("schema:w:a", "schema", { plane: "warehouse" }))).toBe(
      "group:db:w:airhouse"
    );
    // A named database and the schemas in it fold together.
    expect(groupOf(node("schema:w:snow:raw", "schema", { database: "snow" }))).toBe(
      groupOf({ ...node("store:w:snow", "store"), label: "snow" })
    );
    expect(groupOf(node("oltp:x", "schema", { plane: "oltp" }))).toBe("group:oltp");
    expect(groupOf(node("fn:a:f", "function", {}, "app:a"))).toBeNull();
  });
});

describe("viewFlow folding", () => {
  it("folds a group above the threshold into one card and leaves small ones alone", () => {
    const v = viewFlow(flow, opts());
    expect(ids(v)).toEqual([
      "group:source:w",
      "group:pipeline:w",
      "group:db:w:airhouse",
      "store:w:snow",
      "schema:w:snow:raw",
      "oltp:app_x",
      "app:a"
    ]);
    expect([...v.groups.keys()]).toEqual([
      "group:source:w",
      "group:pipeline:w",
      "group:db:w:airhouse"
    ]);
  });

  it("never folds a group of exactly the threshold", () => {
    const small = {
      nodes: nodes.filter((n) => n.id !== `source:w:s${BIG - 1}`),
      edges: []
    };
    expect(ids(viewFlow(small, opts())).filter((id) => id.startsWith("source:"))).toHaveLength(
      FOLD_ABOVE
    );
  });

  it("merges the edges between two folded cards into one that counts them", () => {
    const v = viewFlow(flow, opts());
    const reads = v.edges.filter((e) => e.kind === "reads");
    expect(reads).toEqual([
      expect.objectContaining({ from: "group:source:w", to: "group:pipeline:w", count: BIG })
    ]);
    const writes = v.edges.find((e) => e.from === "group:pipeline:w");
    expect(writes?.tables).toEqual(range(BIG).map((i) => `t${i}`));
  });

  it("keeps an unapproved edge unapproved through a fold", () => {
    const v = viewFlow(flow, opts());
    expect(v.edges.find((e) => e.kind === "triggers")).toMatchObject({
      from: "group:db:w:airhouse",
      to: "app:a",
      approved: false
    });
  });

  it("draws a bundle of uploads unapproved when any one of them is", () => {
    const uploads = {
      nodes: [...nodes, node("fn:a:up", "function", {}, "app:a")],
      edges: range(BIG).map((i) => edge("app:a", `pipe:w:s${i}`, "triggers", { approved: i !== 2 }))
    };
    const bundle = viewFlow(uploads, opts()).edges.find((e) => e.to === "group:pipeline:w");
    expect(bundle).toMatchObject({ approved: false, count: BIG });
    const approved = { ...uploads, edges: uploads.edges.map((e) => ({ ...e, approved: true })) };
    expect(viewFlow(approved, opts()).edges[0].approved).toBe(true);
  });

  it("carries the events and refusals of every edge it folds", () => {
    const subs = {
      nodes: [...nodes, ...range(BIG).map((i) => node(`app:s${i}`, "app", { workspace_id: "v" }))],
      edges: range(BIG).map((i) =>
        edge("app:a", `app:s${i}`, "publishes", {
          events: [`e.n${i % 2}`],
          problems: i === 1 ? ["never_published"] : undefined
        })
      )
    };
    const bundle = viewFlow(subs, opts()).edges.find((e) => e.kind === "publishes");
    expect(bundle?.events).toEqual(["e.n0", "e.n1"]);
    expect(bundle?.problems).toEqual(["never_published"]);
  });

  it("keeps every column the change streams it folds leave in the store", () => {
    const shipped = {
      nodes: [
        ...nodes,
        ...range(BIG).map((i) => node(`oltp:app_s${i}`, "schema", { plane: "oltp" }))
      ],
      edges: range(BIG).map((i) =>
        edge(`oltp:app_s${i}`, `schema:w:s${i}`, "ships", { excluded: [`t${i}.email`] })
      )
    };
    const bundle = viewFlow(shipped, opts()).edges.find((e) => e.kind === "ships");
    expect(bundle?.excluded).toEqual(range(BIG).map((i) => `t${i}.email`));
  });

  it("draws no edge to a node that is not on the canvas", () => {
    const v = viewFlow(flow, opts());
    const drawn = new Set(ids(v));
    for (const e of v.edges) {
      expect(drawn.has(e.from) && drawn.has(e.to)).toBe(true);
    }
  });

  it("opens a group by hand, every group with `all`, and the focus's group by itself", () => {
    expect(ids(viewFlow(flow, opts({ open: ["group:source:w"] })))).toContain("source:w:s1");
    const all = viewFlow(flow, opts({ open: "all" }));
    expect(all.nodes.some(isGroup)).toBe(false);
    expect(all.edges).toHaveLength(edges.length);
    expect(ids(viewFlow(flow, opts(), "pipe:w:s2"))).toContain("pipe:w:s2");
  });

  it("carries a member's problem onto its folded group", () => {
    const group = viewFlow(flow, opts()).nodes.find((n) => n.id === "group:pipeline:w");
    if (!group) throw new Error("no pipeline group");
    // The member does not parse: the fold keeps its danger tone, not a softer warning.
    expect(flagsFor(group)).toEqual([expect.objectContaining({ tone: "danger" })]);
    expect(captionFor(group)).toContain("expand");
    const sources = viewFlow(flow, opts()).nodes.find((n) => n.id === "group:source:w");
    if (!sources) throw new Error("no source group");
    expect(flagsFor(sources)).toEqual([]);
  });

  it("says which groups the focus or a search holds open, whatever `open` says", () => {
    expect([...viewFlow(flow, opts({ open: "all" }), "pipe:w:s2").held]).toEqual([
      "group:pipeline:w"
    ]);
    expect([...viewFlow(flow, opts({ search: "source:" })).held]).toEqual(["group:source:w"]);
    expect(viewFlow(flow, opts({ open: "all" })).held.size).toBe(0);
  });

  it("lists every foldable group of the answer, whatever the filters hide", () => {
    // "Collapse one" under "expand all" starts from this list, so hidden groups stay open.
    expect([...foldableGroups(nodes).keys()]).toEqual([
      "group:source:w",
      "group:pipeline:w",
      "group:db:w:airhouse"
    ]);
    expect(viewFlow(flow, opts({ hidden: ["source"] })).groups.has("group:source:w")).toBe(false);
  });

  it("names the group a node folds into, only when that group folds", () => {
    const v = viewFlow(flow, opts({ open: "all" }));
    expect(foldableGroupOf(v, "source:w:s0")).toBe("group:source:w");
    expect(foldableGroupOf(v, "app:a")).toBeNull();
  });
});

describe("viewFlow filters", () => {
  it("hides a kind with its edges", () => {
    const v = viewFlow(flow, opts({ hidden: ["source", "store"], open: "all" }));
    expect(v.nodes.every((n) => n.type === "pipeline" || n.type === "app")).toBe(true);
    expect(v.edges).toEqual([]);
  });

  it("keeps a search's matches and their neighbours, and opens the groups they sit in", () => {
    const v = viewFlow(flow, opts({ search: "PIPE:w:s2" }));
    expect(ids(v).sort((a, b) => a.localeCompare(b))).toEqual([
      "pipe:w:s2",
      "schema:w:s2",
      "source:w:s2"
    ]);
    expect([...v.matches]).toEqual(["pipe:w:s2"]);
  });

  it("keeps a function's app when the function is kept", () => {
    const withFn = {
      nodes: [...nodes, node("fn:a:sync", "function", {}, "app:a")],
      edges: [...edges, edge("fn:a:sync", "schema:w:snow:raw", "writes")]
    };
    const v = viewFlow(withFn, opts({ search: "sync" }));
    expect(ids(v)).toContain("app:a");
  });
});
