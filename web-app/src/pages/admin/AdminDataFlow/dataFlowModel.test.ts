import { describe, expect, it } from "vitest";
import type { DataFlowEdge, DataFlowNode } from "@/types/dataFlow";
import {
  buildLinks,
  captionOf,
  columnOf,
  columnPlacement,
  countFlow,
  describeEdge,
  EDGE_KIND_WORDS,
  flagsOf,
  layoutEdges,
  linkStyle,
  neighboursOf
} from "./dataFlowModel";

const node = (
  id: string,
  type: DataFlowNode["type"],
  meta = {},
  parent?: string
): DataFlowNode => ({
  id,
  type,
  label: id,
  parent,
  meta
});

const edge = (from: string, to: string, kind: DataFlowEdge["kind"], extra = {}): DataFlowEdge => ({
  from,
  to,
  kind,
  declared: true,
  via: ["test"],
  ...extra
});

const nodes = [
  node("source:w:toast", "source", { workspace: "main" }),
  node("pipe:w:toast", "pipeline", { source_kind: "toast", dataset: "toast" }),
  node("schema:w:toast", "schema", { plane: "warehouse", workspace: "main" }),
  node("schema:w:app_x", "schema", { plane: "warehouse", workspace: "main" }),
  node("oltp:app_x", "schema", { plane: "oltp" }),
  node("schema:w:oltp_x", "schema", { plane: "warehouse", workspace: "main" }),
  node("app:a", "app", { slug: "x", environment: "production" }),
  node("fn:a:sync", "function", {}, "app:a")
];
const byId = (id: string) => {
  const found = nodes.find((n) => n.id === id);
  if (!found) throw new Error(`no node ${id}`);
  return found;
};

const edges = [
  edge("source:w:toast", "pipe:w:toast", "reads"),
  edge("pipe:w:toast", "schema:w:toast", "writes", { tables: ["orders"] }),
  edge("schema:w:toast", "fn:a:sync", "triggers"),
  edge("fn:a:sync", "schema:w:app_x", "writes"),
  edge("app:a", "oltp:app_x", "writes"),
  // Store to store, one column: neither forward nor back.
  edge("oltp:app_x", "schema:w:oltp_x", "ships", { tables: ["leases"] }),
  edge("fn:a:sync", "pipe:w:toast", "triggers", { approved: false })
];

describe("columns", () => {
  it("runs sources, pipelines, stores, functions, apps left to right", () => {
    const order = ["source", "pipeline", "schema", "function", "app"] as const;
    const columns = order.map((type) => columnOf({ type }));
    expect(columns).toEqual([...columns].sort((a, b) => a - b));
    expect(new Set(columns).size).toBe(order.length);
    expect(columnOf({ type: "table" })).toBe(columnOf({ type: "store" }));
  });
});

describe("buildLinks", () => {
  const links = buildLinks({ nodes, edges });

  it("marks a link that runs right to left, and only those", () => {
    const backward = links.filter((l) => l.backward).map((l) => l.id);
    expect(backward).toEqual([
      "writes:fn:a:sync->schema:w:app_x",
      "writes:app:a->oltp:app_x",
      "triggers:fn:a:sync->pipe:w:toast"
    ]);
  });

  it("joins an unfolded function to its app", () => {
    const member = links.find((l) => l.id === "member:fn:a:sync");
    expect(member).toMatchObject({ source: "fn:a:sync", target: "app:a", backward: false });
    expect(member?.edge).toBeUndefined();
  });

  it("drops an edge whose end is not in the answer", () => {
    const cut = buildLinks({ nodes, edges: [...edges, edge("app:a", "schema:gone", "writes")] });
    expect(cut.map((l) => l.id)).not.toContain("writes:app:a->schema:gone");
  });

  it("lays every link out from its lower column to its higher", () => {
    const byId = new Map(nodes.map((n) => [n.id, n] as const));
    for (const e of layoutEdges(links)) {
      const from = byId.get(e.source);
      const to = byId.get(e.target);
      if (!from || !to) throw new Error(`unknown end in ${e.id}`);
      expect(columnOf(from)).toBeLessThanOrEqual(columnOf(to));
    }
  });
});

describe("words", () => {
  it("captions each node with what it is and where it lives", () => {
    expect(captionOf(byId("pipe:w:toast"))).toBe("toast → toast");
    expect(captionOf(byId("schema:w:toast"))).toBe("Airhouse · main");
    expect(captionOf(byId("oltp:app_x"))).toBe("org OLTP store");
    expect(captionOf(byId("app:a"))).toBe("x");
    expect(captionOf(node("app:b", "app", { slug: "y", environment: "staging" }))).toBe(
      "y · staging build"
    );
    expect(captionOf(node("app:c", "app", { slug: "z", environment: null }))).toBe("z · no build");
    expect(
      captionOf(node("store:w:sf", "store", { customer_warehouse: true, workspace: "main" }))
    ).toBe("customer warehouse · main");
  });

  it("flags what is wrong with a declaration, and nothing when nothing is", () => {
    expect(flagsOf(byId("pipe:w:toast"))).toEqual([]);
    const flags = flagsOf(node("pipe:w:x", "pipeline", { missing: true, ambiguous: true }));
    expect(flags.map((f) => f.label)).toEqual(["not compiled", "name held twice"]);
    expect(flagsOf(node("pipe:w:y", "pipeline", { invalid: true }))[0].tone).toBe("danger");
  });

  it("words an edge from the selected node's side", () => {
    expect(EDGE_KIND_WORDS.out.calls).toBe("calls");
    expect(EDGE_KIND_WORDS.in.calls).toBe("called by");
    expect(EDGE_KIND_WORDS.in.publishes).toBe("hears from");
  });

  it("names a missing app as an app, not a pipeline", () => {
    expect(flagsOf(node("app:missing:x", "app", { missing: true }))[0].label).toBe("no such app");
  });

  it("says a call may be either kind, and why one would be refused", () => {
    expect(describeEdge(edge("app:a", "fn:b:f", "calls", { via: ["calls"] }))).toBe(
      "via calls · call (now) or send (queued)"
    );
    expect(
      describeEdge(
        edge("fn:a:p", "fn:b:s", "publishes", {
          via: ["events", "on"],
          events: ["order.placed"],
          problems: ["not_in_catalog"]
        })
      )
    ).toBe(
      "via events, on · events: order.placed · refused: the event is not in the publisher's catalogue"
    );
  });

  it("describes an edge by what declared it and what it names", () => {
    const text = describeEdge(
      edge("a", "b", "writes", {
        via: ["airhouseMigrations", "airhouse"],
        tables: ["t"],
        from_functions: ["f"],
        approved: false
      })
    );
    expect(text).toBe(
      "via airhouseMigrations, airhouse · tables: t · written by: f · not approved by an org admin"
    );
  });

  it("names a publishes edge's publishers and subscribers apart", () => {
    const text = describeEdge(
      edge("app:shop", "app:shipping", "publishes", {
        via: ["events", "on"],
        events: ["order.placed"],
        from_functions: ["place-order"],
        to_functions: ["on-order-placed"]
      })
    );
    expect(text).toBe(
      "via events, on · events: order.placed · published by: place-order · heard by: on-order-placed"
    );
  });
});

describe("linkStyle", () => {
  const links = buildLinks({ nodes, edges });
  const styleOf = (id: string) => {
    const link = links.find((l) => l.id === id);
    if (!link) throw new Error(id);
    return linkStyle(link);
  };

  it("draws an unapproved upload in the warning colour", () => {
    expect(styleOf("triggers:fn:a:sync->pipe:w:toast").stroke).toBe("var(--warning)");
  });

  it("draws the change stream in the brand colour", () => {
    expect(styleOf("ships:oltp:app_x->schema:w:oltp_x").stroke).toBe("var(--primary)");
  });

  it("draws writes solid, triggers dashed, membership without an arrow", () => {
    expect(styleOf("writes:pipe:w:toast->schema:w:toast").dash).toBeUndefined();
    expect(styleOf("triggers:schema:w:toast->fn:a:sync").dash).toBeDefined();
    expect(styleOf("member:fn:a:sync").arrow).toBe(false);
  });

  const solo = (e: DataFlowEdge) =>
    linkStyle({ id: "x", source: e.from, target: e.to, edge: e, backward: false });

  it("moves the lines that send between systems, and nothing else", () => {
    expect(solo(edge("app:a", "fn:b:f", "calls")).moving).toBe(true);
    expect(solo(edge("fn:a:p", "fn:b:s", "publishes", { events: ["e.x"] })).moving).toBe(true);
    expect(styleOf("ships:oltp:app_x->schema:w:oltp_x").moving).toBe(true);
    expect(styleOf("writes:pipe:w:toast->schema:w:toast").moving).toBe(false);
    expect(styleOf("member:fn:a:sync").moving).toBe(false);
  });

  it("draws a call the platform would refuse in the warning colour, still", () => {
    const refused = solo(edge("app:a", "fn:b:f", "calls", { problems: ["not_callable"] }));
    expect(refused).toMatchObject({ stroke: "var(--warning)", moving: false });
  });

  it("tells a call from an event by its dash", () => {
    expect(solo(edge("app:a", "fn:b:f", "calls")).dash).toBeUndefined();
    expect(solo(edge("fn:a:p", "fn:b:s", "publishes")).dash).toBeDefined();
  });
});

describe("counts and neighbours", () => {
  it("counts every kind of store as a store", () => {
    expect(countFlow({ nodes, edges })).toEqual({
      sources: 1,
      pipelines: 1,
      stores: 4,
      apps: 1,
      edges: edges.length
    });
  });

  it("lists a node's edges by direction", () => {
    const { incoming, outgoing } = neighboursOf({ edges }, "pipe:w:toast");
    expect(incoming.map((e) => e.from)).toEqual(["source:w:toast", "fn:a:sync"]);
    expect(outgoing.map((e) => e.to)).toEqual(["schema:w:toast"]);
  });
});

describe("columnPlacement", () => {
  it("puts each node in its column and stacks a column in order, so nothing overlaps", () => {
    const box = { width: 200, height: 50, gapX: 100, gapY: 10 };
    const placed = columnPlacement(nodes, box);
    expect(placed.size).toBe(nodes.length);
    for (const n of nodes) {
      expect(placed.get(n.id)?.x).toBe(columnOf(n) * 300);
    }
    // The three stores share a column, one under the next.
    const stores = ["schema:w:toast", "schema:w:app_x", "oltp:app_x"].map(
      (id) => placed.get(id)?.y
    );
    expect(stores).toEqual([0, 60, 120]);
    const spots = [...placed.values()].map((p) => `${p.x},${p.y}`);
    expect(new Set(spots).size).toBe(spots.length);
  });
});
