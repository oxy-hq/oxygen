/**
 * `oxyc preview` — workspace previews, driven with `globalThis.fetch`
 * stubbed, as `env.test.ts` does for sandboxes.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  runPreviewChecks,
  runPreviewCreate,
  runPreviewDelete,
  runPreviewList,
  runPreviewShow
} from "./preview.js";

const TARGET = "https://oxy.test";
const WORKSPACE = "11111111-2222-3333-4444-555555555555";

/** `workspace: null` opts OUT of the default id, for the "unresolved {workspace}" case. */
function fakeContext(opts: { bearer?: string; workspace?: string | null } = {}): Context {
  const workspace = opts.workspace === null ? undefined : (opts.workspace ?? WORKSPACE);
  return {
    cwd: "/tmp",
    flags: { env: "production", tokenEnv: "OXY_TOKEN", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: () => {
      if (opts.bearer) return opts.bearer;
      throw new CliError(`not authenticated for ${TARGET}`, {
        code: ExitCode.AUTH,
        hint: "oxyc login --env production"
      });
    },
    maybeBearer: () => opts.bearer,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({ workspace }),
    withEnv: () => fakeContext(opts)
  };
}

interface Call {
  method: string;
  url: string;
  body?: string;
}

function stubFetch(
  routes: Record<string, (init: RequestInit | undefined) => { status: number; body: unknown }>,
  calls: Call[]
) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const url = String(input);
      const path = url.replace(TARGET, "");
      const method = (init?.method ?? "GET").toUpperCase();
      calls.push({ method, url, body: init?.body as string | undefined });
      const key = `${method} ${path}`;
      const handler = routes[key];
      if (!handler) {
        return new Response(
          JSON.stringify({ code: "not_stubbed", message: `no stub for ${key}` }),
          {
            status: 404
          }
        );
      }
      const { status, body } = handler(init);
      const text = typeof body === "string" ? body : JSON.stringify(body);
      // A 204 (DELETE /previews) may not carry a body at all — the Response
      // constructor refuses a non-null one on that status.
      return new Response(status === 204 || text === "" ? null : text, {
        status,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

const ITEM_COMPILING = {
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

const ITEM_READY = {
  ...ITEM_COMPILING,
  revision_id: "rev-1",
  status: "ready",
  compiled_at: "2026-10-01T09:05:00.000Z",
  checks: { status: "done", needs_reset: 0, warnings: 1, transforms: 2 }
};

describe("runPreviewCreate", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("POSTs {branch} and prints the PreviewItem with --json", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews`]: () => ({ status: 202, body: { item: ITEM_COMPILING } })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runPreviewCreate(fakeContext({ bearer: "tok" }), "feature/x", { json: true });
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual(ITEM_COMPILING);
    expect(calls[0]?.body).toBe(JSON.stringify({ branch: "feature/x" }));
  });

  it("without --wait, returns the compiling item rather than polling", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews`]: () => ({ status: 202, body: { item: ITEM_COMPILING } })
      },
      calls
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    await runPreviewCreate(fakeContext({ bearer: "tok" }), "feature/x", { json: true });
    write.mockRestore();
    // Only the POST — no poll of the list.
    expect(calls).toHaveLength(1);
  });

  it("--wait polls the list until the compile is ready", async () => {
    let gets = 0;
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews`]: () => ({
          status: 202,
          body: { item: ITEM_COMPILING }
        }),
        [`GET /api/${WORKSPACE}/previews`]: () => {
          gets += 1;
          return { status: 200, body: { items: [gets < 2 ? ITEM_COMPILING : ITEM_READY] } };
        }
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runPreviewCreate(fakeContext({ bearer: "tok" }), "feature/x", {
      json: true,
      waitSeconds: 5,
      pollMs: 1
    });
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual(ITEM_READY);
    expect(gets).toBeGreaterThanOrEqual(2);
  });

  it("--wait times out (exit 7) when the compile never finishes", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews`]: () => ({
          status: 202,
          body: { item: ITEM_COMPILING }
        }),
        [`GET /api/${WORKSPACE}/previews`]: () => ({
          status: 200,
          body: { items: [ITEM_COMPILING] }
        })
      },
      calls
    );
    const err = await runPreviewCreate(fakeContext({ bearer: "tok" }), "feature/x", {
      json: true,
      waitSeconds: 0.05,
      pollMs: 10
    }).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.UNAVAILABLE);
  });

  /**
   * A waited-for compile that reaches the terminal `failed` status is a
   * failure, not a success — `checks run` and `fn call` already exit non-zero
   * on their own terminal failures, and `oxy_preview_create` (the MCP tool
   * wrapping this same `previewCreate`) already sets `isError` on it. The
   * PRINTED document still has to be the full item (so `--json | jq .error`
   * works in a CI script even on the failure path) — only the exit code
   * changes.
   */
  it("exits 1 on a waited-for failed compile, printing the document first", async () => {
    const failedItem = { ...ITEM_READY, status: "failed", error: "syntax error in config.yml" };
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews`]: () => ({
          status: 202,
          body: { item: ITEM_COMPILING }
        }),
        [`GET /api/${WORKSPACE}/previews`]: () => ({ status: 200, body: { items: [failedItem] } })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    const err = await runPreviewCreate(fakeContext({ bearer: "tok" }), "feature/x", {
      json: true,
      waitSeconds: 5,
      pollMs: 1
    }).catch((e: unknown) => e as CliError);
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual(failedItem);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.FAILURE);
  });

  it("maps 409 cannot_compile to exit 6", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews`]: () => ({
          status: 409,
          body: { code: "cannot_compile", message: "uncommitted changes in the worktree" }
        })
      },
      calls
    );
    const err = await runPreviewCreate(fakeContext({ bearer: "tok" }), "feature/x", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.REQUEST);
    expect((err as CliError).serverCode).toBe("cannot_compile");
  });

  it("without --workspace, fails before any request naming the flag", async () => {
    stubFetch({}, calls);
    const err = await runPreviewCreate(
      fakeContext({ bearer: "tok", workspace: null }),
      "feature/x",
      {
        json: true
      }
    ).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect((err as CliError).message).toMatch(/\{workspace\}/);
    expect(calls).toHaveLength(0);
  });
});

describe("runPreviewList / runPreviewShow", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("lists every item", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews`]: () => ({ status: 200, body: { items: [ITEM_READY] } })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runPreviewList(fakeContext({ bearer: "tok" }), { json: true });
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual({ items: [ITEM_READY] });
  });

  it("show filters the list by branch and finds it", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews`]: () => ({
          status: 200,
          body: { items: [ITEM_READY, { ...ITEM_READY, branch: "other" }] }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runPreviewShow(fakeContext({ bearer: "tok" }), "feature/x", { json: true });
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual(ITEM_READY);
  });

  it("show synthesizes NOT_FOUND (exit 5) client-side — there is no GET-by-branch route", async () => {
    stubFetch(
      { [`GET /api/${WORKSPACE}/previews`]: () => ({ status: 200, body: { items: [] } }) },
      calls
    );
    const err = await runPreviewShow(fakeContext({ bearer: "tok" }), "feature/x", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.NOT_FOUND);
  });
});

describe("runPreviewDelete", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("--yes DELETEs ?branch= without asking", async () => {
    stubFetch(
      {
        [`DELETE /api/${WORKSPACE}/previews?branch=feature%2Fx`]: () => ({ status: 204, body: "" })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runPreviewDelete(fakeContext({ bearer: "tok" }), "feature/x", { yes: true, json: true });
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual({ branch: "feature/x", deleted: true });
  });

  /** No `--yes` and no TTY (true under vitest): refused, exit 8, same as `oxyc env delete`. */
  it("refuses off a TTY without --yes (exit 8)", async () => {
    stubFetch({}, calls);
    const err = await runPreviewDelete(fakeContext({ bearer: "tok" }), "feature/x", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.REFUSED);
    expect(calls).toHaveLength(0);
  });
});

describe("runPreviewChecks", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("GETs /previews/checks?branch= and prints the ChecksResponse", async () => {
    const response = {
      branch: "feature/x",
      revision_id: "rev-1",
      status: "done",
      error: null,
      pipelines: [{ name: "p" }],
      transforms: []
    };
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/checks?branch=feature%2Fx`]: () => ({
          status: 200,
          body: response
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runPreviewChecks(fakeContext({ bearer: "tok" }), "feature/x", { json: true });
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual(response);
  });

  it("maps 404 preview_not_found to exit 5", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/checks?branch=feature%2Fx`]: () => ({
          status: 404,
          body: { code: "preview_not_found", message: "there is no preview of branch feature/x" }
        })
      },
      calls
    );
    const err = await runPreviewChecks(fakeContext({ bearer: "tok" }), "feature/x", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.NOT_FOUND);
    expect((err as CliError).serverCode).toBe("preview_not_found");
  });
});
