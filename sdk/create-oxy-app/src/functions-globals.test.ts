// A template's `functions/` typechecks against the isolate's globals alone.
//
// `functions/tsconfig.json` carries no DOM lib and no `@types`, so the globals a
// function may name are ES2022's plus what `functions/oxy-globals.d.ts` declares.
// The `functions` template used to pass `tsc -p functions/tsconfig.json` only
// because its `notify.ts` imports `@oxy-hq/sdk/email`, whose preact types carry
// `/// <reference lib="dom" />`; an app whose functions send no email failed with
// `Cannot find name 'Response'`. So the probes here import nothing: what they
// compile against is exactly the template's own declarations.
//
// The second probe is the other half: a global the isolate does not have
// (`TextEncoder`), and a fetch-API member its `Response` does not have (`text()`),
// must stay errors, or the first probe proves nothing. The third is the template
// as it ships, where an import does bring the DOM lib in. The isolate's `Response`
// and the DOM's disagree, which the template's `skipLibCheck` keeps out of the
// result; with that setting off, this probe fails.

import { spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, describe, expect, test } from "vitest";

const TEMPLATES = resolve(import.meta.dirname, "../templates");

// This package's own compiler, the one its `typecheck` runs. `typescript/bin/tsc`
// is not in the package's exports, so it is found through `package.json#bin`.
const tsPkg = createRequire(import.meta.url).resolve("typescript/package.json");
const TSC = join(dirname(tsPkg), JSON.parse(readFileSync(tsPkg, "utf8")).bin.tsc);

const withFunctions = readdirSync(TEMPLATES, { withFileTypes: true })
  .filter((e) => e.isDirectory() && existsSync(join(TEMPLATES, e.name, "functions")))
  .map((e) => e.name)
  .sort();

// Everything the isolate's bootstrap installs (`Response`, `btoa`, `atob`,
// `console`), used the way a function uses it.
const USES_THE_ISOLATE = `export default async function probe(): Promise<Response> {
  console.log("probe", { n: 1 });
  const token = btoa("oxy");
  if (atob(token) !== "oxy") return new Response("mismatch", { status: 500 });
  return Response.json({ token }, { status: 201, headers: { "x-probe": "1" } });
}
`;

const USES_WHAT_IT_LACKS = `export default async function probe(): Promise<string> {
  const bytes = new TextEncoder().encode("oxy");
  return Response.json({ n: bytes.length }).text();
}
`;

let root: string | undefined;
afterEach(() => {
  if (root) rmSync(root, { recursive: true, force: true });
  root = undefined;
});

/** `tsc -p functions/tsconfig.json` over the template's functions config and one probe. */
function typecheck(template: string, probe: string): { status: number | null; out: string } {
  root = mkdtempSync(join(tmpdir(), `functions-globals-${template}-`));
  const fnDir = join(root, "functions");
  for (const file of ["tsconfig.json", "oxy-globals.d.ts"]) {
    cpSync(join(TEMPLATES, template, "functions", file), join(fnDir, file));
  }
  writeFileSync(join(fnDir, "probe.ts"), probe);
  const run = spawnSync(process.execPath, [TSC, "--noEmit", "-p", "functions/tsconfig.json"], {
    cwd: root,
    encoding: "utf8"
  });
  return { status: run.status, out: `${run.stdout}${run.stderr}` };
}

test("the templates with functions are found", () => {
  expect(withFunctions).toContain("functions");
});

describe.each(withFunctions)("the %s template's functions/", (template) => {
  test("typecheck against the isolate's globals, with no import to lend them", () => {
    const { status, out } = typecheck(template, USES_THE_ISOLATE);
    expect(out).toBe("");
    expect(status).toBe(0);
  });

  test("still refuse a global the isolate lacks, and a Response member it lacks", () => {
    const { status, out } = typecheck(template, USES_WHAT_IT_LACKS);
    expect(status).not.toBe(0);
    expect(out).toContain("Cannot find name 'TextEncoder'");
    expect(out).toContain("Property 'text' does not exist on type 'Response'");
  });

  test("still typecheck when an import pulls the DOM lib in, as preact's types do", () => {
    const { status, out } = typecheck(template, `/// <reference lib="dom" />\n${USES_THE_ISOLATE}`);
    expect(out).toBe("");
    expect(status).toBe(0);
  });
});
