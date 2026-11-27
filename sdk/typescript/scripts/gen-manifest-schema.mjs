#!/usr/bin/env node
// gen-manifest-schema: `schema/oxy-app.schema.json` from the SDK's `OxyAppManifest` type.
//
// ONE DEFINITION. The type in `src/custom-app/manifest-types.ts` is the contract;
// this derives the JSON Schema from it, so an editor completing `oxy-app.json`,
// `oxyc validate`, `oxyc publish`, the Vite plugin and the SDK's own loader all
// read the same fields with the same constraints. `manifest-schema.test.ts`
// regenerates and fails on a diff, so the checked-in copy cannot go stale.
//
// The two name rules the server enforces (`manifest-rules.ts`) are written into
// the schema here rather than as JSDoc tags on the type, so the regex is spelled
// once: `slug.pattern` and `functions.propertyNames.pattern`. The screen-path
// rule is written the same way, onto `checks.screens`.
//
//   node sdk/typescript/scripts/gen-manifest-schema.mjs          write it
//   node sdk/typescript/scripts/gen-manifest-schema.mjs --check  exit 1 if stale

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { createGenerator } from "ts-json-schema-generator";
import {
  FUNCTION_NAME_PATTERN,
  SCREEN_PATH_MAX_LEN,
  SCREEN_PATH_PATTERN,
  SCREENS_MAX,
  SLUG_MAX_LEN,
  SLUG_PATTERN
} from "../src/custom-app/manifest-rules.ts";

const PKG = join(dirname(fileURLToPath(import.meta.url)), "..");

/** Where the schema is checked in, and shipped (`files` in package.json). */
export const SCHEMA_PATH = join(PKG, "schema", "oxy-app.schema.json");

/**
 * The published copy — what a manifest's `$schema` points at. unpkg serves the
 * package's files verbatim, so this tracks the latest release; an app that
 * wants the installed version's exact copy points at
 * `./node_modules/@oxy-hq/sdk/schema/oxy-app.schema.json` instead.
 */
export const SCHEMA_ID = "https://unpkg.com/@oxy-hq/sdk/schema/oxy-app.schema.json";

/** The schema as an object, regenerated from the type on every call. */
export function generateManifestSchema() {
  const generated = createGenerator({
    path: join(PKG, "src", "custom-app", "manifest-types.ts"),
    tsconfig: join(PKG, "tsconfig.json"),
    type: "OxyAppManifest",
    expose: "export",
    topRef: true,
    jsDoc: "extended",
    additionalProperties: false,
    skipTypeCheck: true,
    sortProps: true,
    strictTuples: true,
    encodeRefs: false
  }).createSchema("OxyAppManifest");

  const root = generated.definitions?.OxyAppManifest;
  if (!root || typeof root !== "object" || !root.properties) {
    throw new Error("the generator produced no OxyAppManifest definition");
  }
  root.properties.slug = {
    ...root.properties.slug,
    type: "string",
    minLength: 1,
    maxLength: SLUG_MAX_LEN,
    pattern: SLUG_PATTERN.source
  };
  root.properties.functions = {
    ...root.properties.functions,
    propertyNames: { pattern: FUNCTION_NAME_PATTERN.source }
  };

  // `calls`: slug keys, lists of function names. The rest (non-empty, no
  // duplicates, 64 pairs in total) is `callsRule` in manifest-validate.ts: the
  // schema validator walks no `minItems` / `maxProperties`, and the rule says a
  // duplicate in its own words.
  root.properties.calls = {
    ...root.properties.calls,
    propertyNames: { pattern: SLUG_PATTERN.source, maxLength: SLUG_MAX_LEN },
    additionalProperties: {
      type: "array",
      items: { type: "string", pattern: FUNCTION_NAME_PATTERN.source }
    }
  };

  // `checks.screens`: app-relative paths, each once, at most `SCREENS_MAX`.
  const checks = generated.definitions?.OxyAppChecksManifest;
  if (!checks || typeof checks !== "object" || !checks.properties?.screens) {
    throw new Error("the generator produced no OxyAppChecksManifest.screens");
  }
  checks.properties.screens = {
    ...checks.properties.screens,
    maxItems: SCREENS_MAX,
    uniqueItems: true,
    items: {
      type: "string",
      maxLength: SCREEN_PATH_MAX_LEN,
      pattern: SCREEN_PATH_PATTERN.source
    }
  };

  return {
    $schema: generated.$schema,
    $id: SCHEMA_ID,
    title: "oxy-app.json",
    description:
      "The manifest of an Oxy custom app. Generated from @oxy-hq/sdk's OxyAppManifest type by scripts/gen-manifest-schema.mjs — edit the type, not this file.",
    $ref: generated.$ref,
    definitions: generated.definitions
  };
}

/** The bytes the file holds when it is current. */
export function renderManifestSchema() {
  return `${JSON.stringify(generateManifestSchema(), null, 2)}\n`;
}

/** The same document, whatever the formatting: Biome wraps the file at commit. */
function sameDocument(a, b) {
  if (a === null || b === null) return false;
  try {
    return JSON.stringify(JSON.parse(a)) === JSON.stringify(JSON.parse(b));
  } catch {
    return false;
  }
}

function main() {
  const check = process.argv.includes("--check");
  const fresh = renderManifestSchema();
  let current = null;
  try {
    current = readFileSync(SCHEMA_PATH, "utf8");
  } catch {
    // Absent: written below, or stale under --check.
  }
  const rel = relative(process.cwd(), SCHEMA_PATH);
  if (sameDocument(current, fresh)) {
    console.log(`${rel} is current`);
    return;
  }
  if (check) {
    console.error(`stale: ${rel} — run \`node sdk/typescript/scripts/gen-manifest-schema.mjs\``);
    process.exit(1);
  }
  writeFileSync(SCHEMA_PATH, fresh);
  console.log(`wrote ${rel}`);
}

const invokedDirectly =
  process.argv[1] !== undefined && import.meta.url === new URL(`file://${process.argv[1]}`).href;
if (invokedDirectly) main();
