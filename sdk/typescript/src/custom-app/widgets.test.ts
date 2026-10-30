import { describe, expect, it } from "vitest";
import { validateOxyAppManifest } from "./manifest-validate";
import {
  checkWidgets,
  MAX_WIDGET_ACTIONS,
  MAX_WIDGET_PARAMS,
  MAX_WIDGET_REFRESH_AFTER_SECONDS,
  MAX_WIDGET_ROWS,
  MAX_WIDGETS,
  MIN_WIDGET_REFRESH_AFTER_SECONDS,
  type WidgetCheckId,
  type WidgetInput,
  type WidgetView
} from "./widgets";

const functions = { "close-widget": {}, "post-ready-jes": {}, "other-fn": {} };

function manifest(widgets: unknown, fns: unknown = functions) {
  return { schemaVersion: 2, slug: "x", functions: fns, widgets };
}

const checks = (widgets: unknown): WidgetCheckId[] =>
  checkWidgets(manifest(widgets)).map((f) => f.check);

const valid = {
  close: {
    title: "Month-end close",
    function: "close-widget",
    size: "wide",
    sizes: ["regular", "wide", "full"],
    actions: { "post-ready": { function: "post-ready-jes", confirm: true, destructive: false } }
  }
};

describe("checkWidgets", () => {
  it("says nothing for a valid block, an absent block, or a non-manifest", () => {
    expect(checkWidgets(manifest(valid))).toEqual([]);
    expect(checkWidgets({ schemaVersion: 2, slug: "x" })).toEqual([]);
    expect(checkWidgets(null)).toEqual([]);
    expect(checkWidgets("nope")).toEqual([]);
    expect(checkWidgets(manifest({ a: { title: "A", function: "other-fn" } }))).toEqual([]);
  });

  it("widget_invalid: a block, widget, title, size, id or key that is wrong", () => {
    expect(checks([])).toEqual(["widget_invalid"]);
    expect(checks({ close: "x" })).toEqual(["widget_invalid"]);
    expect(checks({ close: { function: "close-widget" } })).toEqual(["widget_invalid"]);
    expect(checks({ close: { title: "  ", function: "close-widget" } })).toEqual([
      "widget_invalid"
    ]);
    expect(checks({ close: { title: "x".repeat(41), function: "close-widget" } })).toEqual([
      "widget_invalid"
    ]);
    expect(checks({ close: { title: "x".repeat(40), function: "close-widget" } })).toEqual([]);
    expect(checks({ close: { title: "A" } })).toEqual(["widget_invalid"]);
    expect(checks({ Close: { title: "A", function: "close-widget" } })).toEqual(["widget_invalid"]);
    expect(checks({ close: { title: "A", function: "close-widget", actions: [] } })).toEqual([
      "widget_invalid"
    ]);
    const unknown = checkWidgets(
      manifest({ close: { title: "A", function: "close-widget", colour: "red" } })
    );
    expect(unknown).toEqual([
      expect.objectContaining({ check: "widget_invalid", path: "widgets.close.colour" })
    ]);
  });

  it("widget_too_many: more than the limit", () => {
    const seven: Record<string, unknown> = {};
    for (const id of ["a", "b", "c", "d", "e", "f", "g"]) {
      seven[id] = { title: id, function: "other-fn" };
    }
    expect(MAX_WIDGETS).toBe(6);
    expect(checks(seven)).toEqual(["widget_too_many"]);
    delete seven.g;
    expect(checks(seven)).toEqual([]);
  });

  describe("widget_size_invalid", () => {
    const sized = (extra: Record<string, unknown>) =>
      checkWidgets(manifest({ w: { title: "A", function: "close-widget", ...extra } }));
    const sizeChecks = (extra: Record<string, unknown>) => sized(extra).map((f) => f.check);

    it("accepts every size, and sizes that include the default", () => {
      for (const size of ["regular", "wide", "full"]) expect(sizeChecks({ size })).toEqual([]);
      expect(sizeChecks({ sizes: ["regular", "wide", "full"] })).toEqual([]);
      expect(sizeChecks({ size: "wide", sizes: ["wide", "full"] })).toEqual([]);
      expect(sizeChecks({ size: "full", sizes: ["full"] })).toEqual([]);
      // The default size is "regular" when `size` is absent.
      expect(sizeChecks({ sizes: ["regular", "full"] })).toEqual([]);
    });

    it("refuses a size that is not regular, wide or full", () => {
      expect(sized({ size: "huge" })).toEqual([
        expect.objectContaining({ check: "widget_size_invalid", path: "widgets.w.size" })
      ]);
      expect(sizeChecks({ size: 2 })).toEqual(["widget_size_invalid"]);
      expect(sizeChecks({ size: "tall" })).toEqual(["widget_size_invalid"]);
    });

    it("refuses sizes that are empty, not a list, unknown or duplicated", () => {
      expect(sizeChecks({ sizes: [] })).toEqual(["widget_size_invalid"]);
      expect(sizeChecks({ sizes: "wide" })).toEqual(["widget_size_invalid"]);
      expect(sized({ sizes: ["regular", "tall"] })).toEqual([
        expect.objectContaining({ check: "widget_size_invalid", path: "widgets.w.sizes[1]" })
      ]);
      expect(sized({ sizes: ["regular", "wide", "regular"] })).toEqual([
        expect.objectContaining({ check: "widget_size_invalid", path: "widgets.w.sizes[2]" })
      ]);
    });

    it("refuses sizes that leave out the default size", () => {
      expect(sized({ sizes: ["wide", "full"] })).toEqual([
        expect.objectContaining({ check: "widget_size_invalid", path: "widgets.w.sizes" })
      ]);
      expect(sizeChecks({ size: "wide", sizes: ["regular"] })).toEqual(["widget_size_invalid"]);
    });
  });

  describe("widget_params_invalid", () => {
    const withParams = (params: unknown) =>
      checkWidgets(manifest({ w: { title: "A", function: "close-widget", params } }));
    const paths = (params: unknown) => withParams(params).map((f) => f.path);
    const props = (properties: unknown, extra: Record<string, unknown> = {}) => ({
      type: "object",
      properties,
      ...extra
    });
    const prop = (extra: Record<string, unknown>) => props({ store: { type: "string", ...extra } });
    const at = "widgets.w.params.properties.store";

    it("accepts every type and key, and a manifest with no params", () => {
      expect(
        withParams(
          props(
            {
              store: {
                type: "string",
                title: "Store",
                description: "A slug",
                minLength: 1,
                maxLength: 20
              },
              late_after: { type: "integer", minimum: 0, maximum: 240, default: 60 },
              ratio: { type: "number", minimum: 0.5, default: 1.5 },
              flag: { type: "boolean", default: false },
              vendor: { type: "string", enum: ["a", "b"], default: "a" },
              tier: { type: "integer", enum: [1, 2, 3] }
            },
            { required: ["store"] }
          )
        )
      ).toEqual([]);
      expect(withParams(undefined)).toEqual([]);
      expect(withParams(props({}))).toEqual([]);
      expect(MAX_WIDGET_PARAMS).toBe(8);
    });

    it("needs an object with type object and properties", () => {
      for (const bad of [[], "x", null, 3]) {
        expect(paths(bad)).toEqual(["widgets.w.params"]);
      }
      expect(paths({ properties: {} })).toEqual(["widgets.w.params.type"]);
      expect(paths({ type: "array", properties: {} })).toEqual(["widgets.w.params.type"]);
      expect(paths({ type: "object" })).toEqual(["widgets.w.params.properties"]);
      expect(paths({ type: "object", properties: [] })).toEqual(["widgets.w.params.properties"]);
      expect(withParams({ type: "object", properties: {} }).map((f) => f.check)).toEqual([]);
    });

    it("refuses more than 8 settings", () => {
      const nine: Record<string, unknown> = {};
      for (let i = 0; i < 9; i++) nine[`p${i}`] = { type: "string" };
      expect(withParams(props(nine))).toEqual([
        expect.objectContaining({
          check: "widget_params_invalid",
          path: "widgets.w.params.properties"
        })
      ]);
      delete nine.p8;
      expect(withParams(props(nine))).toEqual([]);
    });

    it("refuses a setting name that is not a segment, and a setting that is not an object", () => {
      expect(paths(props({ Store: { type: "string" } }))).toEqual([
        "widgets.w.params.properties.Store"
      ]);
      expect(paths(props({ "a b": { type: "string" } }))).toEqual([
        "widgets.w.params.properties.a b"
      ]);
      expect(paths(props({ store: "string" }))).toEqual([at]);
      expect(paths(props({ store: null }))).toEqual([at]);
    });

    it("refuses a type that is not string, integer, number or boolean", () => {
      for (const type of ["object", "array", "null", 1, undefined]) {
        expect(paths(props({ store: { type } }))).toEqual([`${at}.type`]);
      }
      expect(paths(props({ store: {} }))).toEqual([`${at}.type`]);
    });

    it("refuses unknown keys at both levels", () => {
      expect(paths(prop({ pattern: "x" }))).toEqual([`${at}.pattern`]);
      expect(paths(prop({ $ref: "#/x" }))).toEqual([`${at}.$ref`]);
      expect(paths(props({}, { additionalProperties: false }))).toEqual([
        "widgets.w.params.additionalProperties"
      ]);
    });

    it("checks title and description are text", () => {
      expect(paths(prop({ title: 1, description: {} }))).toEqual([
        `${at}.title`,
        `${at}.description`
      ]);
    });

    it("enum: a non-empty list of values of the setting's type", () => {
      expect(paths(prop({ enum: [] }))).toEqual([`${at}.enum`]);
      expect(paths(prop({ enum: "a" }))).toEqual([`${at}.enum`]);
      expect(paths(prop({ enum: ["a", 2, "c", true] }))).toEqual([
        `${at}.enum[1]`,
        `${at}.enum[3]`
      ]);
      expect(paths(props({ n: { type: "integer", enum: [1, 1.5] } }))).toEqual([
        "widgets.w.params.properties.n.enum[1]"
      ]);
      expect(paths(props({ n: { type: "boolean", enum: [true] } }))).toEqual([]);
    });

    it("default: matches the type, and the enum when there is one", () => {
      expect(paths(prop({ default: 1 }))).toEqual([`${at}.default`]);
      expect(paths(props({ n: { type: "integer", default: 1.5 } }))).toEqual([
        "widgets.w.params.properties.n.default"
      ]);
      expect(paths(props({ n: { type: "number", default: "1" } }))).toEqual([
        "widgets.w.params.properties.n.default"
      ]);
      expect(paths(props({ n: { type: "boolean", default: "true" } }))).toEqual([
        "widgets.w.params.properties.n.default"
      ]);
      expect(paths(prop({ enum: ["a", "b"], default: "c" }))).toEqual([`${at}.default`]);
      expect(paths(prop({ enum: ["a", "b"], default: "b" }))).toEqual([]);
    });

    it("minimum and maximum are for numbers; minLength and maxLength for strings", () => {
      expect(paths(prop({ minimum: 0 }))).toEqual([`${at}.minimum`]);
      expect(paths(prop({ maximum: 5 }))).toEqual([`${at}.maximum`]);
      expect(paths(props({ n: { type: "integer", minLength: 1 } }))).toEqual([
        "widgets.w.params.properties.n.minLength"
      ]);
      expect(paths(props({ b: { type: "boolean", maxLength: 1, minimum: 0 } }))).toEqual([
        "widgets.w.params.properties.b.minimum",
        "widgets.w.params.properties.b.maxLength"
      ]);
      expect(paths(prop({ minLength: -1 }))).toEqual([`${at}.minLength`]);
      expect(paths(prop({ maxLength: 1.5 }))).toEqual([`${at}.maxLength`]);
      expect(paths(props({ n: { type: "number", minimum: "0" } }))).toEqual([
        "widgets.w.params.properties.n.minimum"
      ]);
    });

    it("required: each entry names a declared setting", () => {
      const p = { store: { type: "string" } };
      expect(paths(props(p, { required: ["store"] }))).toEqual([]);
      expect(paths(props(p, { required: ["store", "vendor"] }))).toEqual([
        "widgets.w.params.required[1]"
      ]);
      expect(paths(props(p, { required: "store" }))).toEqual(["widgets.w.params.required"]);
      expect(paths(props(p, { required: [1] }))).toEqual(["widgets.w.params.required[0]"]);
      expect(paths(props(p, { required: ["constructor"] }))).toEqual([
        "widgets.w.params.required[0]"
      ]);
    });

    it("reports every problem at once", () => {
      const found = withParams(
        props(
          {
            Bad: { type: "string" },
            n: { type: "integer", default: "x", minLength: 1 },
            s: { type: "date" }
          },
          { required: ["ghost"], extra: 1 }
        )
      );
      expect(found.every((f) => f.check === "widget_params_invalid")).toBe(true);
      expect(found.map((f) => f.path).sort()).toEqual(
        [
          "widgets.w.params.extra",
          "widgets.w.params.properties.Bad",
          "widgets.w.params.properties.n.default",
          "widgets.w.params.properties.n.minLength",
          "widgets.w.params.properties.s.type",
          "widgets.w.params.required[0]"
        ].sort()
      );
    });
  });

  it("widget_function_undeclared: a function the manifest does not declare", () => {
    const found = checkWidgets(manifest({ close: { title: "A", function: "ghost" } }));
    expect(found).toEqual([
      expect.objectContaining({
        level: "error",
        check: "widget_function_undeclared",
        path: "widgets.close.function"
      })
    ]);
    expect(checkWidgets(manifest({ close: { title: "A", function: "x" } }, undefined))).toEqual([
      expect.objectContaining({ check: "widget_function_undeclared" })
    ]);
    // A prototype name is not a declared function.
    expect(checks({ close: { title: "A", function: "constructor" } })).toEqual([
      "widget_function_undeclared"
    ]);
  });

  it("widget_actions_too_many: more than the limit", () => {
    const actions: Record<string, unknown> = {};
    for (const id of ["a", "b", "c", "d"]) actions[id] = { function: "other-fn" };
    expect(MAX_WIDGET_ACTIONS).toBe(3);
    expect(checks({ close: { title: "A", function: "close-widget", actions } })).toEqual([
      "widget_actions_too_many"
    ]);
  });

  it("widget_action_invalid: a bad id, shape, flag, key or missing function", () => {
    const bad = (actions: unknown) =>
      checks({ w: { title: "A", function: "close-widget", actions } });
    expect(bad({ "Bad Id": { function: "other-fn" } })).toEqual(["widget_action_invalid"]);
    expect(bad({ go: "x" })).toEqual(["widget_action_invalid"]);
    expect(bad({ go: {} })).toEqual(["widget_action_invalid"]);
    expect(bad({ go: { function: "other-fn", confirm: "yes" } })).toEqual([
      "widget_action_invalid"
    ]);
    expect(bad({ go: { function: "other-fn", destructive: 1 } })).toEqual([
      "widget_action_invalid"
    ]);
    expect(bad({ go: { function: "other-fn", label: "Go" } })).toEqual(["widget_action_invalid"]);
  });

  it("widget_action_function_undeclared: an action naming an undeclared function", () => {
    const found = checkWidgets(
      manifest({
        w: { title: "A", function: "close-widget", actions: { go: { function: "ghost" } } }
      })
    );
    expect(found).toEqual([
      expect.objectContaining({
        check: "widget_action_function_undeclared",
        path: "widgets.w.actions.go.function"
      })
    ]);
  });

  it("lists every problem at once", () => {
    const found = checkWidgets(
      manifest({
        a: { title: "", function: "ghost", size: "huge", extra: 1 },
        B: {}
      })
    );
    expect(found.map((f) => f.check).sort()).toEqual([
      "widget_function_undeclared",
      "widget_invalid",
      "widget_invalid",
      "widget_invalid",
      "widget_size_invalid"
    ]);
  });

  it("never throws, whatever the manifest holds", () => {
    for (const widgets of [
      null,
      1,
      "x",
      [],
      { a: null },
      { a: [] },
      { a: { actions: { b: null } } }
    ]) {
      expect(() => checkWidgets(manifest(widgets))).not.toThrow();
    }
  });
});

describe("the view types", () => {
  it("describe a KPI view and the input a widget function receives", () => {
    const input: WidgetInput = { widget: "close", size: "full", params: { store: "mission" } };
    const view: WidgetView = {
      headline: "19 stores ready to post",
      refresh_after: 300,
      metric: { value: "$48,210", label: "Posted", delta: "+12%", trend: "up", spark: [1, 2, 3] },
      rows: []
    };
    expect(input.size).toBe("full");
    expect(view.metric?.trend).toBe("up");
    expect(input.params.store).toBe("mission");
    expect(view.refresh_after).toBe(300);
    expect(MAX_WIDGET_ROWS).toBe(8);
    expect(MIN_WIDGET_REFRESH_AFTER_SECONDS).toBe(60);
    expect(MAX_WIDGET_REFRESH_AFTER_SECONDS).toBe(86400);
  });
});

describe("validateOxyAppManifest with widgets", () => {
  it("accepts a valid widgets block with no issues", () => {
    expect(validateOxyAppManifest(manifest(valid))).toEqual([]);
  });

  it("reports the widget checks as errors at their paths", () => {
    const issues = validateOxyAppManifest(
      manifest({ close: { title: "A", function: "ghost", colour: "red" } })
    );
    expect(issues.filter((i) => i.level === "error").map((i) => i.path)).toEqual(
      expect.arrayContaining(["widgets.close.function", "widgets.close.colour"])
    );
  });
});
