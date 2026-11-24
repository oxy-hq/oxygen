import { describe, expect, it } from "vitest";
import type { WorldModel, WorldModelEntity } from "@/types/worldModel";
import { worldModelCounts } from "./counts";

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

describe("worldModelCounts", () => {
  const model: WorldModel = {
    entities: [entity("Store", 0, 2), entity("Order", 1, 3), entity("Line item", 2, 1)],
    edges: [
      { from: "Order", to: "Store", functional: true },
      { from: "Line item", to: "Order", functional: true }
    ]
  };

  it("counts entities, connections and measures", () => {
    expect(worldModelCounts(model)).toEqual({ entities: 3, connections: 2, measures: 6 });
  });

  it("counts an entity the model lists twice once, with its measures once", () => {
    const twice: WorldModel = {
      ...model,
      entities: [...model.entities, entity("Store", 0, 2)]
    };
    expect(worldModelCounts(twice)).toEqual({ entities: 3, connections: 2, measures: 6 });
  });

  it("counts a pair connected both ways once per direction", () => {
    const both: WorldModel = {
      entities: [entity("Store", 0), entity("Order", 1)],
      edges: [
        { from: "Order", to: "Store", functional: true },
        { from: "Store", to: "Order", functional: false }
      ]
    };
    expect(worldModelCounts(both).connections).toBe(2);
  });
});
