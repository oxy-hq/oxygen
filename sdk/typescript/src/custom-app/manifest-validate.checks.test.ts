/**
 * `checks.screens`: the screens an app declares for the promote checks, as
 * `validateOxyAppManifest` reads them. The loader, `oxyc validate`,
 * `oxyc publish` and the Vite plugin all run this one validator.
 */

import { describe, expect, it } from "vitest";
import { SCREEN_PATH_MAX_LEN, SCREEN_PATH_RULE, SCREENS_MAX } from "./manifest-rules";
import { type ManifestIssue, validateOxyAppManifest } from "./manifest-validate";

const BASE = { schemaVersion: 2, slug: "store-ops" } as const;

const withScreens = (screens: unknown): ManifestIssue[] =>
  validateOxyAppManifest({ ...BASE, checks: { screens } });

/** `n` distinct valid paths. */
const paths = (n: number): string[] => Array.from({ length: n }, (_, i) => `screen-${i}`);

describe("validateOxyAppManifest on checks.screens", () => {
  it("accepts declared screens, the root included", () => {
    expect(withScreens(["", "stores", "reorder/plans"])).toEqual([]);
    expect(withScreens(["stores/", "a.b/c_d-e", "stores/12:30", "1:2"])).toEqual([]);
  });

  it("accepts a screen the app picks by its query, beside the same path without one", () => {
    expect(withScreens(["", "?module=waste", "?module=temps", "stores?tab=open"])).toEqual([]);
  });

  it("accepts the longest path and the longest list", () => {
    expect(withScreens(["x".repeat(SCREEN_PATH_MAX_LEN)])).toEqual([]);
    expect(withScreens(paths(SCREENS_MAX))).toEqual([]);
  });

  it("leaves a manifest that declares none as it was", () => {
    expect(validateOxyAppManifest(BASE)).toEqual([]);
    expect(validateOxyAppManifest({ ...BASE, checks: {} })).toEqual([]);
    expect(withScreens([])).toEqual([]);
  });

  it("refuses an entry that is no app-relative path, naming it and the rule", () => {
    const refused: Array<[string, string]> = [
      ["a leading slash", "/stores"],
      ["a protocol-relative URL", "//example.test/stores"],
      ["an empty query", "stores?"],
      ["a query with a space", "stores?tab=open now"],
      ["a hash", "stores#top"],
      ["a hash route", "#/stores"],
      ["a hash after a query", "stores?tab=open#top"],
      ["a parent segment", "../admin"],
      ["a parent segment inside", "stores/../admin"],
      ["two dots inside a segment", "stores..old"],
      ["an http URL", "https://example.test/stores"],
      ["a mailto link", "mailto:ops@example.test"],
      ["a script URL", "javascript:alert(1)"],
      ["two dots after a line break", "stores\n.."]
    ];
    for (const [what, entry] of refused) {
      expect(withScreens(["", entry]), what).toEqual([
        {
          level: "error",
          path: "checks.screens.1",
          message: `\`checks.screens.1\` ${JSON.stringify(entry)} must be ${SCREEN_PATH_RULE}`
        }
      ]);
    }
  });

  it("refuses a path over the length cap", () => {
    expect(withScreens(["x".repeat(SCREEN_PATH_MAX_LEN + 1)])).toEqual([
      {
        level: "error",
        path: "checks.screens.0",
        message: `\`checks.screens.0\` must be at most ${SCREEN_PATH_MAX_LEN} characters`
      }
    ]);
  });

  it("refuses a duplicate, at the repeat", () => {
    expect(withScreens(["", "stores", "plans", "stores"])).toEqual([
      {
        level: "error",
        path: "checks.screens.3",
        message: '`checks.screens.3` repeats "stores": list each entry once'
      }
    ]);
    // The root twice is a duplicate too.
    expect(withScreens(["", ""]).map((i) => i.path)).toEqual(["checks.screens.1"]);
  });

  it("refuses more entries than the cap", () => {
    expect(withScreens(paths(SCREENS_MAX + 1))).toEqual([
      {
        level: "error",
        path: "checks.screens",
        message: `\`checks.screens\` must list at most ${SCREENS_MAX} entries (it lists ${SCREENS_MAX + 1})`
      }
    ]);
  });

  it("refuses an entry that is not a string", () => {
    for (const entry of [7, null, true, { path: "stores" }, ["stores"]]) {
      expect(withScreens(["stores", entry]), JSON.stringify(entry)).toEqual([
        {
          level: "error",
          path: "checks.screens.1",
          message: "`checks.screens.1` must be a string"
        }
      ]);
    }
  });

  it("refuses a `screens` that is no list and a `checks` that is no object", () => {
    expect(withScreens("stores")).toEqual([
      { level: "error", path: "checks.screens", message: "`checks.screens` must be an array" }
    ]);
    expect(validateOxyAppManifest({ ...BASE, checks: ["stores"] })).toEqual([
      { level: "error", path: "checks", message: "`checks` must be an object" }
    ]);
  });

  it("says every bad entry of one list, one error per entry", () => {
    const issues = withScreens(["/a", "ok", "ok", 3]);
    expect(issues.map((i) => [i.level, i.path])).toEqual([
      ["error", "checks.screens.0"],
      ["error", "checks.screens.3"],
      ["error", "checks.screens.2"]
    ]);
    // An entry that is both malformed and a repeat is told its shape, once.
    expect(withScreens(["/a", "/a"]).map((i) => i.message)).toEqual([
      `\`checks.screens.0\` "/a" must be ${SCREEN_PATH_RULE}`,
      `\`checks.screens.1\` "/a" must be ${SCREEN_PATH_RULE}`
    ]);
  });

  it("warns on a key `checks` does not know, as on any unknown field", () => {
    expect(validateOxyAppManifest({ ...BASE, checks: { screen: ["stores"] } })).toEqual([
      {
        level: "warning",
        path: "checks.screen",
        message:
          "unknown field `checks.screen`: the platform ignores what it does not read, so check the spelling against the schema"
      }
    ]);
  });
});
