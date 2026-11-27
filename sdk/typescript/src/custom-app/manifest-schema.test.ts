/**
 * The checked-in `schema/oxy-app.schema.json` is derived from `manifest-types.ts`
 * and nothing else: regenerating it produces no diff, and it uses only the
 * keywords the walker in `manifest-validate.ts` understands.
 */

import { readFileSync } from "node:fs";
import { beforeAll, describe, expect, it } from "vitest";
import { renderManifestSchema, SCHEMA_PATH } from "../../scripts/gen-manifest-schema.mjs";
import {
  FUNCTION_NAME_PATTERN,
  SCREEN_PATH_MAX_LEN,
  SCREEN_PATH_PATTERN,
  SCREENS_MAX,
  SLUG_MAX_LEN,
  SLUG_PATTERN
} from "./manifest-rules";
import { OXY_APP_SCHEMA, OXY_APP_SCHEMA_URL } from "./manifest-schema";

/**
 * Every keyword `manifest-validate.ts` acts on, plus the annotations it reads
 * past. A generator emitting anything else (a `oneOf`, a `format`) would ship
 * a constraint nothing enforces, which is the silent kind of drift.
 */
const WALKED = new Set([
  "$ref",
  "type",
  "properties",
  "required",
  "additionalProperties",
  "propertyNames",
  "items",
  "maxItems",
  "uniqueItems",
  "enum",
  "const",
  "anyOf",
  "pattern",
  "minLength",
  "maxLength",
  "minimum",
  "maximum"
]);
const ANNOTATIONS = new Set(["$schema", "$id", "title", "description", "definitions"]);

function keywordsIn(node: unknown, into: Set<string>): void {
  if (Array.isArray(node)) {
    for (const item of node) keywordsIn(item, into);
    return;
  }
  if (!node || typeof node !== "object") return;
  for (const [key, value] of Object.entries(node)) {
    into.add(key);
    if (key === "properties" || key === "definitions") {
      for (const sub of Object.values(value as Record<string, unknown>)) keywordsIn(sub, into);
    } else if (!["enum", "const", "required"].includes(key)) {
      keywordsIn(value, into);
    }
  }
}

describe("oxy-app.schema.json", () => {
  // Generated once: the generator builds a TypeScript program, which is
  // seconds, not milliseconds.
  let fresh = "";
  beforeAll(() => {
    fresh = renderManifestSchema();
  }, 120_000);

  // Compared as documents, not bytes: Biome formats the checked-in JSON at
  // commit (`pnpm gen:manifest-schema` runs it too), and a wrapped array is
  // the same schema.
  it("is what the type generates — regenerate with `pnpm gen:manifest-schema`", () => {
    expect(JSON.parse(readFileSync(SCHEMA_PATH, "utf8"))).toEqual(JSON.parse(fresh));
  });

  it("is the document the SDK ships", () => {
    expect(JSON.parse(fresh)).toEqual(OXY_APP_SCHEMA);
    expect(OXY_APP_SCHEMA.$id).toBe(OXY_APP_SCHEMA_URL);
  });

  it("uses only keywords the validator walks", () => {
    const used = new Set<string>();
    keywordsIn(OXY_APP_SCHEMA, used);
    const unknown = [...used].filter((k) => !WALKED.has(k) && !ANNOTATIONS.has(k)).sort();
    expect(unknown).toEqual([]);
  });

  it("carries the two name rules the server enforces", () => {
    const root = OXY_APP_SCHEMA.definitions.OxyAppManifest;
    expect(root?.properties?.slug).toMatchObject({
      type: "string",
      minLength: 1,
      maxLength: SLUG_MAX_LEN,
      pattern: SLUG_PATTERN.source
    });
    expect(root?.properties?.functions?.propertyNames).toEqual({
      pattern: FUNCTION_NAME_PATTERN.source
    });
  });

  it("carries the screen-path rule on `checks.screens`", () => {
    const root = OXY_APP_SCHEMA.definitions.OxyAppManifest;
    expect(root?.properties?.checks?.$ref).toBe("#/definitions/OxyAppChecksManifest");
    expect(root?.required).not.toContain("checks");
    const checks = OXY_APP_SCHEMA.definitions.OxyAppChecksManifest;
    expect(checks?.additionalProperties).toBe(false);
    expect(checks?.required).toBeUndefined();
    expect(checks?.properties?.screens).toMatchObject({
      type: "array",
      maxItems: SCREENS_MAX,
      uniqueItems: true,
      items: {
        type: "string",
        maxLength: SCREEN_PATH_MAX_LEN,
        pattern: SCREEN_PATH_PATTERN.source
      }
    });
  });

  it("names every field the server honours", () => {
    const fn = OXY_APP_SCHEMA.definitions.OxyAppFunctionManifest?.properties ?? {};
    for (const field of [
      "rateLimitPerMinute",
      "fetch",
      "storage",
      "timeoutSeconds",
      "webhook",
      "on",
      "events",
      "callable"
    ]) {
      expect(fn[field], field).toBeDefined();
    }
    const app = OXY_APP_SCHEMA.definitions.OxyAppManifest?.properties ?? {};
    for (const field of [
      "art",
      "icon",
      "status",
      "description",
      "build",
      "environments",
      "events",
      "calls",
      "checks",
      "$schema"
    ]) {
      expect(app[field], field).toBeDefined();
    }
  });
});
