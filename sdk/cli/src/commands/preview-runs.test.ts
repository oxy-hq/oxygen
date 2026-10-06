/**
 * `oxyc preview run` / `oxyc preview runs` — held dry runs, driven with
 * `globalThis.fetch` stubbed, as `preview.test.ts` does for the rest of the
 * command group.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  requireRunKind,
  runPreviewRun,
  runPreviewRunShow,
  runPreviewRunsList
} from "./preview-runs.js";

const TARGET = "https://oxy.test";
const WORKSPACE = "11111111-2222-3333-4444-555555555555";

function fakeContext(opts: { bearer?: string } = {}): Context {
  return {
    cwd: "/tmp",
    flags: { env: "production", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => {
      if (opts.bearer) return opts.bearer;
      throw new CliError(`not authenticated for ${TARGET}`, {
        code: ExitCode.AUTH,
        hint: "oxyc login --env production"
      });
    },
    maybeBearer: async () => opts.bearer,
    storedBearer: () => opts.bearer,
    async credential() {
      return { token: await this.bearer(), source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({ workspace: WORKSPACE }),
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
      return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

const RUN_DETAIL_RUNNING = {
  run_id: "run-1",
  branch: "feature/x",
  kind: "procedure",
  target_ref: "automations/a.automation.yml",
  parent_run_id: null,
  revision_id: "rev-1",
  state: "running",
  outcome: null,
  held_count: 0,
  requested_by: "u-1",
  created_at: "2026-10-01T09:00:00.000Z",
  started_at: "2026-10-01T09:00:01.000Z",
  finished_at: null,
  agentic_run_id: "ar-1",
  error: null,
  steps: [],
  compare: null,
  sample: null
};

const RUN_DETAIL_FINISHED = {
  ...RUN_DETAIL_RUNNING,
  state: "finished",
  outcome: "succeeded",
  finished_at: "2026-10-01T09:00:05.000Z",
  steps: [
    { name: "step1", kind: "execute_sql", status: "held", held: { op: "insert" }, redirected: null }
  ]
};

const RUN_DETAIL_FAILED = {
  ...RUN_DETAIL_RUNNING,
  state: "finished",
  outcome: "failed",
  finished_at: "2026-10-01T09:00:05.000Z",
  error: "step1 failed: syntax error"
};

const RUN_DETAIL_CANCELLED = {
  ...RUN_DETAIL_RUNNING,
  state: "finished",
  outcome: "cancelled",
  finished_at: "2026-10-01T09:00:05.000Z"
};

describe("requireRunKind", () => {
  it("accepts procedure and airway_sample", () => {
    expect(requireRunKind("procedure")).toBe("procedure");
    expect(requireRunKind("airway_sample")).toBe("airway_sample");
  });

  /**
   * `transform_build` and `compare` runs are queued by the server's own
   * change check — `POST /previews/runs` refuses them (`400 bad_request`),
   * so this is caught client-side before any request, naming the read-back
   * verb instead.
   */
  it("refuses transform_build and compare, naming where to read them back", () => {
    for (const kind of ["transform_build", "compare", "nonsense"]) {
      expect(() => requireRunKind(kind)).toThrow(CliError);
      try {
        requireRunKind(kind);
      } catch (e) {
        expect((e as CliError).code).toBe(ExitCode.USAGE);
      }
    }
  });
});

describe("runPreviewRun", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("refuses an unsupported kind before any request", async () => {
    stubFetch({}, calls);
    const err = await runPreviewRun(
      fakeContext({ bearer: "tok" }),
      "feature/x",
      "transform_build",
      "x",
      {
        json: true
      }
    ).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("submits a procedure run with its variables and read_live_only", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews/runs`]: () => ({
          status: 202,
          body: { run_id: "run-1", state: "queued" }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runPreviewRun(
      fakeContext({ bearer: "tok" }),
      "feature/x",
      "procedure",
      "a.automation.yml",
      {
        variables: '{"x":1}',
        readLiveOnly: true,
        json: true
      }
    );
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual({ run_id: "run-1", state: "queued" });
    const body = JSON.parse(calls[0]?.body ?? "{}");
    expect(body).toEqual({
      branch: "feature/x",
      kind: "procedure",
      ref: "a.automation.yml",
      variables: { x: 1 },
      read_live_only: true,
      window: undefined,
      resources: []
    });
  });

  it("submits an airway_sample run with its window and resources", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews/runs`]: () => ({
          status: 202,
          body: { run_id: "run-2", state: "queued" }
        })
      },
      calls
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    await runPreviewRun(
      fakeContext({ bearer: "tok" }),
      "feature/x",
      "airway_sample",
      "p.airway.yml",
      {
        windowFrom: "2026-09-01T00:00:00Z",
        windowTo: "2026-09-08T00:00:00Z",
        resource: ["orders", "customers"],
        json: true
      }
    );
    write.mockRestore();

    const body = JSON.parse(calls[0]?.body ?? "{}");
    expect(body).toMatchObject({
      kind: "airway_sample",
      window: { from: "2026-09-01T00:00:00Z", to: "2026-09-08T00:00:00Z" },
      resources: ["orders", "customers"]
    });
  });

  it("rejects invalid JSON in --variables before any request", async () => {
    stubFetch({}, calls);
    const err = await runPreviewRun(
      fakeContext({ bearer: "tok" }),
      "feature/x",
      "procedure",
      "a.yml",
      {
        variables: "{not json",
        json: true
      }
    ).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("--wait submits, then polls the run to a finished state", async () => {
    let gets = 0;
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews/runs`]: () => ({
          status: 202,
          body: { run_id: "run-1", state: "queued" }
        }),
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => {
          gets += 1;
          return { status: 200, body: gets < 2 ? RUN_DETAIL_RUNNING : RUN_DETAIL_FINISHED };
        }
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runPreviewRun(fakeContext({ bearer: "tok" }), "feature/x", "procedure", "a.yml", {
      waitSeconds: 5,
      pollMs: 1,
      json: true
    });
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual(RUN_DETAIL_FINISHED);
    expect(gets).toBeGreaterThanOrEqual(2);
  });

  /**
   * A waited-for run that finishes `failed` is a failure, not a success —
   * `checks run` and `fn call` already exit non-zero on their own terminal
   * failures, and `oxy_preview_run` (the MCP tool wrapping this same
   * `previewRunGet`) already sets `isError` on it. The document still prints
   * first, so a CI script piping `--json` can inspect `error`/`steps` even on
   * the failure path.
   */
  it("--wait exits 1 on a failed run, printing the document first", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews/runs`]: () => ({
          status: 202,
          body: { run_id: "run-1", state: "queued" }
        }),
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => ({
          status: 200,
          body: RUN_DETAIL_FAILED
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    const err = await runPreviewRun(
      fakeContext({ bearer: "tok" }),
      "feature/x",
      "procedure",
      "a.yml",
      {
        waitSeconds: 5,
        pollMs: 1,
        json: true
      }
    ).catch((e: unknown) => e as CliError);
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual(RUN_DETAIL_FAILED);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.FAILURE);
  });

  /** `cancelled` is terminal-non-success too — e.g. the preview was deleted mid-run. */
  it("--wait exits 1 on a cancelled run", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews/runs`]: () => ({
          status: 202,
          body: { run_id: "run-1", state: "queued" }
        }),
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => ({
          status: 200,
          body: RUN_DETAIL_CANCELLED
        })
      },
      calls
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    const err = await runPreviewRun(
      fakeContext({ bearer: "tok" }),
      "feature/x",
      "procedure",
      "a.yml",
      {
        waitSeconds: 5,
        pollMs: 1,
        json: true
      }
    ).catch((e: unknown) => e as CliError);
    write.mockRestore();
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.FAILURE);
  });

  /** 400/409-shaped server refusals (bad kind, sandbox_required, …) map to exit 6. */
  it("maps a 409 sandbox_required refusal to exit 6", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews/runs`]: () => ({
          status: 409,
          body: { code: "sandbox_required", message: "register a sandbox company first" }
        })
      },
      calls
    );
    const err = await runPreviewRun(
      fakeContext({ bearer: "tok" }),
      "feature/x",
      "airway_sample",
      "p.yml",
      {
        json: true
      }
    ).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.REQUEST);
    expect((err as CliError).serverCode).toBe("sandbox_required");
  });

  /** `OXY_PREVIEW_RUNS` off: every runs route answers 404 `preview_runs_disabled`. */
  it("surfaces preview_runs_disabled (exit 5) when the deployment has the flag off", async () => {
    stubFetch(
      {
        [`POST /api/${WORKSPACE}/previews/runs`]: () => ({
          status: 404,
          body: { code: "preview_runs_disabled", message: "workspace preview runs are not enabled" }
        })
      },
      calls
    );
    const err = await runPreviewRun(
      fakeContext({ bearer: "tok" }),
      "feature/x",
      "procedure",
      "a.yml",
      {
        json: true
      }
    ).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.NOT_FOUND);
    expect((err as CliError).serverCode).toBe("preview_runs_disabled");
  });
});

describe("runPreviewRunsList / runPreviewRunShow", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("lists a branch's runs, of every kind", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/runs?branch=feature%2Fx`]: () => ({
          status: 200,
          body: [
            RUN_DETAIL_FINISHED,
            { ...RUN_DETAIL_FINISHED, run_id: "run-2", kind: "transform_build" }
          ]
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runPreviewRunsList(fakeContext({ bearer: "tok" }), "feature/x", { json: true });
    write.mockRestore();
    const parsed = JSON.parse(printed.trim());
    expect(parsed.runs).toHaveLength(2);
    expect(parsed.runs[1].kind).toBe("transform_build");
  });

  it("show fetches one run's detail, including its steps", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => ({
          status: 200,
          body: RUN_DETAIL_FINISHED
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runPreviewRunShow(fakeContext({ bearer: "tok" }), "run-1", { json: true });
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual(RUN_DETAIL_FINISHED);
  });

  it("show --wait polls to a finished state", async () => {
    let gets = 0;
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => {
          gets += 1;
          return { status: 200, body: gets < 2 ? RUN_DETAIL_RUNNING : RUN_DETAIL_FINISHED };
        }
      },
      calls
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    await runPreviewRunShow(fakeContext({ bearer: "tok" }), "run-1", {
      waitSeconds: 5,
      pollMs: 1,
      json: true
    });
    write.mockRestore();
    expect(gets).toBeGreaterThanOrEqual(2);
  });

  it("show --wait times out (exit 7) on a run that never finishes", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => ({
          status: 200,
          body: RUN_DETAIL_RUNNING
        })
      },
      calls
    );
    const err = await runPreviewRunShow(fakeContext({ bearer: "tok" }), "run-1", {
      waitSeconds: 0.05,
      pollMs: 10,
      json: true
    }).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.UNAVAILABLE);
  });

  it("show --wait exits 1 on a failed run, printing the document first", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => ({
          status: 200,
          body: RUN_DETAIL_FAILED
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    const err = await runPreviewRunShow(fakeContext({ bearer: "tok" }), "run-1", {
      waitSeconds: 5,
      pollMs: 1,
      json: true
    }).catch((e: unknown) => e as CliError);
    write.mockRestore();
    expect(JSON.parse(printed.trim())).toEqual(RUN_DETAIL_FAILED);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.FAILURE);
  });

  it("show --wait exits 1 on a cancelled run", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => ({
          status: 200,
          body: RUN_DETAIL_CANCELLED
        })
      },
      calls
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    const err = await runPreviewRunShow(fakeContext({ bearer: "tok" }), "run-1", {
      waitSeconds: 5,
      pollMs: 1,
      json: true
    }).catch((e: unknown) => e as CliError);
    write.mockRestore();
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.FAILURE);
  });

  /** A plain (non-waited) show is a read, not an "awaited outcome" — never exits on `outcome`. */
  it("show WITHOUT --wait does not exit non-zero on an already-failed run", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => ({
          status: 200,
          body: RUN_DETAIL_FAILED
        })
      },
      calls
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    await runPreviewRunShow(fakeContext({ bearer: "tok" }), "run-1", { json: true });
    write.mockRestore();
  });

  it("show --wait does not throw on a successful run", async () => {
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => ({
          status: 200,
          body: RUN_DETAIL_FINISHED
        })
      },
      calls
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    await runPreviewRunShow(fakeContext({ bearer: "tok" }), "run-1", {
      waitSeconds: 5,
      pollMs: 1,
      json: true
    });
    write.mockRestore();
  });

  /**
   * `previewRunGet` already holds a fresh (non-finished) snapshot by the time
   * it enters its poll loop — the loop must not immediately re-fetch the same
   * state before its first sleep. Modelled on wall-clock time (not a call
   * counter) because a call-count-based stub cannot distinguish "fetched
   * again instantly" from "fetched again after sleeping": both implementations
   * make the same number of calls against a stub that advances per call
   * rather than per elapsed time.
   */
  it("previewRunGet does not waste a GET before its first sleep", async () => {
    const start = Date.now();
    stubFetch(
      {
        [`GET /api/${WORKSPACE}/previews/runs/run-1`]: () => ({
          status: 200,
          body: Date.now() - start < 15 ? RUN_DETAIL_RUNNING : RUN_DETAIL_FINISHED
        })
      },
      calls
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    await runPreviewRunShow(fakeContext({ bearer: "tok" }), "run-1", {
      waitSeconds: 5,
      pollMs: 20,
      json: true
    });
    write.mockRestore();
    // 1 initial fetch (sees "running", elapsed ~0ms) + exactly 1 post-sleep
    // fetch (elapsed >= 20ms >= the 15ms threshold, sees "finished"). A
    // wasted immediate re-fetch before the first sleep would add a third
    // call that also lands under the 15ms threshold and still sees "running".
    expect(calls).toHaveLength(2);
  });
});
