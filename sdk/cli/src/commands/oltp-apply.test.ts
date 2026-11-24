/**
 * `oxyc oltp apply`, driven through the real binary against a fake deployment
 * — `oltp-audit.test.ts`'s shape, for the verb that replaced the Rust
 * `oxy oltp apply`.
 *
 * What is pinned: the path and the body it sends (always JSON, `{}` or
 * `{"workspace_id"}`), the report a script parses (`--json` is the server's
 * body), and the exit codes: 8 without `--yes` with nothing sent, 9 when a
 * migration ran and failed (`--json` included), 6 for a request the server
 * refused, 7 when another apply holds the lock, 5 for no database.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";
import { type ApplyResponse, applyPath, renderApply } from "./oltp-apply.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});
function scratch(prefix: string): string {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  SCRATCH.push(dir);
  return dir;
}

const ORG_ID = "11111111-2222-3333-4444-555555555555";
const WS_ID = "66666666-7777-8888-9999-aaaaaaaaaaaa";
const REV_ID = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
const OLTP = `/api/admin/orgs/${ORG_ID}/oltp`;
let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string }> = [];
let applyStatus = 200;
let answer: ApplyResponse;

function applied(): ApplyResponse {
  return {
    org_id: ORG_ID,
    workspace_id: WS_ID,
    revision_id: REV_ID,
    applied: ["schemas/0003_bookings.sql"],
    already_applied: 2,
    failed: null,
    timed_out: false,
    ok: true
  };
}

function failedApply(): ApplyResponse {
  return {
    ...applied(),
    applied: [],
    failed: { file_path: "schemas/0003_bookings.sql", message: 'syntax error at or near "TABL"' },
    ok: false
  };
}

beforeEach(() => {
  requests = [];
  applyStatus = 200;
  answer = applied();
});

function json(res: import("node:http").ServerResponse, value: unknown): void {
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

beforeAll(async () => {
  server = createServer((req, res) => {
    let body = "";
    req.on("data", (c) => {
      body += c;
    });
    req.on("end", () => {
      const url = req.url ?? "";
      const method = req.method ?? "";
      requests.push({ method, url, body });
      if (url.startsWith("/api/orgs")) return json(res, [{ slug: "acme", id: ORG_ID }]);
      if (method === "POST" && url === `${OLTP}/apply`) {
        if (applyStatus !== 200) {
          res.writeHead(applyStatus, { "content-type": "text/plain" });
          return res.end("refused");
        }
        return json(res, answer);
      }
      res.writeHead(404);
      res.end("{}");
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

/** Async for `assume.test.ts`'s reason: `spawnSync` would block the server above. */
function oxyc(args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = scratch("oxyc-oltp-apply-");
  return new Promise((done) => {
    const child = spawn(process.execPath, [BIN, ...args, "--target", base], {
      env: {
        ...process.env,
        HOME: home,
        OXY_CREDENTIALS_PATH: join(home, "credentials.json"),
        OXYC_CACHE_DIR: join(home, "cache"),
        OXY_TOKEN: "test-token",
        NO_COLOR: "1"
      }
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => {
      stdout += d;
    });
    child.stderr.on("data", (d) => {
      stderr += d;
    });
    child.on("close", (status) => done({ status: status ?? -1, stdout, stderr }));
  });
}

/** Requests past the org lookup — what the verb itself sent. */
const sent = () =>
  requests.filter((q) => !q.url.startsWith("/api/orgs")).map((q) => `${q.method} ${q.url}`);
const applyBody = () => requests.find((q) => q.url === `${OLTP}/apply`)?.body;

describe("oltp apply", () => {
  it("POSTs `{}` to the org's apply route, prints what ran, and exits 0", async () => {
    const r = await oxyc(["oltp", "apply", "--org", "acme", "--yes"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(sent()).toEqual([`POST ${OLTP}/apply`]);
    expect(applyBody()).toBe("{}");
    expect(r.stdout).toContain("applied 1:");
    expect(r.stdout).toContain("    schemas/0003_bookings.sql");
    expect(r.stdout).toContain(`revision ${REV_ID} of workspace ${WS_ID}`);
  });

  it("sends --workspace as workspace_id", async () => {
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--workspace", WS_ID, "--yes"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(JSON.parse(applyBody() ?? "")).toEqual({ workspace_id: WS_ID });
  });

  it("says so when the store is already up to date", async () => {
    answer = { ...applied(), applied: [], already_applied: 3 };
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(r.stdout).toContain("already up to date (3 applied previously)");
  });

  it("refuses without --yes with exit 8, and posts nothing", async () => {
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID]);
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(sent()).toEqual([]);
    expect(r.stderr).toContain("apply needs --yes");
    expect(r.stderr).toContain(`oxyc oltp apply --org ${ORG_ID} --yes`);
  });

  it("exits 9 when a migration ran and failed, naming the file", async () => {
    answer = failedApply();
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes"]);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stdout).toContain("failed: schemas/0003_bookings.sql");
    expect(r.stderr).toContain("migration schemas/0003_bookings.sql failed");
    expect(r.stderr).toContain("syntax error");
  });

  it("--json is the server's body on stdout, and still exits 9 on a failure", async () => {
    answer = failedApply();
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes", "--json"]);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(JSON.parse(r.stdout)).toEqual(failedApply());
  });

  it("--json exits 0 on a clean apply", async () => {
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes", "--json"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(JSON.parse(r.stdout)).toEqual(applied());
  });

  it.each([
    [400, ExitCode.REQUEST],
    [409, ExitCode.REQUEST],
    [404, ExitCode.NOT_FOUND],
    [403, ExitCode.AUTH],
    [502, ExitCode.UNAVAILABLE],
    [503, ExitCode.UNAVAILABLE],
    [504, ExitCode.UNAVAILABLE]
  ])("maps a %i to exit %i, with nothing on stdout", async (status, code) => {
    applyStatus = status;
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes"]);
    expect(r.status).toBe(code);
    expect(r.stdout).toBe("");
  });

  it("explains a 400 as an ambiguous workspace, a 503 as a held lock and a 504 as a cut-off apply", async () => {
    applyStatus = 400;
    expect((await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes"])).stderr).toContain(
      "pass --workspace"
    );
    applyStatus = 503;
    expect((await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes"])).stderr).toContain(
      "another apply holds this org's lock"
    );
    applyStatus = 504;
    expect((await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes"])).stderr).toContain(
      "re-run it to continue"
    );
  });

  it("exits 7 on an apply the server's budget cut off, naming the files that landed", async () => {
    answer = { ...applied(), ok: false, timed_out: true };
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes"]);
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
    expect(r.stdout).toContain("applied 1:\n    schemas/0003_bookings.sql");
    expect(r.stdout).toContain("re-run to continue");
    expect(r.stderr).toContain("1 file(s) applied in this call and stay applied");
  });

  it("--json on a cut-off apply is the server's body, and still exits 7", async () => {
    answer = { ...applied(), ok: false, timed_out: true };
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes", "--json"]);
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
    expect(JSON.parse(r.stdout)).toEqual({ ...applied(), ok: false, timed_out: true });
  });

  it("names the files that applied before a failure", async () => {
    answer = { ...failedApply(), applied: ["schemas/0002_orders.sql"] };
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--yes"]);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stdout).toContain("applied 1:\n    schemas/0002_orders.sql");
    expect(r.stderr).toContain("1 file(s) before it applied in this call");
  });

  it("is a usage error before any request for a --workspace that is not a UUID", async () => {
    const r = await oxyc(["oltp", "apply", "--org", ORG_ID, "--workspace", "main", "--yes"]);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });
});

describe("renderApply", () => {
  it("lists the applied files and the failed one", () => {
    const text = renderApply({
      ...failedApply(),
      applied: ["schemas/0002_orders.sql"]
    });
    expect(text).toContain("applied 1:\n    schemas/0002_orders.sql");
    expect(text).toContain("failed: schemas/0003_bookings.sql");
  });

  it("builds the route path from the org", () => {
    expect(applyPath(ORG_ID)).toBe(`${OLTP}/apply`);
  });
});
