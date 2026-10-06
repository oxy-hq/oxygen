/**
 * The MCP server, driven over real stdio.
 *
 * Spawned rather than unit-tested because the thing worth checking is the
 * PROTOCOL: that the handshake completes, that stdout carries frames and
 * nothing else, and that a failing tool call comes back as a result the model
 * can read rather than as a dead transport.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, describe, expect, it } from "vitest";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");

interface Frame {
  id?: number;
  result?: Record<string, unknown>;
  error?: Record<string, unknown>;
}

/**
 * Send a batch of JSON-RPC requests and collect the frames that come back.
 *
 * Points every credential and cache lookup at a path that does not exist, so
 * nothing here can read a developer's real token or reach a real deployment —
 * the tool calls under test are the ones that fail before any network I/O,
 * UNLESS `net` names a real (local) target and token, for the handful of
 * tests that need the tool to actually complete a request — a spawned
 * subprocess talking to `fetch` has no stub to intercept, so those tests spin
 * up a real `node:http` server instead (see `fakeServer` below).
 */
async function rpc(
  requests: object[],
  timeoutMs = 20_000,
  cacheDir?: string,
  net: { target?: string; token?: string; workspace?: string } = {}
): Promise<Frame[]> {
  // `--login`: these sessions stand for a person serving on their own login —
  // the mode every test here was written against. The fail-closed default, and
  // an agent's own token, are `session` below.
  const args = ["--login"];
  if (net.target) args.push("--target", net.target);
  if (net.workspace) args.push("--workspace", net.workspace);
  const { frames } = await session(requests, { args, token: net.token, cacheDir, timeoutMs });
  return frames;
}

/**
 * One `oxyc mcp` process, start to exit: the frames it wrote, what it said on
 * stderr, and its exit code — `null` when it had to be killed at the timeout.
 */
async function session(
  requests: object[],
  opts: {
    args?: string[];
    token?: string;
    env?: Record<string, string>;
    cacheDir?: string;
    timeoutMs?: number;
  } = {}
): Promise<{ frames: Frame[]; stderr: string; code: number | null }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);

  const child = spawn(process.execPath, [BIN, "mcp", "--env", "production", ...(opts.args ?? [])], {
    env: {
      ...process.env,
      OXY_CREDENTIALS_PATH: join(BIN, "..", "__no_creds__.json"),
      OXYC_CACHE_DIR: opts.cacheDir ?? join(BIN, "..", "__no_cache__"),
      OXY_TOKEN: opts.token ?? "",
      OXY_API_KEY: "",
      // Cleared: this suite is itself often run by an agent, whose harness
      // sets the marker the user agent detects.
      OXY_AGENT: "",
      CLAUDECODE: "",
      NO_COLOR: "1",
      ...opts.env
    }
  });

  let out = "";
  let stderr = "";
  child.stdout.on("data", (d) => {
    out += d;
  });
  child.stderr.on("data", (d) => {
    stderr += d;
  });
  // A process that refused to start has already closed its stdin.
  child.stdin.on("error", () => {});
  for (const req of requests) child.stdin.write(`${JSON.stringify(req)}\n`);
  child.stdin.end();

  const code = await Promise.race([
    new Promise<number | null>((r) => child.on("close", (exit) => r(exit))),
    new Promise<null>((r) =>
      setTimeout(() => {
        child.kill();
        r(null);
      }, opts.timeoutMs ?? 20_000)
    )
  ]);

  const frames = out
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line) as Frame);
  return { frames, stderr, code };
}

/**
 * A real local HTTP server for the handful of tests that need an MCP tool to
 * actually complete a request — a spawned subprocess's own `fetch` cannot be
 * stubbed the way `preview.test.ts` stubs `globalThis.fetch` in-process.
 * `routes` matches `"<METHOD> <path>"` (path including its query string,
 * exactly as the client sends it) to a JSON body; an unmatched request 404s
 * loudly rather than hanging, so a routing mistake fails fast.
 */
function fakeServer(
  routes: Record<string, unknown>
): Promise<{ url: string; close: () => Promise<void>; userAgents: string[] }> {
  /** The `User-Agent` of every request, in arrival order. */
  const userAgents: string[] = [];
  return new Promise((resolveServer) => {
    const server: Server = createServer((req: IncomingMessage, res: ServerResponse) => {
      const key = `${(req.method ?? "GET").toUpperCase()} ${req.url}`;
      userAgents.push(req.headers["user-agent"] ?? "");
      const body = routes[key];
      res.setHeader("content-type", "application/json");
      if (body === undefined) {
        res.writeHead(404);
        res.end(JSON.stringify({ code: "not_stubbed", message: `no stub for ${key}` }));
        return;
      }
      // `{ $status }` answers with that status and no body worth reading —
      // unless it carries one: `$body` as JSON, or `$text` sent as it is
      // (`""` is a bare status, the way a refused route answers).
      const reply = body as { $status?: number; $body?: unknown; $text?: string } | null;
      const status = reply?.$status;
      if (typeof reply?.$text === "string") {
        res.setHeader("content-type", "text/plain; charset=utf-8");
        res.writeHead(status ?? 200);
        res.end(reply.$text);
        return;
      }
      res.writeHead(status ?? 200);
      res.end(JSON.stringify(status === undefined ? body : (reply?.$body ?? {})));
    });
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      const port = typeof addr === "object" && addr ? addr.port : 0;
      resolveServer({
        url: `http://127.0.0.1:${port}`,
        close: () => new Promise((r) => server.close(() => r())),
        userAgents
      });
    });
  });
}

/**
 * A cache directory holding a ready route catalog for `--env production`.
 *
 * `loadCatalog` reads this before it reaches for the network and returns it
 * whole while it is inside the TTL, so the refusal paths below are exercised
 * against a real catalog with no deployment and no credential in sight. The
 * file layout is `catalog/<hostKey>.json` with the host stored inside it —
 * `readDisk` rejects a file whose stored host disagrees with the target.
 */
function seedCatalog(routes: Array<{ path: string; description?: string }>): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-mcp-"));
  SCRATCH.push(dir);
  mkdirSync(join(dir, "catalog"), { recursive: true });
  writeFileSync(
    join(dir, "catalog", "app.oxygen-hq.com.json"),
    JSON.stringify({
      host: "app.oxygen-hq.com",
      fetchedAt: Date.now(),
      surfaces: [{ id: "workspace", label: "Workspace", credential: "bearer" }],
      routes: routes.map((r) => ({
        method: "GET",
        path: r.path,
        surface: "workspace",
        credential: "bearer",
        path_parameters: [],
        description: r.description ?? "",
        note: "",
        handler: "handler",
        role: "fleet-ok"
      }))
    })
  );
  return dir;
}

/** `n` routes that all carry the same substring, so one filter matches them all. */
function manyRoutes(n: number): Array<{ path: string; description: string }> {
  return Array.from({ length: n }, (_, i) => ({
    path: `/api/thing-${i}/details`,
    description: `endpoint number ${i} in the same family as every other one`
  }));
}

const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const INIT = {
  jsonrpc: "2.0",
  id: 1,
  method: "initialize",
  params: {
    protocolVersion: "2024-11-05",
    capabilities: {},
    clientInfo: { name: "test", version: "0" }
  }
};

describe("the MCP handshake", () => {
  it("initialises and names itself", async () => {
    const [init] = await rpc([INIT]);
    expect((init?.result?.serverInfo as { name?: string })?.name).toBe("oxyc");
  });
});

describe("the tool list", () => {
  /**
   * FOUR GENERIC TOOLS, PLUS TWO NAMED EXCEPTIONS — pinned by name so a fifth
   * generic tool or a stray addition to either loop is a decision, not a
   * drift. `SANDBOX_TOOLS` (10) and `PREVIEW_TOOLS` (8) exist because the
   * sandbox and workspace-previews loops are meant to run unsupervised, which
   * needs per-operation validation (the dev-<handle> grammar, a confirm gate
   * before a delete, a hard refusal of a non-sandbox publish) that the four
   * generic tools cannot carry — see the module doc in `mcp.ts`.
   */
  it("exposes the four generic tools plus the sandbox and preview loops", async () => {
    const frames = await rpc([INIT, { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} }]);
    const tools = (frames.find((f) => f.id === 2)?.result?.tools ?? []) as { name: string }[];
    expect(tools.map((t) => t.name).sort()).toEqual([
      "oxy_checks_run",
      "oxy_env_create",
      "oxy_env_delete",
      "oxy_env_list",
      "oxy_env_show",
      "oxy_fn_call",
      "oxy_invocations_held",
      "oxy_invocations_list",
      "oxy_logs",
      "oxy_preview_checks",
      "oxy_preview_create",
      "oxy_preview_delete",
      "oxy_preview_list",
      "oxy_preview_run",
      "oxy_preview_run_show",
      "oxy_preview_runs_list",
      "oxy_preview_show",
      "oxy_publish_sandbox",
      "oxy_request",
      "oxy_routes",
      "oxy_schema",
      "oxy_whoami"
    ]);
  });

  /**
   * THE DESIGN DECISION, PINNED, RAISED DELIBERATELY. One tool per endpoint
   * would mean ~670 tool schemas, and an agent runtime ships every one of
   * them in EVERY request — tens of kilobytes of context spent per turn
   * before a question is asked. Four generic tools cost a couple of KB and
   * still reach every endpoint, including ones added after this package
   * shipped — that principle is intact for the ~660 routes NOT in the
   * sandbox or previews loop.
   *
   * The 22-tool list above costs ~14.7 KB, not a couple — raised from the
   * original 8 KB on purpose, because the sandbox/previews tools trade
   * minimalism for validation an agent needs to drive either loop
   * unsupervised (see the tool-list test above and `mcp.ts`'s module doc).
   * Ceiling set to 17 KB, not a round 20 KB: close enough to the ~14.7 KB
   * actual that a tool added without trimming anything still trips it, but
   * with enough headroom that a wording tweak to an existing description
   * does not. The ceiling still catches erosion: it is not "whatever the
   * count happens to be today" but a number a couple of named exceptions
   * can clear without much room to spare.
   */
  it("keeps the per-turn schema cost bounded — raised for the two named loops", async () => {
    const frames = await rpc([INIT, { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} }]);
    const tools = frames.find((f) => f.id === 2)?.result?.tools;
    const bytes = JSON.stringify(tools).length;
    expect(bytes).toBeLessThan(17_000);
  });

  it("tells the model to discover before it guesses", async () => {
    const frames = await rpc([INIT, { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} }]);
    const tools = (frames.find((f) => f.id === 2)?.result?.tools ?? []) as {
      name: string;
      description: string;
    }[];
    const routes = tools.find((t) => t.name === "oxy_routes");
    expect(routes?.description).toMatch(/before .*oxy_request|rather than guessing/i);
  });
});

describe("tool failures", () => {
  /**
   * A throw inside a handler would kill the session. The model can act on
   * "here is what went wrong"; it cannot act on a dead transport.
   */
  it("returns an unknown tool as a result, not as a broken stream", async () => {
    const frames = await rpc([
      INIT,
      { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "nope", arguments: {} } },
      { jsonrpc: "2.0", id: 3, method: "tools/list", params: {} }
    ]);
    const failed = frames.find((f) => f.id === 2);
    expect(failed?.result?.isError).toBe(true);
    // The session survived: a later request still gets an answer.
    expect(frames.find((f) => f.id === 3)?.result?.tools).toBeDefined();
  });

  /**
   * With no credential, a call that needs one must come back as a readable
   * error carrying the hint — the same `CliError` the CLI would have printed.
   */
  it("surfaces a missing credential as a readable error, with the hint", async () => {
    const frames = await rpc([
      INIT,
      {
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: { name: "oxy_whoami", arguments: {} }
      }
    ]);
    const result = frames.find((f) => f.id === 2)?.result;
    expect(result?.isError).toBe(true);
    const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
    expect(body).toMatch(/not authenticated/i);
    expect(body).toMatch(/oxyc login/);
    // THE EXIT-CODE CLASS, APPENDED — every CliError now carries one, so an
    // agent can branch on `[exit 4 AUTH]` the way it would on `oxyc`'s own
    // exit code, by name rather than a bare digit. Checked here, on the
    // generic catch-all's own test, rather than repeated on every new
    // sandbox/preview tool's failure case below.
    expect(body).toMatch(/\[exit 4 AUTH\]/);
  });

  /** An unresolved placeholder is caught before any request is attempted. */
  it("refuses an unresolvable placeholder with the flag that would fill it", async () => {
    const frames = await rpc([
      INIT,
      {
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: { name: "oxy_request", arguments: { path: "{workspace}/threads" } }
      }
    ]);
    const result = frames.find((f) => f.id === 2)?.result;
    expect(result?.isError).toBe(true);
    const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
    expect(body).toMatch(/could not resolve \{workspace\}/);
    expect(body).toMatch(/--workspace/);
  });
});

describe("the route cap", () => {
  /**
   * THE COUNT IS THE TEST, not whether a filter was supplied. `searchRoutes`
   * matches a substring of method, path, surface OR description, so a filter
   * like "e" or "api" narrows nothing — and an uncapped result ships every
   * matching route WITH its description straight into a context window. This
   * case is the whole reason the guard is not gated on `!filter`.
   */
  it("degrades a broad FILTER to method and path, rather than refusing it", async () => {
    const dir = seedCatalog(manyRoutes(200));
    const frames = await rpc(
      [
        INIT,
        {
          jsonrpc: "2.0",
          id: 2,
          method: "tools/call",
          params: { name: "oxy_routes", arguments: { filter: "e" } }
        }
      ],
      20_000,
      dir
    );
    const result = frames.find((f) => f.id === 2)?.result;
    // NOT an error: "admin", "workspace" and "query" all clear 60 in good
    // faith, and "guess something narrower" is a dead end for a caller who
    // cannot see what it hit.
    expect(result?.isError).toBeFalsy();
    const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
    expect(body).toMatch(/200 routes match/);
    // It names the filter that failed to narrow, and how to get descriptions back.
    expect(body).toContain('"e"');
    expect(body).toMatch(/description/);

    // Split on the blank line every note ends with, not on the first "[" — a
    // note that ever contains a bracket would silently take the wrong slice.
    // Guarded for the no-note body, where `lastIndexOf` is -1 and a bare
    // `slice(+2)` would eat the leading bracket instead of failing.
    const payload = body.includes("\n\n") ? body.slice(body.lastIndexOf("\n\n") + 2) : body;
    const rows = JSON.parse(payload) as Record<string, unknown>[];
    expect(rows).toHaveLength(200);
    // DESCRIPTION is the only field dropped: `credential` is what separates the
    // bearer mount from the API-key one, and a caller needs it in either form.
    expect(Object.keys(rows[0] ?? {}).sort()).toEqual(["credential", "method", "path"]);
  });

  /**
   * Refusal is reserved for a match set too large even to list. At ~670 routes
   * a deployment-wide match is the only thing that reaches it.
   */
  it("still refuses a match set too large even to list", async () => {
    const dir = seedCatalog(manyRoutes(500));
    const frames = await rpc(
      [
        INIT,
        {
          jsonrpc: "2.0",
          id: 2,
          method: "tools/call",
          params: { name: "oxy_routes", arguments: { filter: "e" } }
        }
      ],
      20_000,
      dir
    );
    const result = frames.find((f) => f.id === 2)?.result;
    expect(result?.isError).toBe(true);
    const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
    expect(body).toMatch(/500 routes match/);
    expect(body).toMatch(/narrower/i);
  });

  it("returns a genuinely narrow filter in full", async () => {
    const dir = seedCatalog([
      ...manyRoutes(200),
      { path: "/api/{workspace}/threads", description: "list the threads" }
    ]);
    const frames = await rpc(
      [
        INIT,
        {
          jsonrpc: "2.0",
          id: 2,
          method: "tools/call",
          params: { name: "oxy_routes", arguments: { filter: "threads" } }
        }
      ],
      20_000,
      dir
    );
    const result = frames.find((f) => f.id === 2)?.result;
    expect(result?.isError).toBeFalsy();
    const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
    // Under the threshold the description is present — that is the whole
    // difference between this and the degraded listing above.
    expect(JSON.parse(body)).toEqual([
      {
        method: "GET",
        path: "/api/{workspace}/threads",
        credential: "bearer",
        description: "list the threads"
      }
    ]);
  });

  /**
   * An empty array reads as "this deployment has no such endpoint", which is
   * usually false — so a zero match says what to do instead.
   */
  it("explains a zero match rather than returning []", async () => {
    const dir = seedCatalog([{ path: "/api/{workspace}/threads" }]);
    const frames = await rpc(
      [
        INIT,
        {
          jsonrpc: "2.0",
          id: 2,
          method: "tools/call",
          params: { name: "oxy_routes", arguments: { filter: "zzzznope" } }
        }
      ],
      20_000,
      dir
    );
    const result = frames.find((f) => f.id === 2)?.result;
    expect(result?.isError).toBe(true);
    const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
    expect(body).toMatch(/No route matches/);
    expect(body).toMatch(/all=true/);
  });
});

describe("required arguments", () => {
  /**
   * A model that omits `path` used to reach `request()` with `undefined` and
   * build a call to `/api/` — a 404 from the deployment, which reads as "that
   * endpoint does not exist" rather than "you forgot an argument".
   */
  it("names the missing argument instead of calling /api/", async () => {
    const frames = await rpc([
      INIT,
      {
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: { name: "oxy_request", arguments: {} }
      }
    ]);
    const result = frames.find((f) => f.id === 2)?.result;
    expect(result?.isError).toBe(true);
    const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
    expect(body).toMatch(/oxy_request needs path/);
  });

  /** An empty string is as missing as an absent key, and easier to send. */
  it("treats a blank string as missing", async () => {
    const frames = await rpc([
      INIT,
      {
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: { name: "oxy_schema", arguments: { path: "   " } }
      }
    ]);
    const result = frames.find((f) => f.id === 2)?.result;
    expect(result?.isError).toBe(true);
    const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
    expect(body).toMatch(/oxy_schema needs path/);
  });
});

/** One `tools/call`, with the result's `isError` and text body extracted. */
async function callTool(
  name: string,
  args: Record<string, unknown> = {}
): Promise<{ isError: boolean; body: string }> {
  const frames = await rpc([
    INIT,
    { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name, arguments: args } }
  ]);
  const result = frames.find((f) => f.id === 2)?.result;
  const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
  return { isError: Boolean(result?.isError), body };
}

/** `callTool`, but against a real (local) server — see `fakeServer`. */
async function callToolAt(
  net: { target: string; token: string; workspace: string },
  name: string,
  args: Record<string, unknown> = {}
): Promise<{ isError: boolean; document: unknown }> {
  const frames = await rpc(
    [INIT, { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name, arguments: args } }],
    20_000,
    undefined,
    net
  );
  const result = frames.find((f) => f.id === 2)?.result;
  const body = ((result?.content ?? []) as { text: string }[])[0]?.text ?? "";
  // `jsonResult` appends "\n\n[exit N NAME]" on failure — the document is
  // everything before that first blank line.
  const jsonText = body.split("\n\n")[0] ?? body;
  return { isError: Boolean(result?.isError), document: JSON.parse(jsonText) };
}

describe("sandbox tools — client-side validation before any request", () => {
  /**
   * The same grammar `oxyc env create` validates, caught before any request —
   * `envCreate` checks `requireSandboxName` before `staffCreds` ever reaches
   * for a token, so this needs no credential to exercise.
   */
  it("oxy_env_create refuses a malformed sandbox name", async () => {
    const { isError, body } = await callTool("oxy_env_create", {
      app: "acme/store",
      name: "not valid!"
    });
    expect(isError).toBe(true);
    expect(body).toMatch(/not a valid environment/);
    expect(body).toMatch(/\[exit 2 USAGE\]/);
  });

  /** There is no terminal inside an MCP server to confirm on — confirm=true is mandatory. */
  it("oxy_env_delete refuses without confirm=true", async () => {
    const { isError, body } = await callTool("oxy_env_delete", {
      app: "acme/store",
      name: "dev-a1"
    });
    expect(isError).toBe(true);
    expect(body).toMatch(/needs confirm=true/);
  });

  /**
   * THE HARD REFUSAL. This tool exists so an agent can iterate in a sandbox
   * unsupervised; it must never be able to reach the promote/production path,
   * and that refusal happens before any build or request, reusing the exact
   * grammar `oxyc publish --app-env` validates.
   */
  it("oxy_publish_sandbox refuses production client-side", async () => {
    const { isError, body } = await callTool("oxy_publish_sandbox", { appEnv: "production" });
    expect(isError).toBe(true);
    expect(body).toMatch(/is not a sandbox/);
  });

  it("oxy_publish_sandbox refuses staging client-side", async () => {
    const { isError, body } = await callTool("oxy_publish_sandbox", { appEnv: "staging" });
    expect(isError).toBe(true);
    expect(body).toMatch(/is not a sandbox/);
  });

  it("oxy_fn_call refuses a malformed appEnv before any request", async () => {
    const { isError, body } = await callTool("oxy_fn_call", {
      app: "acme/store",
      function: "f",
      appEnv: "not-a-real-env!"
    });
    expect(isError).toBe(true);
    expect(body).toMatch(/not a valid environment/);
  });

  it("oxy_checks_run names the missing argument", async () => {
    const { isError, body } = await callTool("oxy_checks_run", {});
    expect(isError).toBe(true);
    expect(body).toMatch(/oxy_checks_run needs app/);
  });

  it("oxy_invocations_held names the missing argument", async () => {
    const { isError, body } = await callTool("oxy_invocations_held", { app: "acme/store" });
    expect(isError).toBe(true);
    expect(body).toMatch(/oxy_invocations_held needs invocationId/);
  });

  it("oxy_logs surfaces the missing credential", async () => {
    const { isError, body } = await callTool("oxy_logs", { app: "acme/store" });
    expect(isError).toBe(true);
    expect(body).toMatch(/not authenticated/i);
  });
});

describe("preview tools — client-side validation and workspace resolution", () => {
  /**
   * `oxy_preview_run` checks the kind grammar before touching `ctx` at all,
   * so this needs neither a credential nor `--workspace`. The message also
   * has to say where the two un-startable kinds ARE reachable, since an
   * agent reading only the refusal has to find the next step on its own.
   */
  it("oxy_preview_run refuses a kind the server cannot start", async () => {
    const { isError, body } = await callTool("oxy_preview_run", {
      branch: "feature/x",
      kind: "transform_build",
      ref: "some/automation.automation.yml"
    });
    expect(isError).toBe(true);
    expect(body).toMatch(/not a run kind/);
    expect(body).toMatch(/oxy_preview_runs_list|runs show/);
  });

  it("oxy_preview_run refuses an unsupported compare kind too", async () => {
    const { isError, body } = await callTool("oxy_preview_run", {
      branch: "feature/x",
      kind: "compare",
      ref: "x"
    });
    expect(isError).toBe(true);
    expect(body).toMatch(/not a run kind/);
  });

  /**
   * `variables` must go through `preview-runs.ts`'s OWN `parseVariables` — the
   * same validator `oxyc preview run --variables` uses — not the generic
   * `parseJson`, which silently returns `undefined` on bad JSON and would
   * submit a real held run with its variables quietly dropped. Reaching the
   * USAGE error here (rather than the `{workspace}` placeholder error
   * `oxy_preview_run` would hit next, since no `--workspace` is passed
   * anywhere in this suite) proves `previewSubmitRun` — and so any request —
   * was never reached.
   */
  it("oxy_preview_run refuses malformed JSON in variables, before any request", async () => {
    const { isError, body } = await callTool("oxy_preview_run", {
      branch: "feature/x",
      kind: "procedure",
      ref: "a.automation.yml",
      variables: "{not valid json"
    });
    expect(isError).toBe(true);
    expect(body).toMatch(/--variables is not valid JSON/);
    expect(body).not.toMatch(/\{workspace\}/);
  });

  /** No terminal to confirm on, same rule as `oxy_env_delete`. */
  it("oxy_preview_delete refuses without confirm=true", async () => {
    const { isError, body } = await callTool("oxy_preview_delete", { branch: "feature/x" });
    expect(isError).toBe(true);
    expect(body).toMatch(/needs confirm=true/);
  });

  /**
   * Every other preview tool needs `{workspace}` to build its request path —
   * resolved the same way `oxyc api {workspace}/...` resolves it, through
   * `ctx.placeholders()`. No `--workspace` is passed anywhere in this suite,
   * so this is the same placeholder error `oxy_request` gives, naming the
   * same flag — proof there is no second workspace resolver here.
   */
  it("oxy_preview_list names --workspace when it is not resolved", async () => {
    const { isError, body } = await callTool("oxy_preview_list");
    expect(isError).toBe(true);
    expect(body).toMatch(/could not resolve \{workspace\}/);
    expect(body).toMatch(/--workspace/);
  });

  it("oxy_preview_show names --workspace when it is not resolved", async () => {
    const { isError, body } = await callTool("oxy_preview_show", { branch: "feature/x" });
    expect(isError).toBe(true);
    expect(body).toMatch(/--workspace/);
  });

  it("oxy_preview_create names the missing argument", async () => {
    const { isError, body } = await callTool("oxy_preview_create", {});
    expect(isError).toBe(true);
    expect(body).toMatch(/oxy_preview_create needs branch/);
  });

  it("oxy_preview_run_show names the missing argument", async () => {
    const { isError, body } = await callTool("oxy_preview_run_show", {});
    expect(isError).toBe(true);
    expect(body).toMatch(/oxy_preview_run_show needs runId/);
  });
});

/**
 * `isError` on a terminal preview/run outcome — needs a real request to
 * complete, so these run against a real local `fakeServer` rather than the
 * credential-free paths every other case in this file stops short of.
 * Mirrors `preview.test.ts`'s "exits 1 on a waited-for failed compile" /
 * `preview-runs.test.ts`'s "--wait exits 1 on a failed/cancelled run" —
 * same underlying data, the MCP-side half of the same contract.
 */
describe("preview tools — isError on a terminal non-success outcome", () => {
  const WORKSPACE = "11111111-2222-3333-4444-555555555555";

  const RUN_DETAIL_BASE = {
    run_id: "run-1",
    branch: "feature/x",
    kind: "procedure",
    target_ref: "a.automation.yml",
    parent_run_id: null,
    revision_id: "rev-1",
    state: "finished",
    held_count: 0,
    requested_by: "u-1",
    created_at: "2026-10-01T09:00:00.000Z",
    started_at: "2026-10-01T09:00:01.000Z",
    finished_at: "2026-10-01T09:00:05.000Z",
    agentic_run_id: "ar-1",
    steps: [],
    compare: null,
    sample: null
  };

  it("oxy_preview_run_show sets isError on a failed outcome, with the document in the result", async () => {
    const { url, close } = await fakeServer({
      [`GET /api/${WORKSPACE}/previews/runs/run-1`]: {
        ...RUN_DETAIL_BASE,
        outcome: "failed",
        error: "boom"
      }
    });
    try {
      const { isError, document } = await callToolAt(
        { target: url, token: "tok", workspace: WORKSPACE },
        "oxy_preview_run_show",
        { runId: "run-1" }
      );
      expect(isError).toBe(true);
      expect(document).toMatchObject({ run_id: "run-1", outcome: "failed", error: "boom" });
    } finally {
      await close();
    }
  });

  /** `cancelled` must set `isError` too — the gap finding 3 calls out by name. */
  it("oxy_preview_run_show sets isError on a cancelled outcome", async () => {
    const { url, close } = await fakeServer({
      [`GET /api/${WORKSPACE}/previews/runs/run-1`]: { ...RUN_DETAIL_BASE, outcome: "cancelled" }
    });
    try {
      const { isError, document } = await callToolAt(
        { target: url, token: "tok", workspace: WORKSPACE },
        "oxy_preview_run_show",
        { runId: "run-1" }
      );
      expect(isError).toBe(true);
      expect(document).toMatchObject({ outcome: "cancelled" });
    } finally {
      await close();
    }
  });

  it("oxy_preview_run_show does NOT set isError on a succeeded outcome", async () => {
    const { url, close } = await fakeServer({
      [`GET /api/${WORKSPACE}/previews/runs/run-1`]: { ...RUN_DETAIL_BASE, outcome: "succeeded" }
    });
    try {
      const { isError, document } = await callToolAt(
        { target: url, token: "tok", workspace: WORKSPACE },
        "oxy_preview_run_show",
        { runId: "run-1" }
      );
      expect(isError).toBe(false);
      expect(document).toMatchObject({ outcome: "succeeded" });
    } finally {
      await close();
    }
  });

  it("oxy_preview_run sets isError on a waited-for failed outcome", async () => {
    const { url, close } = await fakeServer({
      [`POST /api/${WORKSPACE}/previews/runs`]: { run_id: "run-1", state: "queued" },
      [`GET /api/${WORKSPACE}/previews/runs/run-1`]: {
        ...RUN_DETAIL_BASE,
        outcome: "failed",
        error: "boom"
      }
    });
    try {
      const { isError, document } = await callToolAt(
        { target: url, token: "tok", workspace: WORKSPACE },
        "oxy_preview_run",
        { branch: "feature/x", kind: "procedure", ref: "a.automation.yml", waitSeconds: 5 }
      );
      expect(isError).toBe(true);
      expect(document).toMatchObject({ outcome: "failed" });
    } finally {
      await close();
    }
  });

  it("oxy_preview_create sets isError on a waited-for failed compile", async () => {
    const compiling = {
      branch: "feature/x",
      revision_id: null,
      sha: "abc123",
      status: "compiling",
      error: null,
      created_by: null,
      updated_at: "2026-10-01T09:00:00.000Z",
      compiled_at: null,
      checks: null
    };
    const failed = { ...compiling, status: "failed", error: "syntax error" };
    const { url, close } = await fakeServer({
      [`POST /api/${WORKSPACE}/previews`]: { item: compiling },
      [`GET /api/${WORKSPACE}/previews`]: { items: [failed] }
    });
    try {
      const { isError, document } = await callToolAt(
        { target: url, token: "tok", workspace: WORKSPACE },
        "oxy_preview_create",
        { branch: "feature/x", waitSeconds: 5 }
      );
      expect(isError).toBe(true);
      expect(document).toMatchObject({ status: "failed" });
    } finally {
      await close();
    }
  });
});

const LIST = { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} };

const call = (id: number, name: string, args: Record<string, unknown> = {}) => ({
  jsonrpc: "2.0",
  id,
  method: "tools/call",
  params: { name, arguments: args }
});

const textOf = (frames: Frame[], id: number): string =>
  ((frames.find((f) => f.id === id)?.result?.content ?? []) as { text: string }[])[0]?.text ?? "";

/**
 * `oxyc mcp` is what an agent runtime starts, with the agent's own token in
 * its environment. Unset, it must not serve on whatever `oxyc login` cached for
 * the machine's owner: it exits 4 before the transport is up. A person serving
 * on their own login says so, with `--login`.
 */
describe("the credential — fail closed unless --login", () => {
  it("exits 4 with no token variable set, and writes no frame", async () => {
    const { frames, stderr, code } = await session([INIT, LIST]);
    expect(code).toBe(4);
    expect(frames).toEqual([]);
    expect(stderr).toMatch(/OXY_TOKEN is not set/);
    expect(stderr).toMatch(/oxyc mcp --login/);
  });

  it("exits 4 when --token-env names a variable that is not set", async () => {
    const { frames, stderr, code } = await session([INIT, LIST], {
      args: ["--token-env", "OXYC_TEST_NEVER_SET"],
      // A token in the DEFAULT variable is not the one that was named.
      token: "oxy_pat_in_the_wrong_variable"
    });
    expect(code).toBe(4);
    expect(frames).toEqual([]);
    expect(stderr).toMatch(/OXYC_TEST_NEVER_SET is not set/);
  });

  it("starts with a token in the variable, with no --login", async () => {
    const { frames, code } = await session([INIT, LIST], { token: "oxy_pat_agent" });
    expect(code).toBe(0);
    expect(frames.find((f) => f.id === 2)?.result?.tools).toBeDefined();
  });

  it("refuses --login together with --token-env, as a usage error", async () => {
    const { code, frames } = await session([INIT], {
      args: ["--login", "--token-env", "OXYC_TEST_NEVER_SET"]
    });
    expect(code).toBe(2);
    expect(frames).toEqual([]);
  });
});

/**
 * THE SECOND PINNED LIST. A sandbox agent token (`oxy_sbx_…`) is served the
 * sandbox loop, `oxy_whoami`, and four tools that exist only for it — and none
 * of the generic or preview tools, which the server answers this token 404 on.
 */
describe("a sandbox agent token", () => {
  const SANDBOX_TOKEN = "oxy_sbx_0123456789abcdefghijABCDEFGHIJ012345";
  const DESCRIBED = {
    id: "tok-sbx-1",
    name: "fix the checkout",
    kind: "sandbox_agent",
    display_prefix: "oxy_sbx_0123",
    last_four: "2345",
    grants: [],
    blocked_orgs: [],
    expires_at: "2099-01-01T00:00:00Z",
    minter: { user_id: "u-1", email: "luong@oxy.tech" },
    apps: [
      { id: "a1a1a1a1-2222-3333-4444-555555555555", org_slug: "acme", slug: "store", name: "Store" }
    ]
  };
  const A1 = { "GET /api/auth/token": DESCRIBED };

  const served = async (requests: object[], routes: Record<string, unknown>) => {
    const { url, close } = await fakeServer(routes);
    try {
      return await session(requests, { args: ["--target", url], token: SANDBOX_TOKEN });
    } finally {
      await close();
    }
  };

  it("is served exactly these fifteen tools", async () => {
    const { frames, code } = await served([INIT, LIST], A1);
    expect(code).toBe(0);
    const tools = (frames.find((f) => f.id === 2)?.result?.tools ?? []) as { name: string }[];
    expect(tools.map((t) => t.name).sort()).toEqual([
      "oxy_checks_run",
      "oxy_env_create",
      "oxy_env_delete",
      "oxy_env_list",
      "oxy_env_secret_delete",
      "oxy_env_secret_list",
      "oxy_env_secret_set",
      "oxy_env_show",
      "oxy_fn_call",
      "oxy_invocations_held",
      "oxy_invocations_list",
      "oxy_logs",
      "oxy_publish_sandbox",
      "oxy_token_revoke",
      "oxy_whoami"
    ]);
  });

  it("costs less schema per turn than the list a person is served", async () => {
    const mine = await served([INIT, LIST], A1);
    const theirs = await rpc([INIT, LIST]);
    const bytes = (frames: Frame[]) =>
      JSON.stringify(frames.find((f) => f.id === 2)?.result?.tools).length;
    expect(bytes(mine.frames)).toBeLessThan(bytes(theirs));
    expect(bytes(mine.frames)).toBeLessThan(12_000);
  });

  it("requires the sandbox on every tool that defaults to production for staff", async () => {
    const { frames } = await served([INIT, LIST], A1);
    const tools = (frames.find((f) => f.id === 2)?.result?.tools ?? []) as {
      name: string;
      inputSchema: { required?: string[] };
    }[];
    for (const name of ["oxy_fn_call", "oxy_checks_run", "oxy_invocations_list", "oxy_logs"]) {
      expect(tools.find((t) => t.name === name)?.inputSchema.required).toContain("appEnv");
    }
  });

  it("refuses a dropped tool called by name anyway — the list is the surface", async () => {
    const { frames } = await served(
      [
        INIT,
        call(2, "oxy_request", { path: "orgs" }),
        call(3, "oxy_preview_list"),
        { ...LIST, id: 9 }
      ],
      A1
    );
    for (const id of [2, 3]) {
      expect(frames.find((f) => f.id === id)?.result?.isError).toBe(true);
      expect(textOf(frames, id)).toMatch(/not served to a sandbox agent token/);
    }
    // The session survived the refusals.
    expect(frames.find((f) => f.id === 9)?.result?.tools).toBeDefined();
  });

  it("oxy_whoami answers with the token's own description, not /api/user", async () => {
    const { frames } = await served([INIT, call(2, "oxy_whoami")], A1);
    const answered = JSON.parse(textOf(frames, 2)) as { token: typeof DESCRIBED };
    expect(answered.token.kind).toBe("sandbox_agent");
    expect(answered.token.minter.email).toBe("luong@oxy.tech");
    expect(answered.token.apps[0]?.slug).toBe("store");
  });

  it("the secret tools accept only a dev-<handle> environment", async () => {
    const { frames } = await served(
      [
        INIT,
        call(2, "oxy_env_secret_set", {
          app: "acme/store",
          appEnv: "staging",
          key: "STRIPE_KEY",
          value: "sk_test"
        }),
        call(3, "oxy_env_secret_list", { app: "acme/store", appEnv: "production" })
      ],
      A1
    );
    for (const id of [2, 3]) {
      expect(frames.find((f) => f.id === id)?.result?.isError).toBe(true);
      expect(textOf(frames, id)).toMatch(/is not a sandbox/);
      expect(textOf(frames, id)).toMatch(/\[exit 2 USAGE\]/);
    }
  });

  /**
   * A tool result is the error's one line, its hint and its exit class — never
   * the response body. So the server's reason has to be ON that line, in every
   * shape a route refuses this token with: coded JSON, plain text, plain text
   * led by a code, and a bare status.
   */
  it("says why the server refused, on the one line a model is given", async () => {
    const base = "/api/customer-apps/a1a1a1a1-2222-3333-4444-555555555555";
    const { frames } = await served(
      [
        INIT,
        call(2, "oxy_env_secret_set", {
          app: "acme/store",
          appEnv: "dev-a1",
          key: "OXY",
          value: SANDBOX_TOKEN
        }),
        call(3, "oxy_env_show", { app: "acme/store", name: "dev-b2" }),
        call(4, "oxy_env_create", { app: "acme/store", name: "dev-c3" }),
        call(5, "oxy_env_secret_list", { app: "acme/store", appEnv: "dev-b2" }),
        call(6, "oxy_invocations_held", { app: "acme/store", invocationId: "i-1" })
      ],
      {
        ...A1,
        [`POST ${base}/secrets`]: {
          $status: 400,
          $text:
            "credential_shaped_value: a sandbox agent token cannot store a value shaped like an Oxy credential"
        },
        [`GET ${base}/environments/dev-b2`]: {
          $status: 404,
          $body: { error: "environment_not_found", message: "this app has no environment dev-b2" }
        },
        [`POST ${base}/environments`]: {
          $status: 409,
          $body: {
            error: "token_sandbox_limit",
            message: "this token already holds 3 sandboxes, counting those still being deleted"
          }
        },
        [`GET ${base}/secrets?environment=dev-b2`]: {
          $status: 404,
          $text: "this app has no environment dev-b2"
        },
        [`GET ${base}/invocations/i-1/held`]: { $status: 404, $text: "" }
      }
    );
    const said = (id: number) => {
      expect(frames.find((f) => f.id === id)?.result?.isError).toBe(true);
      return textOf(frames, id);
    };

    expect(said(2)).toMatch(/cannot store a value shaped like an Oxy credential/);
    expect(said(2)).toMatch(/\[exit 6 REQUEST\] server code: credential_shaped_value/);
    // The value that was refused is never quoted back to the model.
    expect(said(2)).not.toContain(SANDBOX_TOKEN);

    expect(said(3)).toMatch(/this app has no environment dev-b2/);
    expect(said(3)).toMatch(/\[exit 5 NOT_FOUND\] server code: environment_not_found/);

    expect(said(4)).toMatch(/counting those still being deleted/);
    expect(said(4)).toMatch(/\[exit 6 REQUEST\] server code: token_sandbox_limit/);
    expect(said(4)).toMatch(/Do not retry in a loop/);

    expect(said(5)).toMatch(/this app has no environment dev-b2/);
    expect(said(5)).toMatch(/\[exit 5 NOT_FOUND\]/);

    // A bare 404: no reason to give, so the status line and what to do instead.
    expect(said(6)).toMatch(/^404 /);
    expect(said(6)).toMatch(/anything else answers 404/);
    expect(said(6)).toMatch(/\[exit 5 NOT_FOUND\]$/);
  });

  it("oxy_token_revoke refuses without confirm=true, and revokes with it", async () => {
    const { frames } = await served(
      [INIT, call(2, "oxy_token_revoke"), call(3, "oxy_token_revoke", { confirm: true })],
      { ...A1, "DELETE /api/auth/token": {} }
    );
    expect(frames.find((f) => f.id === 2)?.result?.isError).toBe(true);
    expect(textOf(frames, 2)).toMatch(/needs confirm=true/);
    expect(JSON.parse(textOf(frames, 3))).toMatchObject({ revoked: true });
  });

  it("exits 4 at startup when the deployment no longer accepts the token", async () => {
    const { frames, stderr, code } = await served([INIT, LIST], {
      "GET /api/auth/token": { $status: 401 }
    });
    expect(code).toBe(4);
    expect(frames).toEqual([]);
    expect(stderr).toMatch(/no longer accepted/);
    expect(stderr).toMatch(/do not look for another credential/);
  });

  /**
   * The server records the user agent on audit rows and in the token's usage.
   * ` mcp` is what tells a tool call from the same agent shelling out, and
   * `agent/<label>` what tells the agent from its operator.
   */
  it("marks every request as made through mcp, by the agent that set OXY_AGENT", async () => {
    const { url, close, userAgents } = await fakeServer(A1);
    try {
      await session([INIT, call(2, "oxy_whoami")], {
        args: ["--target", url],
        token: SANDBOX_TOKEN,
        env: { OXY_AGENT: "Store Ops Agent" }
      });
    } finally {
      await close();
    }
    // The startup check and the tool call, both.
    expect(userAgents.length).toBeGreaterThanOrEqual(2);
    for (const agent of userAgents) {
      expect(agent).toMatch(/^oxyc\/\d+\.\d+\.\d+ agent\/store-ops-agent mcp$/);
    }
  });

  it("is never offered oxy_token_revoke on a person's login", async () => {
    const frames = await rpc([INIT, call(2, "oxy_token_revoke", { confirm: true })]);
    expect(frames.find((f) => f.id === 2)?.result?.isError).toBe(true);
    expect(textOf(frames, 2)).toMatch(/unknown tool/);
  });
});
