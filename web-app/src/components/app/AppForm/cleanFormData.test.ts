import { describe, expect, it } from "vitest";
import { cleanAppFormData } from "./cleanFormData";

// `AppConfig` parses the whole `.app.yml` strictly when the app runs: one
// required key missing anywhere fails every task and display in it.
describe("cleanAppFormData — keys where empty is not missing", () => {
  it("writes empty `tasks` and `display` as [] rather than leaving them out", () => {
    // The editor deletes a key this leaves out, and an app without either
    // does not parse.
    expect(cleanAppFormData({ tasks: [], display: [] })).toEqual({ tasks: [], display: [] });
  });

  it("keeps a row's empty `children` and a markdown block's empty `content`", () => {
    const display = [
      { type: "row", columns: undefined, children: [] },
      { type: "markdown", content: "" }
    ];
    expect(cleanAppFormData({ display }).display).toEqual([
      { type: "row", children: [] },
      { type: "markdown", content: "" }
    ]);
  });

  it("keeps a control's `default: \"\"`, which renders as '' where none renders as no value", () => {
    const display = [{ type: "control", name: "region", control_type: "select", default: "" }];
    expect(cleanAppFormData({ display }).display).toEqual(display);
  });

  it("applies the task rules to the app's tasks", () => {
    const tasks = [
      { name: "c", type: "conditional", conditions: [{ if: "{{ x }}", tasks: [] }] },
      { name: "q", type: "looker_query", integration: "l", fields: [] }
    ];
    expect(cleanAppFormData({ tasks }).tasks).toEqual(tasks);
  });

  it("still strips what the form leaves empty where empty and missing are the same", () => {
    const display = [{ type: "bar_chart", data: "t", x: "a", y: "b", title: "", series: "" }];
    expect(cleanAppFormData({ display }).display).toEqual([
      { type: "bar_chart", data: "t", x: "a", y: "b" }
    ]);
  });
});
