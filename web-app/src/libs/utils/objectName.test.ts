import { describe, expect, it } from "vitest";
import { objectNameFromPath } from "./objectName";

describe("objectNameFromPath", () => {
  it.each([
    ["automations/daily_sales.automation.yml", "Daily sales"],
    ["agents/default.agentic.yml", "Default"],
    ["apps/store-forecast.app.yml", "Store forecast"],
    ["procedures/legacy.procedure.yaml", "Legacy"],
    ["queries/top_items.sql", "Top items"],
    ["revenue.app.yml", "Revenue"],
    ["nested/dir/x.workflow.yml", "X"]
  ])("%s reads as %s", (path, name) => {
    expect(objectNameFromPath(path)).toBe(name);
  });

  it("never returns a folder or an extension", () => {
    const name = objectNameFromPath("a/b/c.automation.yml");
    expect(name).not.toMatch(/[/.]/);
  });
});
