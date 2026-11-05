/**
 * `oxyc flags`, driven through the real binary against a fake deployment.
 *
 * A LOCAL SERVER, for `assume.test.ts`'s reason: what is under test is the wire
 * contract with `/api/admin/feature-flags*` and `/api/orgs/{id}/feature-flags`
 * — the paths, the PATCH bodies (a read-modify-write, not a retype), that
 * nothing is sent unconfirmed, and that a refusal is printed in the server's
 * own words with the contract's exit code.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";
import type { FeatureFlag } from "./flags-rules.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const ADMIN = "/api/admin/feature-flags";
const ACME = "11111111-1111-4111-8111-111111111111";
const GLOBEX = "22222222-2222-4222-8222-222222222222";
const ALICE = "33333333-3333-4333-8333-333333333333";

function fresh(): FeatureFlag[] {
  return [
    {
      key: "checkout_v2",
      description: "New checkout",
      default: false,
      enabled: true,
      updated_at: "2026-11-01T00:00:00Z",
      variants: ["control", "treatment"],
      unit: "org",
      per_org: true,
      client: true,
      targeting: {
        users: {},
        orgs: { [ACME]: "treatment" },
        fallthrough: { type: "split", weights: { control: 5000, treatment: 5000 } }
      },
      targeting_error: null
    },
    {
      key: "billing",
      description: "Billing",
      default: false,
      enabled: false,
      updated_at: null,
      variants: ["off", "on"],
      unit: "org",
      per_org: false,
      client: false,
      targeting: null,
      targeting_error: null
    },
    {
      key: "broken",
      description: "",
      default: false,
      enabled: true,
      updated_at: null,
      variants: ["off", "on"],
      unit: "org",
      per_org: true,
      client: false,
      targeting: null,
      targeting_error: "weights sum to 9000"
    }
  ];
}

let server: Server;
let base: string;
let flags: FeatureFlag[] = fresh();
let requests: Array<{ method: string; url: string; body: string }> = [];

beforeEach(() => {
  flags = fresh();
  requests = [];
});

function json(res: ServerResponse, value: unknown, status = 200): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

function patch(key: string, body: string, res: ServerResponse) {
  const flag = flags.find((f) => f.key === key);
  if (!flag) return json(res, { error: "unknown flag" }, 404);
  const patchBody = JSON.parse(body) as Partial<FeatureFlag>;
  if ("targeting" in patchBody && patchBody.targeting && !flag.per_org) {
    return json(res, { error: `${key} is read without an org, so rules would do nothing` }, 422);
  }
  Object.assign(flag, patchBody);
  json(res, flag);
}

function route(method: string, url: string, body: string, res: ServerResponse) {
  if (method === "GET" && url === ADMIN) return json(res, flags);
  if (method === "PATCH" && url.startsWith(`${ADMIN}/`)) {
    return patch(decodeURIComponent(url.slice(ADMIN.length + 1)), body, res);
  }
  if (method === "GET" && url.startsWith(`${ADMIN}/evaluate`)) {
    return json(res, [
      { key: "checkout_v2", variant: "treatment", reason: "TARGETING_MATCH" },
      { key: "billing", variant: "off", reason: "DISABLED", bucket: 1234 }
    ]);
  }
  if (method === "GET" && url === "/api/orgs") {
    return json(res, [
      { id: ACME, slug: "acme" },
      { id: GLOBEX, slug: "globex" }
    ]);
  }
  if (method === "GET" && url === `/api/orgs/${ACME}/feature-flags`) {
    return json(res, { flags: { checkout_v2: { variant: "treatment", enabled: true } } });
  }
  res.writeHead(404);
  res.end("{}");
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
      route(method, url, body, res);
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

function scratch(): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-flags-"));
  SCRATCH.push(dir);
  return dir;
}

/** Async for `assume.test.ts`'s reason: `spawnSync` would block the server above. */
function oxyc(...args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = scratch();
  return new Promise((done) => {
    // stdin is a pipe, not a terminal — which is what an unconfirmed write refuses on.
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

const sent = () => requests.map((q) => `${q.method} ${q.url}`);
const patches = () => requests.filter((q) => q.method === "PATCH");

describe("flags list / show", () => {
  it("list summarises each flag's switch and rules", async () => {
    const r = await oxyc("flags", "list");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${ADMIN}`]);
    expect(r.stdout).toMatch(/checkout_v2[\s|]+on[\s|]+1 org, split control 50% \/ treatment 50%/);
    expect(r.stdout).toMatch(/billing[\s|]+off[\s|]+global only/);
    expect(r.stdout).toContain("UNREADABLE RULES");
  });

  it("--json is the server's array", async () => {
    const r = await oxyc("flags", "list", "--json");
    expect(JSON.parse(r.stdout)).toHaveLength(3);
  });

  it("show prints the rules and the rules error", async () => {
    const ok = await oxyc("flags", "show", "checkout_v2");
    expect(ok.status, ok.stderr).toBe(0);
    expect(ok.stdout).toContain(ACME);
    expect(ok.stdout).toContain("split control 50% / treatment 50%");
    const broken = await oxyc("flags", "show", "broken");
    expect(broken.stdout).toContain("UNREADABLE RULES: weights sum to 9000");
  });

  it("show of an unknown flag is NOT_FOUND and lists the real keys", async () => {
    const r = await oxyc("flags", "show", "nope");
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toContain("checkout_v2");
  });
});

describe("flags set", () => {
  it("refuses to write without --yes when stdin is not a terminal", async () => {
    const r = await oxyc("flags", "set", "billing", "--on");
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(patches()).toEqual([]);
  });

  it("sends only the kill switch", async () => {
    const r = await oxyc("flags", "set", "billing", "--on", "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(patches().map((p) => [p.url, JSON.parse(p.body)])).toEqual([
      [`${ADMIN}/billing`, { enabled: true }]
    ]);
    expect(r.stdout).toContain("kill switch on");
  });

  it("sends nothing when the switch is already there", async () => {
    const r = await oxyc("flags", "set", "billing", "--off", "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(patches()).toEqual([]);
  });

  it("needs exactly one of --on / --off", async () => {
    expect((await oxyc("flags", "set", "billing", "--yes")).status).toBe(ExitCode.USAGE);
    expect((await oxyc("flags", "set", "billing", "--on", "--off", "--yes")).status).toBe(
      ExitCode.USAGE
    );
  });
});

describe("flags rules", () => {
  it("adds one org override by slug and sends the other rules back unchanged", async () => {
    const r = await oxyc("flags", "rules", "checkout_v2", "--set-org", "globex=control", "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toContain("GET /api/orgs");
    expect(patches()).toHaveLength(1);
    expect(JSON.parse(patches()[0]?.body ?? "")).toEqual({
      targeting: {
        users: {},
        orgs: { [ACME]: "treatment", [GLOBEX]: "control" },
        fallthrough: { type: "split", weights: { control: 5000, treatment: 5000 } }
      }
    });
  });

  it("accepts a UUID org without a lookup, a user override and a split in percent", async () => {
    const r = await oxyc(
      "flags",
      "rules",
      "checkout_v2",
      "--set-org",
      `${GLOBEX}=treatment`,
      "--set-user",
      `${ALICE}=control`,
      "--split",
      "control=90,treatment=10",
      "--yes"
    );
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).not.toContain("GET /api/orgs");
    expect(JSON.parse(patches()[0]?.body ?? "").targeting).toEqual({
      users: { [ALICE]: "control" },
      orgs: { [ACME]: "treatment", [GLOBEX]: "treatment" },
      fallthrough: { type: "split", weights: { control: 9000, treatment: 1000 } }
    });
  });

  it("removes an override with `org=` and with --remove-org", async () => {
    const a = await oxyc("flags", "rules", "checkout_v2", "--set-org", "acme=", "--yes");
    expect(a.status, a.stderr).toBe(0);
    expect(JSON.parse(patches()[0]?.body ?? "").targeting.orgs).toEqual({});
    flags = fresh();
    const b = await oxyc("flags", "rules", "checkout_v2", "--remove-org", ACME, "--yes");
    expect(b.status, b.stderr).toBe(0);
    expect(JSON.parse(patches()[1]?.body ?? "").targeting.orgs).toEqual({});
  });

  it("--clear sends targeting: null", async () => {
    const r = await oxyc("flags", "rules", "checkout_v2", "--clear", "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(patches()[0]?.body ?? "")).toEqual({ targeting: null });
  });

  it("--file replaces the rules", async () => {
    const file = join(scratch(), "rules.json");
    writeFileSync(
      file,
      JSON.stringify({ users: {}, orgs: {}, fallthrough: { type: "variant", variant: "control" } })
    );
    const r = await oxyc("flags", "rules", "checkout_v2", "--file", file, "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(patches()[0]?.body ?? "").targeting).toEqual({
      users: {},
      orgs: {},
      fallthrough: { type: "variant", variant: "control" }
    });
  });

  it("--dry-run prints the rules and sends no write", async () => {
    const r = await oxyc("flags", "rules", "checkout_v2", "--everyone", "treatment", "--dry-run");
    expect(r.status, r.stderr).toBe(0);
    expect(patches()).toEqual([]);
    expect(r.stdout).toContain("dry run");
    expect(r.stdout).toContain("everyone else: treatment");
  });

  it("refuses to write without --yes", async () => {
    const r = await oxyc("flags", "rules", "checkout_v2", "--everyone", "control");
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(patches()).toEqual([]);
  });

  it("is a usage error with no edit, a bad split or an unknown variant — before any write", async () => {
    for (const args of [
      ["checkout_v2", "--yes"],
      ["checkout_v2", "--split", "control=50,treatment=40", "--yes"],
      ["checkout_v2", "--everyone", "on", "--yes"],
      ["checkout_v2", "--clear", "--everyone", "control", "--yes"]
    ]) {
      const r = await oxyc("flags", "rules", ...args);
      expect(r.status, args.join(" ")).toBe(ExitCode.USAGE);
    }
    expect(patches()).toEqual([]);
  });

  it("prints the server's 422 plainly", async () => {
    const r = await oxyc("flags", "rules", "billing", "--everyone", "on", "--yes");
    expect(r.status).toBe(ExitCode.REQUEST);
    expect(r.stderr).toContain("the server refused these rules");
    expect(r.stderr).toContain("so rules would do nothing");
  });

  it("will not edit unreadable stored rules, but clears them", async () => {
    const refused = await oxyc("flags", "rules", "broken", "--set-org", `${ACME}=on`, "--yes");
    expect(refused.status).toBe(ExitCode.REFUSED);
    expect(refused.stderr).toContain("weights sum to 9000");
    expect(patches()).toEqual([]);
    const cleared = await oxyc("flags", "rules", "broken", "--clear", "--yes");
    expect(cleared.status, cleared.stderr).toBe(0);
  });

  it("warns that rules sit behind an off kill switch", async () => {
    flags[0] = { ...(flags[0] as FeatureFlag), enabled: false };
    const r = await oxyc("flags", "rules", "checkout_v2", "--everyone", "control", "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("kill switch is off");
  });
});

describe("flags eval / org", () => {
  it("eval resolves the org slug and passes both ids as query", async () => {
    const r = await oxyc("flags", "eval", "--org", "acme", "--user", ALICE);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toContain(`GET ${ADMIN}/evaluate?org_id=${ACME}&user_id=${ALICE}`);
    expect(r.stdout).toMatch(/checkout_v2[\s|]+treatment[\s|]+TARGETING_MATCH/);
    expect(r.stdout).toMatch(/billing[\s|]+off[\s|]+DISABLED[\s|]+1234/);
  });

  it("org reads the member-facing route", async () => {
    const r = await oxyc("flags", "org", "acme");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toContain(`GET /api/orgs/${ACME}/feature-flags`);
    expect(r.stdout).toMatch(/checkout_v2[\s|]+treatment[\s|]+yes/);
  });

  it("an unknown org slug is NOT_FOUND", async () => {
    const r = await oxyc("flags", "org", "nobody");
    expect(r.status).toBe(ExitCode.NOT_FOUND);
  });
});
