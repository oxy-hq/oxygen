import { describe, expect, it } from "vitest";
import type { FileTreeModel } from "@/types/file";
import { getObjectName, groupObjectsByType } from "./utils";

const makeFile = (name: string, path: string): FileTreeModel => ({
  name,
  path,
  is_dir: false,
  children: []
});

describe("getObjectName", () => {
  it("strips .agentic.yml", () => {
    expect(getObjectName(makeFile("analytics.agentic.yml", "analytics.agentic.yml"))).toBe(
      "analytics"
    );
  });

  it("strips .agentic.yaml", () => {
    expect(
      getObjectName(makeFile("training_coach.agentic.yaml", "training_coach.agentic.yaml"))
    ).toBe("training_coach");
  });

  it("strips .automation.yml (no regression)", () => {
    expect(getObjectName(makeFile("etl.automation.yml", "etl.automation.yml"))).toBe("etl");
  });

  it("strips .app.yml (no regression)", () => {
    expect(getObjectName(makeFile("dashboard.app.yml", "dashboard.app.yml"))).toBe("dashboard");
  });

  it("strips .airway.yml", () => {
    expect(getObjectName(makeFile("orders_etl.airway.yml", "orders_etl.airway.yml"))).toBe(
      "orders_etl"
    );
  });
});

describe("groupObjectsByType", () => {
  it("puts .agentic.yml files into the agents group", () => {
    const file = makeFile("analytics.agentic.yml", "analytics.agentic.yml");
    const result = groupObjectsByType([file]);
    expect(result.agents).toContain(file);
    expect(result.automations).toHaveLength(0);
    expect(result.apps).toHaveLength(0);
  });

  it("puts .agentic.yaml files into the agents group", () => {
    const file = makeFile("coach.agentic.yaml", "coach.agentic.yaml");
    const result = groupObjectsByType([file]);
    expect(result.agents).toContain(file);
  });

  it("does NOT put .agentic.yml files into automations group", () => {
    const file = makeFile("analytics.agentic.yml", "analytics.agentic.yml");
    const result = groupObjectsByType([file]);
    expect(result.automations).not.toContain(file);
  });

  it("ignores directory entries", () => {
    const dir: FileTreeModel = {
      name: "analytics.agentic.yml",
      path: "analytics.agentic.yml",
      is_dir: true,
      children: []
    };
    const result = groupObjectsByType([dir]);
    expect(result.agents).toHaveLength(0);
  });

  it("sorts every group by name, pipelines included", () => {
    // The files arrive from a directory walk, so their order is whatever the walk
    // produced — each group has to sort itself.
    const result = groupObjectsByType([
      makeFile("zeta.airway.yml", "b/zeta.airway.yml"),
      makeFile("Alpha.airway.yml", "c/Alpha.airway.yml"),
      makeFile("mid.airway.yml", "a/mid.airway.yml"),
      makeFile("zeta.automation.yml", "zeta.automation.yml"),
      makeFile("alpha.automation.yml", "alpha.automation.yml")
    ]);
    expect(result.pipelines.map((f) => f.name)).toEqual([
      "Alpha.airway.yml",
      "mid.airway.yml",
      "zeta.airway.yml"
    ]);
    expect(result.automations.map((f) => f.name)).toEqual([
      "alpha.automation.yml",
      "zeta.automation.yml"
    ]);
  });
});
