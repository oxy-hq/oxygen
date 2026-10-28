import { describe, expect, it } from "vitest";
import type { WorldModel, WorldModelEntity } from "@/types/worldModel";
import { previewLayout, worldModelCounts } from "./previewLayout";

const entity = (id: string, depth: number, measures = 0): WorldModelEntity => ({
  id,
  label: id,
  view: `${id}_view`,
  depth,
  dimensions: [],
  own_measures: Array.from({ length: measures }, (_, i) => ({
    name: `${id}_m${i}`,
    measure_type: "sum",
    additivity: "additive"
  })),
  induced_measures: []
});

describe("previewLayout", () => {
  const model: WorldModel = {
    entities: [entity("Store", 0, 2), entity("Order", 1, 3), entity("Line item", 2, 1)],
    edges: [
      { from: "Order", to: "Store", functional: true },
      { from: "Line item", to: "Order", functional: true }
    ]
  };

  it("puts one column per depth and draws every connection between placed entities", () => {
    const layout = previewLayout(model);
    const x = Object.fromEntries(layout.nodes.map((n) => [n.id, n.x]));
    expect(x.Store).toBeLessThan(x.Order);
    expect(x.Order).toBeLessThan(x["Line item"]);
    expect(layout.edges).toHaveLength(2);
    expect(layout.hidden).toBe(0);
  });

  it("keeps every node inside the drawing", () => {
    for (const n of previewLayout(model).nodes) {
      expect(n.x).toBeGreaterThanOrEqual(0);
      expect(n.x).toBeLessThanOrEqual(100);
      expect(n.y).toBeGreaterThan(0);
      expect(n.y).toBeLessThan(100);
    }
  });

  it("caps each column and counts what it left off, so the card never overflows", () => {
    const flat: WorldModel = {
      entities: Array.from({ length: 20 }, (_, i) => entity(`E${i}`, 0)),
      edges: []
    };
    const layout = previewLayout(flat, { maxColumns: 4, maxPerColumn: 4 });
    expect(layout.nodes).toHaveLength(16);
    expect(layout.hidden).toBe(4);
    // A flat model is spread across the columns rather than drawn as one list.
    expect(new Set(layout.nodes.map((n) => n.x)).size).toBe(4);
  });

  it("counts entities, connections and measures", () => {
    expect(worldModelCounts(model)).toEqual({ entities: 3, connections: 2, measures: 6 });
  });
});
