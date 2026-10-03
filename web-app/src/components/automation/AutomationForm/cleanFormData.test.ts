import { describe, expect, it } from "vitest";
import { cleanAutomationFormData } from "./cleanFormData";

// What the form holds, cleaned into what the editor writes. Each case is a key
// where an empty value and a missing one mean different things to
// `crates/agentic/automation/src/config.rs` (or its step parsers).
describe("cleanAutomationFormData — keys where empty is not missing", () => {
  it("keeps an empty task list: `tasks` is required, so dropping it stops the file parsing", () => {
    expect(cleanAutomationFormData({ name: "a", tasks: [] })).toEqual({ name: "a", tasks: [] });
  });

  it("keeps a new condition's empty `tasks` and an empty `conditions`", () => {
    const tasks = [
      {
        name: "branch",
        type: "conditional",
        conditions: [{ if_expr: "", if: "{{ x }}", tasks: [] }]
      },
      { name: "none_yet", type: "conditional", conditions: [] }
    ];
    expect(cleanAutomationFormData({ tasks }).tasks).toEqual([
      { name: "branch", type: "conditional", conditions: [{ if: "{{ x }}", tasks: [] }] },
      { name: "none_yet", type: "conditional", conditions: [] }
    ]);
  });

  it("keeps loop values exactly: an empty string or null item is an iteration", () => {
    const tasks = [
      {
        name: "each",
        type: "loop_sequential",
        values: ["a", "", null],
        tasks: [{ name: "x", type: "formatter", template: "{{ value }}" }]
      }
    ];
    expect(cleanAutomationFormData({ tasks }).tasks?.[0].values).toEqual(["a", "", null]);
  });

  it('keeps variable declarations exactly: `default: ""` is a default', () => {
    const variables = { region: { type: "string", default: "" }, ids: { default: [] } };
    expect(cleanAutomationFormData({ tasks: [], variables }).variables).toEqual(variables);
  });

  it("keeps a SQL task's variable overrides exactly, an empty one included", () => {
    const tasks = [
      { name: "q", type: "execute_sql", database: "db", sql_query: "x", variables: { a: "" } }
    ];
    expect(cleanAutomationFormData({ tasks }).tasks?.[0].variables).toEqual({ a: "" });
  });

  it("keeps a semantic filter's `value: null` (IS NULL) and its value lists", () => {
    const filters = [
      { field: "orders.coupon", op: "eq", value: null },
      { field: "orders.status", op: "in", values: ["", "open"] }
    ];
    const tasks = [{ name: "q", type: "semantic_query", topic: "orders", filters }];
    expect(cleanAutomationFormData({ tasks }).tasks?.[0].filters).toEqual(filters);
  });

  it("keeps Looker and Omni `fields` when empty", () => {
    const tasks = [{ name: "q", type: "looker_query", integration: "l", fields: [] }];
    expect(cleanAutomationFormData({ tasks }).tasks?.[0].fields).toEqual([]);
  });

  it("still strips what the form leaves empty where empty and missing are the same", () => {
    expect(
      cleanAutomationFormData({
        name: "a",
        description: "",
        tests: [],
        retrieval: { include: [""], exclude: [] },
        tasks: [{ name: "t", type: "execute_sql", sql_query: "x", sql_file: "", cache: {} }]
      })
    ).toEqual({ name: "a", tasks: [{ name: "t", type: "execute_sql", sql_query: "x" }] });
  });
});
