/**
 * `--jq` without a `jq` binary — what a fresh CI runner or a cloud VM has.
 *
 * The filters are the ones the docs teach (CLAUDE.md, README, the oxy-cli
 * skill, `oxyc guide`), because those are what an agent pastes first. Each runs
 * on the embedded engine, and against the real binary too where one is
 * installed: the claim is not "it works" but "it prints what jq prints", so the
 * binary is the oracle.
 */

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, symlinkSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { CliError, ExitCode } from "../util/errors.js";
import { runEmbeddedJq, runJq } from "./jq.js";

const ORGS = JSON.stringify([
  { id: "o1", name: "acme", role: "owner", tags: ["a", "b"] },
  { id: "o2", name: "globex", role: "member", tags: [] }
]);
const THREADS = JSON.stringify({ threads: [{ title: "Q3 revenue" }, { title: "churn" }] });

/** Filter → body → exactly what `jq -r` prints. */
const DOC_EXAMPLES: [string, string, string][] = [
  [".[].name", ORGS, "acme\nglobex\n"],
  ["length", ORGS, "2\n"],
  [".threads[].title", THREADS, "Q3 revenue\nchurn\n"],
  ['.[] | select(.role == "owner") | .name', ORGS, "acme\n"],
  ["map(.id)", ORGS, '[\n  "o1",\n  "o2"\n]\n'],
  ['.[] | "\\(.name) (\\(.role))"', ORGS, "acme (owner)\nglobex (member)\n"],
  [".[0].tags", ORGS, '[\n  "a",\n  "b"\n]\n'],
  [".[1].tags", ORGS, "[]\n"],
  [".[0] | keys", ORGS, '[\n  "id",\n  "name",\n  "role",\n  "tags"\n]\n'],
  ["empty", ORGS, ""]
];

const hasJqBinary = spawnSync("jq", ["--version"]).error === undefined;

describe("the embedded jq", () => {
  it.each(DOC_EXAMPLES)("%s prints what jq -r prints", async (filter, body, expected) => {
    expect(await runEmbeddedJq(body, filter)).toBe(expected);
  });

  it.runIf(hasJqBinary).each(DOC_EXAMPLES)("%s matches the jq binary", async (filter, body) => {
    expect(await runEmbeddedJq(body, filter)).toBe(await runJq(body, filter));
  });

  /**
   * jq-wasm trims its stdout, which would eat the first row's indent here and
   * any trailing whitespace on the last — the reason the output is bracketed.
   */
  it("keeps leading and trailing whitespace a raw string carries", async () => {
    expect(await runEmbeddedJq(ORGS, '.[] | "  - \\(.name)  "')).toBe("  - acme  \n  - globex  \n");
  });

  it("survives a program ending in a comment, or made of a def", async () => {
    expect(await runEmbeddedJq(ORGS, "length # count them")).toBe("2\n");
    expect(await runEmbeddedJq(ORGS, "def n: .name; .[] | n")).toBe("acme\nglobex\n");
    expect(await runEmbeddedJq(ORGS, ".[0].name, .[1].name")).toBe("acme\nglobex\n");
  });

  it("runs the program once per JSON text, as jq does", async () => {
    expect(await runEmbeddedJq('{"a":1}\n{"a":2}', ".a")).toBe("1\n2\n");
  });

  it("prints nothing for an empty body, and what came before a halt", async () => {
    expect(await runEmbeddedJq("", "length")).toBe("");
    expect(await runEmbeddedJq(ORGS, ".[0].name, halt")).toBe("acme\n");
  });

  it("is reused across calls", async () => {
    for (let i = 0; i < 20; i++) expect(await runEmbeddedJq(ORGS, "length")).toBe("2\n");
  });
});

describe("a bad filter", () => {
  async function failure(run: Promise<string>): Promise<CliError> {
    const error = await run.then(
      () => undefined,
      (e: unknown) => e
    );
    expect(error).toBeInstanceOf(CliError);
    return error as CliError;
  }

  /**
   * The exit code is the contract (`oxyc exit-codes`): an agent branches on it.
   * And the message must point at the program the person TYPED — the engine
   * runs a bracketed copy, whose line numbers would be off by one.
   */
  it("is a usage error naming the column in the program as typed", async () => {
    const error = await failure(runEmbeddedJq(ORGS, ".[] | nope("));
    expect(error.code).toBe(ExitCode.USAGE);
    expect(error.message).toBe("jq exited 3");
    expect(error.detail).toMatch(/syntax error/);
    expect(error.detail).toMatch(/line 1, column 11/);
  });

  it("is a usage error at run time too, and says <stdin> as jq does", async () => {
    const error = await failure(runEmbeddedJq(ORGS, ".[0].name | keys"));
    expect(error.code).toBe(ExitCode.USAGE);
    expect(error.detail).toBe('jq: error (at <stdin>:0): string ("acme") has no keys');
  });

  it("is a usage error on a body that is not JSON", async () => {
    const error = await failure(runEmbeddedJq("<html>", "."));
    expect(error.code).toBe(ExitCode.USAGE);
    expect(error.detail).toMatch(/parse error/);
  });

  it.runIf(hasJqBinary)("gets the same exit code from the binary", async () => {
    const error = await failure(runJq(ORGS, ".[] | nope("));
    expect(error.code).toBe(ExitCode.USAGE);
    expect(error.message).toBe("jq exited 3");
  });
});

describe("runJq", () => {
  it("falls back to the embedded engine when the binary is not installed", async () => {
    expect(await runJq(ORGS, "length", { binary: "oxyc-no-such-jq" })).toBe("2\n");
  });
});

/**
 * The task's own acceptance line: `env -i PATH=<node only> oxyc api … -q length`
 * prints a number. A PATH holding nothing but a `node` symlink is the bare
 * container, whatever this machine has installed.
 */
describe("oxyc api -q with no jq on PATH", () => {
  const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
  let server: Server;
  let target: string;

  beforeAll(async () => {
    server = createServer((_req, res) => {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(ORGS);
    });
    await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
    target = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });

  afterAll(() => {
    server.close();
  });

  function oxyc(...args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
    if (!existsSync(BIN)) {
      throw new Error(
        `${BIN} is missing — run \`pnpm build\` (or \`pnpm test\`, which builds first)`
      );
    }
    const nodeOnly = mkdtempSync(join(tmpdir(), "oxyc-node-only-"));
    symlinkSync(process.execPath, join(nodeOnly, "node"));
    return new Promise((done, fail) => {
      const child = spawn(join(nodeOnly, "node"), [BIN, ...args, "--target", target], {
        env: {
          PATH: nodeOnly,
          OXY_TOKEN: "oxy_pat_test",
          // Nothing here may read a developer's real credentials or cache.
          OXY_CREDENTIALS_PATH: join(nodeOnly, "__no_such_credentials__.json"),
          OXYC_CACHE_DIR: join(nodeOnly, "__no_such_cache__")
        }
      });
      let stdout = "";
      let stderr = "";
      child.stdout.on("data", (chunk) => {
        stdout += String(chunk);
      });
      child.stderr.on("data", (chunk) => {
        stderr += String(chunk);
      });
      child.on("error", fail);
      child.on("close", (code) => done({ status: code ?? -1, stdout, stderr }));
    });
  }

  it("prints a number for -q length", async () => {
    const r = await oxyc("api", "/api/orgs", "-q", "length");
    expect(r.stderr).toBe("");
    expect(r.status).toBe(ExitCode.OK);
    expect(r.stdout).toBe("2\n");
  });

  it("exits with the usage code for a bad filter", async () => {
    const r = await oxyc("api", "/api/orgs", "-q", ".[] | nope(");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stdout).toBe("");
    expect(r.stderr).toMatch(/syntax error/);
  });
});
