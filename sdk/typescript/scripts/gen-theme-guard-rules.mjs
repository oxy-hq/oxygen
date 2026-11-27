#!/usr/bin/env node
// gen-theme-guard-rules: `src/testing/themeGuardSdkRules.gen.json`, the rules the SDK's own
// stylesheets declare, for the theme guard's reading of BUILT CSS
// (`checkThemeCss(sources, { built: true })`, src/testing/themeGuard.ts).
//
// An app's built CSS carries the SDK's CSS inside it, so the guard has to know which rules are
// the SDK's own: each prelude, and the properties the SDK declares under it. It cannot read the
// stylesheets when it runs: `oxyc` bundles the guard into one file, where a path relative to
// the module leads nowhere. So the reading is generated here and checked in, and
// `themeGuardBuilt.test.ts` regenerates it and fails on a difference, so the checked-in copy
// cannot go stale.
//
// The scanner is the guard's own (`src/testing/themeGuardScan.ts`, loaded as it is), so a
// prelude is keyed here exactly as the guard keys the one it meets.
//
//   node scripts/gen-theme-guard-rules.mjs          write it
//   node scripts/gen-theme-guard-rules.mjs --check  write nothing; exit 1 if it is stale
//
// `pnpm gen:theme-guard-rules` runs it and then Biome, which wraps the file.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { collectSdkRules } from "../src/testing/themeGuardScan.ts";
import { uiCssSources } from "./build-ui-css.mjs";

const PKG = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** Where the rules are checked in. */
export const SDK_RULES_PATH = join(PKG, "src", "testing", "themeGuardSdkRules.gen.json");

/**
 * The stylesheets an app's bundle can carry, relative to the package: `@oxy-hq/sdk/theme.css`,
 * `motion.css`, `shell.css`, and the sources `build-ui-css.mjs` assembles `ui.css` from.
 */
export function sdkStylesheets() {
  return [
    "src/theme/theme.css",
    "src/motion/motion.css",
    "src/shell/shell.css",
    ...uiCssSources().map((rel) => `src/ui/${rel}`)
  ];
}

/** The rules as an object, read from the stylesheets on every call. */
export function generateSdkRules(root = PKG) {
  return collectSdkRules(sdkStylesheets().map((rel) => readFileSync(join(root, rel), "utf8")));
}

/** The bytes the file holds before Biome wraps it. */
export function renderSdkRules(root = PKG) {
  return `${JSON.stringify(generateSdkRules(root), null, 2)}\n`;
}

/** The same document, whatever the formatting: Biome wraps the file. */
function sameDocument(a, b) {
  if (a === null || b === null) return false;
  try {
    return JSON.stringify(JSON.parse(a)) === JSON.stringify(JSON.parse(b));
  } catch {
    return false;
  }
}

function main(argv) {
  const fresh = renderSdkRules();
  let current = null;
  try {
    current = readFileSync(SDK_RULES_PATH, "utf8");
  } catch {
    // Absent: written below, or stale under --check.
  }
  const rel = relative(process.cwd(), SDK_RULES_PATH);
  if (sameDocument(current, fresh)) {
    console.log(`${rel} is current`);
    return 0;
  }
  if (argv.includes("--check")) {
    console.error(`stale: ${rel}; run \`pnpm --filter @oxy-hq/sdk gen:theme-guard-rules\``);
    return 1;
  }
  writeFileSync(SDK_RULES_PATH, fresh);
  console.log(`wrote ${rel}`);
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.exitCode = main(process.argv.slice(2));
}
