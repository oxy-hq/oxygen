/**
 * `env.<KEY>.shared`: a key staging may read from production must be one no
 * function writes and no webhook verifies with — the server's publish gate
 * (`check_shared_env`), answered first by `oxyc validate`.
 */

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { checkAppPlacement } from "./placement.js";
import { checkSharedEnv, secretsSetKeys } from "./shared-env.js";

const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

function app(manifest: Record<string, unknown>, files: Record<string, string> = {}): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-shared-env-"));
  SCRATCH.push(dir);
  writeFileSync(join(dir, "oxy-app.json"), JSON.stringify(manifest));
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), content);
  }
  return dir;
}

describe("secretsSetKeys", () => {
  it("reads string-literal keys and skips computed ones and comments", () => {
    const source = [
      'await ctx.secrets.set("QB_REFRESH_TOKEN", t);',
      "await ctx.secrets.set(keyFor(org), v);",
      "// ctx.secrets.set('COMMENTED', v)",
      "await e.secrets.set(`MINIFIED`, v);"
    ].join("\n");
    expect(secretsSetKeys(source)).toEqual([
      { key: "QB_REFRESH_TOKEN", line: 1 },
      { key: "MINIFIED", line: 4 }
    ]);
  });

  it("reads the optional, bracketed and destructured spellings the server reads", () => {
    const source = [
      'await ctx.secrets?.set("OPTIONAL", t);',
      "await ctx['secrets'].set('BRACKETED', t);",
      'const{secrets:s,env:n}=ctx;await s.set("ALIASED",n.X);',
      "const { secrets: store } = ctx;\nawait store?.set(`SPACED`, v);",
      'x.s.set("NOT_THE_ALIAS", v); cache.set("NOT_SECRETS", v);'
    ].join("\n");
    expect(secretsSetKeys(source).map((k) => k.key)).toEqual([
      "OPTIONAL",
      "BRACKETED",
      "ALIASED",
      "SPACED"
    ]);
  });
});

describe("checkSharedEnv", () => {
  const manifest = {
    slug: "books",
    env: {
      QB_REFRESH_TOKEN: { shared: true },
      SIGNING_KEY: { shared: true },
      POKEHOUSE_API_KEY: { shared: true },
      PRIVATE: {}
    },
    functions: {
      refresh: { secrets: { write: true } },
      hook: { webhook: { secretVar: "OLD_KEY, SIGNING_KEY" } }
    }
  };
  const files = {
    "functions/refresh.ts":
      'export default async (req, ctx) => {\n  await ctx.secrets.set("QB_REFRESH_TOKEN", t);\n};\n',
    "functions/hook.ts": "export default async () => new Response();\n"
  };

  it("refuses shared on a written key and a webhook secret, not a read-only key", () => {
    const issues = checkSharedEnv(app(manifest, files), manifest);
    expect(issues.map((i) => i.path).sort()).toEqual([
      "env.QB_REFRESH_TOKEN.shared",
      "env.SIGNING_KEY.shared"
    ]);
    expect(issues.every((i) => i.level === "error")).toBe(true);
    const written = issues.find((i) => i.path === "env.QB_REFRESH_TOKEN.shared");
    expect(written?.message).toContain("written by function `refresh`");
    expect(written?.message).toContain("line 2");
  });

  it("is part of oxyc validate's placement check", () => {
    const dir = app(manifest, files);
    const errors = checkAppPlacement(dir, manifest, "books").filter((i) => i.level === "error");
    expect(errors.map((i) => i.path)).toContain("env.SIGNING_KEY.shared");
  });

  it("refuses a flag that is not a boolean, and passes a manifest with nothing shared", () => {
    const bad = { env: { K: { shared: "yes" } } };
    expect(checkSharedEnv(app(bad), bad)).toMatchObject([
      { level: "error", path: "env.K.shared", message: "must be a boolean" }
    ]);
    const none = { env: { K: { required: true } }, functions: manifest.functions };
    expect(checkSharedEnv(app(none, files), none)).toEqual([]);
  });
});
