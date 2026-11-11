#!/usr/bin/env node
/**
 * Check that the `@oxy-hq/cli` tarball carries what it needs to work.
 *
 * THE PUBLISHED PACKAGE IS NOT THE SOURCE TREE, and the difference is invisible
 * from a checkout: npm strips a file literally named `.gitignore` out of a
 * tarball. The workspace template `oxyc new` renders therefore ships as
 * `_gitignore` and is renamed at render time — and if that ever regresses, the
 * package builds, installs, tests green, and hands the first customer a repo
 * with no ignore rules, which commits `node_modules/` and every app's build
 * output on their first `git add -A`.
 *
 * Nothing else in the pipeline looks at the tarball, so this does. It is cheap:
 * `npm pack --dry-run` resolves the file list without writing anything.
 *
 * IT ALSO CHECKS WHAT IS INSIDE TWO OF THOSE FILES, because the file list is
 * not the whole story. This package is public and the repository it is built
 * from is not, and the build shipped both the other way round once: `sourcemap`
 * with no `sourcemapExcludeSources` embedded all 159 source files verbatim in
 * `dist/main.mjs.map`, so `npm pack` handed anyone the CLI's whole source, and
 * `minify: false` left every developer comment in the bundle, Rust paths and
 * `internal-docs/` filenames among them. Both are off now in
 * `sdk/cli/tsdown.config.ts` — a comment there explains why — but a comment is
 * weaker than a check, and flipping either back is one word.
 */

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const PKG = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "sdk", "cli");

/**
 * Files the package is useless without.
 *
 * `dist/main.mjs` is the binary. The template files are what `oxyc new`
 * renders. `.oxyc-managed` is the ownership manifest that decides what
 * `oxyc update --apply` may rewrite — without it every file falls to
 * `unmatched` and the sync silently does nothing.
 */
const REQUIRED = [
  "dist/main.mjs",
  "template/_gitignore",
  "template/.oxyc-managed",
  "template/package.json",
  "template/config.yml",
  "template/scripts/dev.sh",
  "template/.github/workflows/validate.yaml",
  "template/.github/workflows/publish.yaml",
  "skills/oxy-cli/SKILL.md",
  // `oxyc validate` is useless without these, and they are COPIED in by
  // `prebuild` rather than committed — so a broken copy step ships a command
  // that fails on first use with "no JSON Schemas are available".
  // EVERY schema `oxyc validate` maps a file kind to. A missing one does not
  // fail the command — it silently stops checking that whole kind — so the
  // list here has to match `SCHEMA_KINDS` in validate.ts, not a sample of it.
  "json-schemas/config.json",
  "json-schemas/agentic.json",
  "json-schemas/workflow.json",
  "json-schemas/agent-test.json"
];

/** A file whose presence means the rename regressed. */
const FORBIDDEN = ["template/.gitignore"];

function packedFiles() {
  const raw = execFileSync("npm", ["pack", "--dry-run", "--json"], {
    cwd: PKG,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"]
  });
  return JSON.parse(raw)[0].files.map((f) => f.path);
}

const files = packedFiles();
const problems = [];

for (const path of REQUIRED) {
  if (!files.includes(path)) problems.push(`missing from the tarball: ${path}`);
}
for (const path of FORBIDDEN) {
  if (files.includes(path)) {
    problems.push(
      `${path} is in the tarball — npm will strip it. Ship it as \`_gitignore\` and rename at render time.`
    );
  }
}

// ── What is INSIDE the bundle and its map ────────────────────────────────────
//
// CI builds immediately before running this, so `dist` is present. A missing
// bundle is itself worth reporting rather than skipping: it means this check
// ran against nothing.
const BUNDLE = join(PKG, "dist", "main.mjs");
const BUNDLE_MAP = `${BUNDLE}.map`;

/** Names that mean nothing to a reader who has the package and not the repo. */
const INTERNAL = [
  { what: "a Rust source path", re: /crates\/[a-z0-9/_-]+\.rs/g },
  { what: "an internal-docs file", re: /internal-docs\/[a-z0-9.-]+\.(?:md|mdx)/g },
  { what: "an internal authority name", re: /\b(?:Ring|Cap|Action)::[A-Za-z]+/g }
];

if (!existsSync(BUNDLE)) {
  problems.push(`dist/main.mjs is missing — build sdk/cli before this check, or it proves nothing`);
} else {
  const bundle = readFileSync(BUNDLE, "utf8");
  for (const { what, re } of INTERNAL) {
    const hits = [...new Set(bundle.match(re) ?? [])];
    if (hits.length > 0) {
      problems.push(
        `dist/main.mjs names ${what}: ${hits.slice(0, 3).join(", ")}` +
          `${hits.length > 3 ? ` (+${hits.length - 3} more)` : ""} — ` +
          `state the behaviour instead, or check \`comments\` in tsdown.config.ts`
      );
    }
  }
}

if (!existsSync(BUNDLE_MAP)) {
  problems.push("dist/main.mjs.map is missing — build sdk/cli before this check");
} else if (JSON.parse(readFileSync(BUNDLE_MAP, "utf8")).sourcesContent !== undefined) {
  problems.push(
    "dist/main.mjs.map carries `sourcesContent` — that publishes this repository's " +
      "source to npm. Set `sourcemapExcludeSources` in tsdown.config.ts; the map still " +
      "resolves a stack frame to a file and a line without it."
  );
}

// A template that lost most of its files still passes the checks above if the
// named ones survive, so compare the tarball against the TREE rather than
// against a number written down here. A hardcoded threshold goes stale the
// next time somebody adds a template file, and a hardcoded message
// ("it shipped 20") goes stale one commit sooner than that.
const onDisk = readdirSync(join(PKG, "template"), { recursive: true, withFileTypes: true })
  .filter((e) => e.isFile())
  .map((e) => `${relative(PKG, join(e.parentPath ?? e.path, e.name))}`.split(sep).join("/"));

const missing = onDisk.filter((f) => !files.includes(f));
if (missing.length > 0) {
  problems.push(
    `${missing.length} template file(s) are in the tree but not the tarball: ${missing.slice(0, 5).join(", ")}` +
      (missing.length > 5 ? ` (+${missing.length - 5} more)` : "")
  );
}

if (problems.length > 0) {
  console.error("@oxy-hq/cli would publish broken:\n");
  for (const p of problems) console.error(`  - ${p}`);
  console.error(
    `\n${files.length} files in the tarball. Run \`npm pack --dry-run\` in sdk/cli to see them.`
  );
  process.exit(1);
}

console.log(
  `@oxy-hq/cli tarball ok — ${files.length} files, all ${onDisk.length} template files present.`
);
