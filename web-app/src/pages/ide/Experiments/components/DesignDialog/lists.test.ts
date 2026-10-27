import { describe, expect, it } from "vitest";
import type { MetricNode } from "@/types/metricTree";
import type { WorldModelEntity } from "@/types/worldModel";
import {
  automationGroups,
  automationMatch,
  dimensionGroups,
  dimensionMatch,
  entityGroups,
  entityMatch,
  measureGroups,
  measureMatch,
  timeDimensionGroups,
  timeDimensionMatch,
  withCurrent
} from "./lists";

const node = (
  view: string,
  measure: string,
  label = measure,
  description?: string
): MetricNode => ({
  id: `${view}.${measure}`,
  view,
  measure,
  label,
  description,
  measure_type: "sum",
  is_composite: false,
  drillable: false
});
const NODES = [
  node("refunds", "rate", "Refund rate", "Share refunded"),
  node("orders", "total_order_value", "Total order value"),
  node("orders", "count", "count")
];

const entity = (id: string, view: string, dims: string[]): WorldModelEntity => ({
  id,
  label: id,
  view,
  description: null,
  depth: 0,
  dimensions: dims.map((name) => ({ name, dim_type: "string" })),
  own_measures: [],
  induced_measures: []
});
const ENTITIES = [
  entity("store", "stores", ["region", "format"]),
  entity("customer", "customers", ["tier"])
];

describe("measures", () => {
  it("are grouped by view, sorted, with the label (or description) beside each", () => {
    const groups = measureGroups(NODES);
    expect(groups.map((g) => g.title)).toEqual(["orders", "refunds"]);
    expect(groups[0].options.map((o) => o.value)).toEqual([
      "orders.count",
      "orders.total_order_value"
    ]);
    expect(groups[0].options[1].detail).toBe("Total order value");
    expect(groups[1].options[0].detail).toBe("Refund rate");
    expect(groups[1].options[0].search).toContain("Share refunded");
  });

  it("say whether a value is one, and say nothing before the list has loaded", () => {
    expect(measureMatch("orders.count", NODES)?.tone).toBe("success");
    expect(measureMatch("orders.nope", NODES)?.tone).toBe("warning");
    expect(measureMatch("orders.nope", undefined)).toBeNull();
    expect(measureMatch("  ", NODES)).toBeNull();
  });
});

describe("entities and their dimensions", () => {
  it("lists entities by id", () => {
    expect(entityGroups(ENTITIES)[0].options.map((o) => o.value)).toEqual(["customer", "store"]);
    expect(entityMatch("store", ENTITIES)?.tone).toBe("success");
    expect(entityMatch("shop", ENTITIES)?.tone).toBe("warning");
    expect(entityMatch("shop", undefined)).toBeNull();
  });

  it("lists the dimensions of the chosen entity only", () => {
    expect(dimensionGroups("store", ENTITIES)[0].options.map((o) => o.value)).toEqual([
      "format",
      "region"
    ]);
    expect(dimensionGroups("unknown", ENTITIES)).toEqual([]);
    expect(dimensionGroups("", ENTITIES)).toEqual([]);
  });

  it("accepts a dimension with or without its view, as the server does", () => {
    expect(dimensionMatch("region", "store", ENTITIES)?.tone).toBe("success");
    expect(dimensionMatch("stores.region", "store", ENTITIES)?.tone).toBe("success");
    expect(dimensionMatch("tier", "store", ENTITIES)?.tone).toBe("warning");
    expect(dimensionMatch("region", "unknown", ENTITIES)).toBeNull();
  });
});

describe("time dimensions", () => {
  const byView = { orders: ["orders.order_date", "orders.shipped_at"], refunds: ["refunds.day"] };

  it("are those of the outcome's view when it is known", () => {
    const groups = timeDimensionGroups("orders.total_order_value", byView);
    expect(groups.map((g) => g.title)).toEqual(["orders"]);
    expect(groups[0].options.map((o) => o.value)).toEqual([
      "orders.order_date",
      "orders.shipped_at"
    ]);
  });

  it("are every view's when the outcome is not known yet", () => {
    expect(timeDimensionGroups("", byView).map((g) => g.title)).toEqual(["orders", "refunds"]);
  });

  it("flag a date that the outcome's view does not have", () => {
    expect(timeDimensionMatch("orders.order_date", "orders.total_order_value", byView)?.tone).toBe(
      "success"
    );
    expect(timeDimensionMatch("refunds.day", "orders.total_order_value", byView)?.tone).toBe(
      "warning"
    );
    expect(timeDimensionMatch("x.y", "", byView)?.tone).toBe("warning");
    expect(timeDimensionMatch("x.y", "", undefined)).toBeNull();
  });
});

describe("automations", () => {
  // The real listing carries a path and no name.
  const automations = [
    { path: "automations/promo.automation.yml", description: "Turns the promo on" },
    { path: "procedures/fruit_summary.procedure.yml" },
    { path: "a/alpha.automation.yml" },
    { path: "b/alpha.workflow.yml" }
  ];

  it("are named by the file's basename without its extension, with the path beside them", () => {
    const options = automationGroups(automations)[0].options;
    expect(options.map((o) => o.value)).toEqual(["alpha", "fruit_summary", "promo"]);
    expect(options.find((o) => o.value === "fruit_summary")?.detail).toBe(
      "procedures/fruit_summary.procedure.yml"
    );
  });

  it("must resolve to exactly one", () => {
    expect(automationMatch("promo", automations)?.tone).toBe("success");
    expect(automationMatch("fruit_summary", automations)?.tone).toBe("success");
    expect(automationMatch("nope", automations)?.tone).toBe("warning");
    expect(automationMatch("alpha", automations)?.text).toContain("2 automations");
    expect(automationMatch("alpha", undefined)).toBeNull();
  });
});

describe("injected measures", () => {
  const nodes = [...NODES, node("astronauts", "__oxy_row_count")];

  it("are left out of the options but never flagged when stored", () => {
    expect(measureGroups(nodes).map((g) => g.title)).toEqual(["orders", "refunds"]);
    expect(measureMatch("astronauts.__oxy_row_count", nodes)?.tone).toBe("success");
  });
});

describe("withCurrent", () => {
  it("keeps a stored value that is not a standard choice, so the select still shows it", () => {
    const choices = [{ value: "0.05", label: "0.05" }];
    expect(withCurrent(choices, "0.05")).toEqual(choices);
    expect(withCurrent(choices, "")).toEqual(choices);
    expect(withCurrent(choices, "0.07")).toEqual([
      ...choices,
      { value: "0.07", label: "0.07 (custom)" }
    ]);
  });
});
