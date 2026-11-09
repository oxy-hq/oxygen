/**
 * `oxyc bootstrap`, driven through the real binary against a fake deployment.
 *
 * A LOCAL SERVER, for `airhouse.test.ts`'s reason: what is under test is the
 * wire contract — which routes the verify pass reads, that `plan` reads
 * nothing at all, that a run refuses before it writes when an assume session
 * is live, and that the exit code says whether the account is actually stood
 * up. None of that is observable from the pure functions.
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

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const ORG_ID = "11111111-2222-3333-4444-555555555555";
const WS_BLANK = "22222222-2222-3333-4444-555555555555";
const WS_REPO = "33333333-2222-3333-4444-555555555555";
const APP_ID = "44444444-2222-3333-4444-555555555555";
const SLUG = "poke-house";

/** What the fake deployment is pretending to be, reset per test. */
interface World {
  orgExists: boolean;
  assuming: boolean;
  /** Make every `/api/admin/*` read answer this status, for the failure paths. */
  adminStatus?: number;
  /** Status for the billing route alone — it has its own guard (Global Owner). */
  billingStatus?: number;
  subscription: "active" | "none" | "past_due";
  oltpProvisioned: boolean;
  stagingBranch: boolean;
  workspaceWithRemote: boolean;
  compiled: boolean;
  airhouse: "none" | "unbound" | "ready";
  appExists: boolean;
  appServing: boolean;
  missingSecrets: number;
}

let world: World;
let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string }> = [];

function fresh(): World {
  return {
    orgExists: true,
    assuming: false,
    subscription: "active",
    oltpProvisioned: true,
    stagingBranch: true,
    workspaceWithRemote: true,
    compiled: true,
    airhouse: "ready",
    appExists: true,
    appServing: true,
    missingSecrets: 0
  };
}

beforeEach(() => {
  world = fresh();
  requests = [];
});

function json(res: import("node:http").ServerResponse, value: unknown, status = 200): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

function workspaceRows(): unknown[] {
  const blank = { id: WS_BLANK, name: "Default", status: "Ready", org_slug: SLUG };
  if (!world.workspaceWithRemote) return [blank];
  return [blank, { id: WS_REPO, name: "pokehouse-oxy", status: "Ready", org_slug: SLUG }];
}

function workspaceDetail(id: string): unknown {
  const row = workspaceRows().find((w) => (w as { id: string }).id === id);
  if (!row) return undefined;
  const remote = id === WS_REPO ? "https://github.com/oxy-hq/pokehouse-oxy.git" : null;
  return {
    ...(row as object),
    git_remote_url: remote,
    current_revision_id: remote && world.compiled ? "rev-1" : null
  };
}

function airhouseRow(): unknown {
  return {
    workspace_id: WS_REPO,
    workspace_name: "pokehouse-oxy",
    org_id: ORG_ID,
    org_name: "Poke House",
    status: world.airhouse === "none" ? "none" : "active",
    tenant_id: world.airhouse === "none" ? "" : `oxy-ws-${WS_REPO}`,
    bucket: "oxy-airhouse",
    prefix: `tenants/${WS_REPO}`,
    service_account_ready: world.airhouse === "ready",
    sa_rotated_at: null,
    created_at: "2026-10-01T00:00:00+00:00",
    service_account_id: "sa-1",
    sa_created_at: null,
    bearer_max_role: "admin",
    bearer_max_ttl_secs: 3600
  };
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
      const path = url.split("?")[0] ?? "";
      requests.push({ method, url, body });

      if (method === "GET" && path === "/api/assume/current") {
        return json(res, world.assuming ? [{ org_slug: SLUG, reason: "work" }] : []);
      }
      // `block_admin_while_acting` refuses the whole admin surface while the
      // caller is acting as a tenant. The fake used to answer 200 regardless,
      // which is why it passed a `run` that in reality died on the first
      // admin read with exit 4 long before reaching the refusal.
      if (world.assuming && path.startsWith("/api/admin/")) {
        return json(res, { error: "admin surface refused while acting as a tenant" }, 403);
      }
      if (world.adminStatus && path.startsWith("/api/admin/")) {
        return json(res, { error: "boom" }, world.adminStatus);
      }
      if (method === "GET" && path === "/api/admin/orgs-meta") {
        return json(res, {
          items: world.orgExists
            ? [
                {
                  id: ORG_ID,
                  name: "Poke House",
                  slug: SLUG,
                  workspace_count: workspaceRows().length,
                  owner_email: "ops@poke.example"
                }
              ]
            : []
        });
      }
      if (method === "GET" && path === "/api/orgs") {
        // The real handler lists the assumed org while a session is live.
        return json(
          res,
          world.assuming ? [{ id: ORG_ID, name: "Poke House", slug: SLUG, role: "member" }] : []
        );
      }
      if (method === "POST" && path === "/api/admin/orgs") {
        world.orgExists = true;
        return json(res, {
          org: { id: ORG_ID, slug: SLUG, name: "Poke House" },
          owner_status: "invited",
          owner_email: "ops@poke.example",
          default_workspace_id: WS_BLANK
        });
      }
      if (path === `/api/admin/orgs/${ORG_ID}/billing/subscription`) {
        // `oxy_owner_guard_middleware` refuses everyone but the Global Owner,
        // so this route answers 403 even to a caller the rest of `/admin`
        // serves — which is why it has its own switch.
        if (world.billingStatus) return json(res, { error: "owner only" }, world.billingStatus);
        if (world.subscription === "none") return json(res, { code: "no_subscription" }, 404);
        return json(res, {
          id: "sub_1",
          status: world.subscription,
          cancel_at_period_end: false
        });
      }
      if (method === "GET" && path === `/api/admin/orgs/${ORG_ID}/oltp`) {
        return json(res, {
          is_provisioned: world.oltpProvisioned,
          database: "oxy_org_poke",
          host: "db.example",
          provider: "neon",
          project_name: "poke",
          region: "us-west-2",
          status: "active",
          analyst_role: "oxy_analyst_ro",
          analyst_ready: true,
          platform_schema_version: 1,
          expected_platform_schema_version: 1,
          schemas: world.oltpProvisioned
            ? [{ schema: "app_poke_house", kind: "app", writer_name: "poke_house_rw" }]
            : []
        });
      }
      if (path === `/api/admin/orgs/${ORG_ID}/oltp/branches/staging`) {
        return json(res, {
          branch: "staging",
          provisioned: world.stagingBranch,
          database: "oxy_org_poke_staging",
          host: "db.example"
        });
      }
      if (method === "GET" && path === "/api/admin/workspaces-meta") {
        return json(res, { items: workspaceRows() });
      }
      const detail = /^\/api\/admin\/workspaces\/([^/]+)\/detail$/.exec(path);
      if (method === "GET" && detail) {
        const found = workspaceDetail(detail[1] as string);
        return found ? json(res, found) : json(res, {}, 404);
      }
      if (method === "GET" && path === "/api/admin/airhouse") {
        return json(res, {
          rows: [airhouseRow()],
          truncated: { unprovisioned: false, provisioned: false }
        });
      }
      if (method === "POST" && path === `/api/admin/workspaces/${WS_REPO}/airhouse/provision`) {
        world.airhouse = "ready";
        return json(res, airhouseRow());
      }
      if (method === "GET" && path === "/api/customer-apps") {
        return json(res, {
          items: world.appExists
            ? [{ id: APP_ID, slug: SLUG, org_slug: SLUG, name: "Poke House" }]
            : []
        });
      }
      if (method === "GET" && path === `/api/customer-apps/${APP_ID}/builds`) {
        return json(res, {
          builds: [
            {
              id: "b1",
              build_id: "build-1",
              created_at: "2026-10-02T00:00:00+00:00",
              is_draft: true,
              is_published: world.appServing,
              published_by_email: "ops@poke.example",
              published_via: null,
              source_repo: null,
              commit_sha: "abcdef1234",
              source_branch: "main"
            }
          ]
        });
      }
      if (method === "GET" && path === `/api/customer-apps/${APP_ID}/secrets`) {
        return json(res, {
          environment: "production",
          entries: [],
          missing_required: world.missingSecrets
        });
      }
      if (method === "GET" && path === `/api/orgs/${ORG_ID}/frontline/workers`) {
        return json(res, []);
      }
      if (method === "GET" && path === `/api/orgs/${ORG_ID}/frontline/devices`) {
        return json(res, []);
      }
      res.writeHead(404, { "content-type": "application/json" });
      res.end("{}");
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

/** Async for `airhouse.test.ts`'s reason: `spawnSync` would block the server. */
function oxyc(...args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = mkdtempSync(join(tmpdir(), "oxyc-bootstrap-"));
  SCRATCH.push(home);
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

describe("bootstrap plan", () => {
  it("prints the whole sequence and makes NO request", async () => {
    const r = await oxyc("bootstrap", "plan", SLUG);
    expect(r.status, r.stderr).toBe(0);
    expect(requests).toEqual([]);
    for (const phase of [
      "repository",
      "organization",
      "subscription",
      "OLTP Postgres",
      "workspace that is the customer's repository",
      "Airhouse tenant",
      "first app published",
      "declared secret"
    ]) {
      expect(r.stdout).toContain(phase);
    }
  });

  it("names the stage boundaries, because the credential changes at each", async () => {
    const r = await oxyc("bootstrap", "plan", SLUG, "--crews");
    expect(r.stdout).toContain("stage 1 · local");
    expect(r.stdout).toContain("oxyc assume start");
    // Two crossings into the tenant surface: the import, then the crew.
    expect(r.stdout.match(/· tenant —/g)).toHaveLength(2);
  });

  it("leaves the frontline phase out unless --crews", async () => {
    expect((await oxyc("bootstrap", "plan", SLUG)).stdout).not.toContain("crew identities");
    expect((await oxyc("bootstrap", "plan", SLUG, "--crews")).stdout).toContain("crew identities");
  });

  it("states why each manual phase is manual", async () => {
    const r = await oxyc("bootstrap", "plan", SLUG);
    expect(r.stdout).toContain("manual because it buys a billable project");
    expect(r.stdout).toContain("manual because the authority is owner-only");
  });

  it("checks the invocation before anything else", async () => {
    const r = await oxyc("bootstrap", "plan", "Poke House");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
    expect(r.stdout).toBe("");
  });
});

describe("bootstrap verify", () => {
  it("exits 0 on an account that is fully stood up", async () => {
    const r = await oxyc("bootstrap", "verify", SLUG);
    expect(r.status, `${r.stdout}\n${r.stderr}`).toBe(0);
    expect(r.stdout).toContain("ok");
    expect(r.stdout).toContain(`oxy-ws-${WS_REPO}`);
    expect(r.stderr).toContain("every phase read as landed");
    // The repo stays on the checklist even then: it is a human step whose
    // landing this pass cannot read, and dropping it would lose the one
    // reminder that a scaffold still has to be pushed.
    expect(r.stderr).toContain("the customer's workspace repository exists");
    expect(r.stderr).not.toContain("MISSING");
  });

  it("reads the routes that carry each phase's fact, and nothing else", async () => {
    await oxyc("bootstrap", "verify", SLUG);
    const seen = requests.map((q) => `${q.method} ${q.url.split("?")[0]}`);
    expect(seen).toContain("GET /api/assume/current");
    expect(seen).toContain("GET /api/admin/orgs-meta");
    expect(seen).toContain(`GET /api/admin/orgs/${ORG_ID}/billing/subscription`);
    expect(seen).toContain(`GET /api/admin/orgs/${ORG_ID}/oltp`);
    expect(seen).toContain(`GET /api/admin/orgs/${ORG_ID}/oltp/branches/staging`);
    expect(seen).toContain("GET /api/admin/workspaces-meta");
    // `git_remote_url` lives only on the detail read — the listing cannot say.
    expect(seen).toContain(`GET /api/admin/workspaces/${WS_REPO}/detail`);
    expect(seen).toContain("GET /api/admin/airhouse");
    expect(requests.every((q) => q.method === "GET")).toBe(true);
  });

  it("exits 9 and names the phase when a workspace has no remote", async () => {
    world.workspaceWithRemote = false;
    const r = await oxyc("bootstrap", "verify", SLUG);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stdout).toContain("git-link");
    expect(r.stderr).toContain("not one with a remote");
    expect(r.stderr).toContain("onboarding/github");
  });

  it("calls out a missing subscription as the thing that 402s the import", async () => {
    world.subscription = "none";
    const r = await oxyc("bootstrap", "verify", SLUG);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stderr).toContain("billing-Incomplete");
    expect(r.stderr).toContain("provision-subscription");
  });

  it("separates 'no tenant' from 'tenant with no service account'", async () => {
    world.airhouse = "unbound";
    const r = await oxyc("bootstrap", "verify", SLUG);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stderr).toContain("service account is not bound");
  });

  it("reports a per-environment secret gap, which a publish never refuses on", async () => {
    world.missingSecrets = 2;
    const r = await oxyc("bootstrap", "verify", SLUG);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stderr).toMatch(/production: 2 required key\(s\) unset/);
  });

  it("says an app is registered but serving nothing, which is not the same as absent", async () => {
    world.appServing = false;
    const r = await oxyc("bootstrap", "verify", SLUG);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stderr).toContain("no build being served");
    expect(r.stderr).toContain("staging_never_served");
  });

  it("reports an absent org as missing rather than failing the read", async () => {
    world.orgExists = false;
    const r = await oxyc("bootstrap", "verify", SLUG);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stdout).toContain("org");
    expect(r.stderr).toContain("no organization with slug 'poke-house'");
  });

  it("--json emits one document per phase, with nothing styled", async () => {
    const r = await oxyc("bootstrap", "verify", SLUG, "--json");
    expect(r.status, r.stderr).toBe(0);
    const doc = JSON.parse(r.stdout) as {
      org_slug: string;
      phases: Array<{ id: string; landed: string; automation: string }>;
    };
    expect(doc.org_slug).toBe(SLUG);
    expect(doc.phases.map((p) => p.id)).toContain("git-link");
    expect(doc.phases.find((p) => p.id === "oltp")?.automation).toBe("manual");
    expect(doc.phases.find((p) => p.id === "repo")?.landed).toBe("skipped");
  });

  it("scopes to one phase and reads only what that phase needs", async () => {
    const r = await oxyc("bootstrap", "verify", SLUG, "--phase", "airhouse");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("airhouse");
    expect(r.stdout).not.toContain("billing");
    expect(requests.map((q) => q.url)).not.toContain(
      `/api/admin/orgs/${ORG_ID}/billing/subscription`
    );
  });
});

describe("bootstrap run", () => {
  it("--dry-run is the plan, and writes nothing", async () => {
    const r = await oxyc("bootstrap", "run", SLUG, "--owner", "ops@poke.example", "--dry-run");
    expect(r.status, r.stderr).toBe(0);
    expect(requests).toEqual([]);
    expect(r.stdout).toContain("standing up poke-house");
  });

  it("needs an owner before it will create an org, and says so before any request", async () => {
    const r = await oxyc("bootstrap", "run", SLUG);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
    expect(r.stderr).toContain("--owner");
  });

  it("does not need an owner to re-run one phase", async () => {
    const r = await oxyc("bootstrap", "run", SLUG, "--phase", "airhouse", "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).not.toContain("--owner");
  });

  it("refuses while an assume session is live, before it writes anything", async () => {
    world.assuming = true;
    world.orgExists = false;
    const r = await oxyc("bootstrap", "run", SLUG, "--owner", "ops@poke.example", "--yes");
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(r.stderr).toContain("closes /admin");
    expect(requests.some((q) => q.method === "POST")).toBe(false);
  });

  /**
   * The refusal has to beat the first admin READ, not just the first write.
   * `block_admin_while_acting` 403s the whole admin surface during a session,
   * so reading first meant exit 4 and a raw 403 — the documented exit 8 was
   * unreachable for the case it exists for.
   */
  it("refuses during a session before it reads /admin at all", async () => {
    world.assuming = true;
    const r = await oxyc("bootstrap", "run", SLUG, "--owner", "ops@poke.example", "--yes");
    expect(r.status, r.stderr).toBe(ExitCode.REFUSED);
    expect(requests.some((q) => q.url.startsWith("/api/admin/"))).toBe(false);
  });

  /** `verify` has no refusal: it reports, and an unreadable phase is `unknown`. */
  it("verify reports unknown rather than dying when a session closes /admin", async () => {
    world.assuming = true;
    const r = await oxyc("bootstrap", "verify", SLUG, "--json");
    expect(r.status, r.stderr).not.toBe(ExitCode.AUTH);
    const doc = JSON.parse(r.stdout) as {
      phases: Array<{ id: string; landed: string; detail: string }>;
    };
    const phase = (id: string) => doc.phases.find((p) => p.id === id);
    expect(phase("org")?.landed).toBe("yes");
    for (const id of ["oltp", "git-link", "billing"]) {
      expect(phase(id)?.landed, id).toBe("unknown");
      expect(phase(id)?.detail, id).toContain("assume-role session");
    }
  });

  /**
   * An outage must not read as `org: no`, because the next thing `run --yes`
   * does with that answer is create a second org.
   */
  it("stops on a 5xx from the admin org list instead of offering to create one", async () => {
    world.adminStatus = 502;
    const r = await oxyc("bootstrap", "run", SLUG, "--owner", "ops@poke.example", "--yes");
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
    expect(requests.some((q) => q.method === "POST" && q.url === "/api/admin/orgs")).toBe(false);
  });

  /**
   * The subscription route is Global-Owner-only, so a 403 is the normal case
   * for the person standing an account up — and it must not read as "no
   * Stripe configuration, so this phase does not apply", which is the
   * opposite of the truth for the one phase that 402s the GitHub import.
   */
  it("says only the Global Owner can read a subscription, rather than blaming Stripe", async () => {
    world.billingStatus = 403;
    const r = await oxyc("bootstrap", "verify", SLUG);
    expect(r.stdout + r.stderr).toContain("only the Global Owner can read a subscription");
    expect(r.stdout + r.stderr).not.toContain("no Stripe configuration");
  });

  /** `grants_access` grants `past_due` inside its grace window, which this response omits. */
  it("does not call a past_due subscription missing", async () => {
    world.subscription = "past_due";
    const r = await oxyc("bootstrap", "verify", SLUG, "--json");
    const doc = JSON.parse(r.stdout) as { phases: Array<{ id: string; landed: string }> };
    expect(doc.phases.find((p) => p.id === "billing")?.landed).toBe("unknown");
  });

  it("creates the org when it is absent, and warns that an invited owner cannot sign in", async () => {
    world.orgExists = false;
    const r = await oxyc("bootstrap", "run", SLUG, "--owner", "ops@poke.example", "--yes");
    const posted = requests.filter((q) => q.method === "POST").map((q) => q.url);
    expect(posted).toContain("/api/admin/orgs");
    expect(JSON.parse(requests.find((q) => q.url === "/api/admin/orgs")?.body ?? "{}")).toEqual({
      name: SLUG,
      slug: SLUG,
      owner_email: "ops@poke.example"
    });
    expect(r.stdout).toContain(`created org ${SLUG}`);
    expect(r.stderr).toContain("expires in 7 days");
  });

  it("does not re-create an org that is already there", async () => {
    const r = await oxyc("bootstrap", "run", SLUG, "--owner", "ops@poke.example", "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(requests.map((q) => q.url)).not.toContain("/api/admin/orgs");
  });

  it("provisions the Airhouse tenant when it is missing, then re-reads it", async () => {
    world.airhouse = "none";
    const r = await oxyc("bootstrap", "run", SLUG, "--owner", "ops@poke.example", "--yes");
    expect(
      requests.filter((q) => q.method === "POST" && q.url.includes("/airhouse/provision"))
    ).toHaveLength(1);
    expect(r.status, `${r.stdout}\n${r.stderr}`).toBe(0);
  });

  it("never provisions the OLTP store itself, and prints the command instead", async () => {
    world.oltpProvisioned = false;
    world.stagingBranch = false;
    const r = await oxyc("bootstrap", "run", SLUG, "--owner", "ops@poke.example", "--yes");
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(requests.some((q) => q.url.includes("/oltp/provision"))).toBe(false);
    expect(r.stderr).toContain("oxyc oltp provision --org poke-house --writer app:poke-house");
    expect(r.stderr).toContain("--branch staging");
  });

  it("refuses a write with no terminal and no --yes, rather than assuming yes", async () => {
    world.orgExists = false;
    const r = await oxyc("bootstrap", "run", SLUG, "--owner", "ops@poke.example");
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(requests.some((q) => q.method === "POST")).toBe(false);
  });
});
