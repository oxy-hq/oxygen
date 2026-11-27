// Checks that dist/ holds each module format where package.json says it is. `pnpm build` runs it
// after tsdown, so a build that would publish a broken pair fails instead:
//
//   - every dist/*.cjs compiles as CommonJS (nothing in it is run): no ESM `import` or `export`
//     hides in an entry or in a chunk;
//   - every subpath in package.json `exports` loads both ways, `require()` of its `require` file
//     and `import()` of its `import` file, and the two give the same export names.
//
// 2.35.0 shipped every .cjs entry as ESM (tsdown's declaration pass wrote over them; see
// tsdown.config.ts), so `require("@oxy-hq/sdk")` threw "Cannot use import statement outside a
// module". This is the check that would have stopped it.
//
//   node scripts/check-dist.mjs
//
// Plain Node, no dependencies.

import { readdirSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { compileFunction } from "node:vm";

const PKG = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DIST = join(PKG, "dist");
const require = createRequire(join(PKG, "package.json"));

/** The parameters of Node's CommonJS wrapper: inside it, `import` or `export` is a SyntaxError. */
const CJS_WRAPPER = ["exports", "require", "module", "__filename", "__dirname"];

/** The first line of an error, which is the line that says what is wrong. */
const firstLine = (e) => String(e?.message ?? e).split("\n")[0];

/** The .cjs files that do not compile as CommonJS, each with why. */
function nonCommonJs() {
  const bad = [];
  for (const name of readdirSync(DIST).filter((f) => f.endsWith(".cjs"))) {
    const file = join(DIST, name);
    try {
      compileFunction(readFileSync(file, "utf8"), CJS_WRAPPER, { filename: file });
    } catch (e) {
      bad.push(`dist/${name} is not CommonJS: ${firstLine(e)}`);
    }
  }
  return bad;
}

/** The subpaths of `exports` that have both a `require` and an `import` file. */
function dualEntries() {
  const { exports } = JSON.parse(readFileSync(join(PKG, "package.json"), "utf8"));
  return Object.entries(exports).flatMap(([subpath, target]) =>
    typeof target === "object" && target.require?.default && target.import?.default
      ? [{ subpath, cjs: target.require.default, esm: target.import.default }]
      : []
  );
}

/** What goes wrong loading one entry both ways, if anything. */
async function loadBothWays({ subpath, cjs, esm }) {
  let cjsNames;
  let esmNames;
  try {
    cjsNames = Object.keys(require(resolve(PKG, cjs))).sort();
  } catch (e) {
    return `${subpath}: require("${cjs}") fails: ${firstLine(e)}`;
  }
  try {
    esmNames = Object.keys(await import(pathToFileURL(resolve(PKG, esm)).href)).sort();
  } catch (e) {
    return `${subpath}: import("${esm}") fails: ${firstLine(e)}`;
  }
  const only = (a, b) => a.filter((n) => !b.includes(n));
  const cjsOnly = only(cjsNames, esmNames);
  const esmOnly = only(esmNames, cjsNames);
  if (cjsOnly.length || esmOnly.length)
    return `${subpath}: CommonJS and ESM export different names (only CommonJS: ${cjsOnly.join(", ") || "none"}; only ESM: ${esmOnly.join(", ") || "none"})`;
  return null;
}

const entries = dualEntries();
const problems = [...nonCommonJs()];
for (const entry of entries) {
  const problem = await loadBothWays(entry);
  if (problem) problems.push(problem);
}
if (problems.length) {
  for (const p of problems) console.error(`check-dist: ${p}`);
  console.error(`check-dist: ${problems.length} problem(s)`);
  process.exit(1);
}
console.log(`check-dist: ${entries.length} entries load as CommonJS and as ESM`);
