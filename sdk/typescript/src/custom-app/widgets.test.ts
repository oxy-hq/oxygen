import { describe, expect, it } from "vitest";
import { validateOxyAppManifest } from "./manifest-validate";
import { checkWidgets, MAX_WIDGET_ACTIONS, MAX_WIDGETS, type WidgetCheckId } from "./widgets";

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
    expect(checks({ close: { title: "A", function: "close-widget", size: "huge" } })).toEqual([
      "widget_invalid"
    ]);
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
    const four: Record<string, unknown> = {};
    for (const id of ["a", "b", "c", "d"]) four[id] = { title: id, function: "other-fn" };
    expect(MAX_WIDGETS).toBe(3);
    expect(checks(four)).toEqual(["widget_too_many"]);
    delete four.d;
    expect(checks(four)).toEqual([]);
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
      "widget_invalid"
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
