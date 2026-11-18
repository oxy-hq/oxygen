/**
 * `oxyc tokens create --org-service-account`, against a fake deployment and a
 * fake browser — the harness of `tokens-agent.test.ts`.
 *
 * What is pinned: every usage error lands before a browser opens; the
 * `/cli-auth` URL; without `--set-app-secret` stdout is the secret and nothing
 * else; with it the secret goes into the app's secret on the operator's own
 * login and is NEVER PRINTED, on success or failure; the credentials file is
 * never written; and a token that is not the one asked for, or one that could
 * not be stored, is revoked with itself.
 */

import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { saveCredential } from "../auth/credentials.js";
import { createContext } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  parseServiceAccountMint,
  runTokensCreateServiceAccount,
  type ServiceAccountMintFlags,
  sameOrg,
  serviceAccountMintQuery,
  serviceAccountMismatch
} from "./tokens-service-account.js";

const SECRET = "oxy_sat_0123456789abcdefghijABCDEFGHIJ012345";
/** The operator's own login: what reads the app listing and writes the secret. */
const OPERATOR = "oxy_pat_operatorLOGIN0123456789abcdefghij";
const ORG_ID = "6f1c2f7e-0000-4000-8000-000000000001";
const APP_ID = "6f1c2f7e-0000-4000-8000-0000000000aa";

const inDays = (days: number) => new Date(Date.now() + days * 86_400_000).toISOString();

let TOKEN_ROW: Record<string, unknown>;
let server: Server;
let target: string;
let exchangeReply: { status: number; body: unknown };
let describedOver: Record<string, unknown>;
/** What the secrets route answers. */
let secretReply: { status: number; body?: unknown };
let requests: Array<{ method: string; url: string; authorization?: string; body: string }>;
let opened: URL[];
let scratch: string;
let credentialsFile: string;
let stdout: string;
let stderr: string;

async function read(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString();
}

const APP_ROW = {
  id: APP_ID,
  slug: "store-ops",
  org_slug: "poke-house",
  org_id: ORG_ID,
  name: "Store Ops",
  project_id: "p",
  branch: "main",
  source_repo: "r",
  status: "published",
  url: "u",
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z"
};

beforeAll(async () => {
  server = createServer((req, res) => {
    void (async () => {
      const body = await read(req);
      requests.push({
        method: req.method ?? "GET",
        url: req.url ?? "",
        authorization: req.headers.authorization,
        body
      });
      const reply = (status: number, value?: unknown) => {
        res.writeHead(status, { "content-type": "application/json" });
        res.end(value === undefined ? undefined : JSON.stringify(value));
      };
      const url = req.url ?? "";
      if (url === "/api/auth/cli/exchange" && req.method === "POST") {
        return reply(exchangeReply.status, exchangeReply.body);
      }
      if (url === "/api/auth/token" && req.method === "DELETE") return reply(204);
      if (url === "/api/auth/token" && req.method === "GET") {
        return reply(200, { ...TOKEN_ROW, ...describedOver });
      }
      if (url.startsWith("/api/customer-apps?") && req.method === "GET") {
        return reply(200, { items: [APP_ROW], next_offset: null });
      }
      if (url === `/api/customer-apps/${APP_ID}/secrets` && req.method === "POST") {
        return reply(secretReply.status, secretReply.body);
      }
      reply(404, { error: `unexpected ${url}` });
    })();
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  target = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(() => {
  server.close();
});

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-sa-mint-"));
  credentialsFile = join(scratch, "credentials.json");
  vi.stubEnv("OXY_CREDENTIALS_PATH", credentialsFile);
  vi.stubEnv("OXY_TOKEN", "");
  vi.stubEnv("OXYC_QUIET", "");
  saveCredential(target, { token: OPERATOR, email: "ops@oxy.test", is_app_admin: true });
  TOKEN_ROW = {
    id: "tok-sa-1",
    name: "tablets",
    kind: "service_account",
    source: "oxyc",
    display_prefix: "oxy_sat_0123",
    last_four: "2345",
    all_access: false,
    platform: false,
    partner: false,
    grants: [
      {
        id: "g-1",
        kind: "workspace",
        org_id: ORG_ID,
        org_name: "Poke House",
        workspace_id: null,
        workspace_name: null,
        role_ceiling: "admin",
        app_id: null,
        app_name: null,
        revoked_at: null
      }
    ],
    expires_at: inDays(90),
    status: "active",
    owner: { type: "service_account", id: "sa-1", label: "store-ops-tablets" },
    blocked_orgs: []
  };
  exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: SECRET } };
  describedOver = {};
  secretReply = { status: 201 };
  requests = [];
  opened = [];
  stdout = "";
  stderr = "";
  vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    stdout += String(chunk);
    return true;
  });
  vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
    stderr += String(chunk);
    return true;
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

/** The page: the approver approves, and it redirects to the loopback with a code. */
function browser(url: string): void {
  const page = new URL(url);
  opened.push(page);
  const port = page.searchParams.get("port");
  const state = page.searchParams.get("state") ?? "";
  void fetch(
    `http://127.0.0.1:${port}/callback?code=one-time-code&state=${encodeURIComponent(state)}`
  );
}

const context = () => createContext({ env: "dev", target }, scratch);

const BASE: ServiceAccountMintFlags = {
  serviceAccount: "poke-house/store-ops-tablets",
  role: "admin",
  tokenName: "tablets"
};

const mint = (flags: Partial<ServiceAccountMintFlags> = {}) =>
  runTokensCreateServiceAccount(
    context(),
    { ...BASE, ...flags },
    {
      open: browser,
      hostname: "ops-laptop"
    }
  );

const STORE = { setAppSecret: "poke-house/store-ops", key: "TABLETS_API_KEY" };

async function failure(run: Promise<unknown>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (thrown: unknown) => thrown
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

const revokes = () => requests.filter((r) => r.method === "DELETE" && r.url === "/api/auth/token");
const secretWrites = () => requests.filter((r) => r.url.endsWith("/secrets"));
/** The credentials file holds the operator's login and nothing the mint returned. */
const credentialsKeepNoSecret = () =>
  expect(readFileSync(credentialsFile, "utf8")).not.toContain(SECRET);
/** Every byte this run wrote to a terminal, and every error it raised. */
const printedEverywhere = (cause?: CliError) =>
  [stdout, stderr, cause?.message, cause?.detail, cause?.hint].join("\n");

describe("argument validation, before any browser opens", () => {
  const parse = (flags: Partial<ServiceAccountMintFlags>) =>
    parseServiceAccountMint({ serviceAccount: "acme/store-ops", ...flags }, "ops-laptop");

  it("defaults to a member account, 90 days and a name that says where it was minted", () => {
    expect(parse({})).toEqual({
      mint: {
        org: "acme",
        account: "store-ops",
        role: "member",
        name: "minted by oxyc on ops-laptop",
        days: 90
      }
    });
  });

  it("reads the role, the lifetime and where the secret goes", () => {
    expect(
      parse({
        role: "admin",
        expiresInDays: "30",
        setAppSecret: "acme/app",
        key: "K",
        appEnv: "staging"
      })
    ).toEqual({
      mint: {
        org: "acme",
        account: "store-ops",
        role: "admin",
        name: "minted by oxyc on ops-laptop",
        days: 30
      },
      secret: { app: "acme/app", key: "K", environment: "staging" }
    });
  });

  it("refuses what the server would refuse", () => {
    expect(() => parse({ serviceAccount: "store-ops" })).toThrow("is not <org>/<account>");
    expect(() => parse({ serviceAccount: "acme/Store_Ops" })).toThrow(
      "is not a service account name"
    );
    expect(() => parse({ serviceAccount: "acme/x" })).toThrow("is not a service account name");
    expect(() => parse({ serviceAccount: "ac&me/store-ops" })).toThrow("slug or id");
    expect(() => parse({ role: "owner" })).toThrow("cannot be an owner");
    expect(() => parse({ role: "viewer" })).toThrow("is not member or admin");
    for (const days of ["0", "3651", "1.5", "soon"]) {
      expect(() => parse({ expiresInDays: days }), days).toThrow("--expires-in-days");
    }
    expect(() => parse({ setAppSecret: "acme/app" })).toThrow("needs --key");
    expect(() => parse({ setAppSecret: "acme/app", key: "A KEY" })).toThrow("is not a secret key");
    expect(() => parse({ setAppSecret: "store-ops", key: "K" })).toThrow("is not <org>/<app>");
    expect(() => parse({ setAppSecret: "acme/app", key: "K", appEnv: "dev-luong" })).toThrow(
      "not production or staging"
    );
    expect(() => parse({ key: "K" })).toThrow("only valid with --set-app-secret");
  });

  it("is a usage error (exit 2), and nothing was opened or requested", async () => {
    for (const flags of [{ role: "owner" }, { expiresInDays: "0" }, { key: "K" }]) {
      expect((await failure(mint(flags))).code).toBe(ExitCode.USAGE);
    }
    expect(opened).toHaveLength(0);
    expect(requests).toHaveLength(0);
  });
});

describe("the /cli-auth URL", () => {
  it("names the kind, the org, the account, the role, the lifetime and the token name", () => {
    expect(
      serviceAccountMintQuery({
        org: "acme",
        account: "store-ops",
        role: "admin",
        name: "a&b",
        days: 30
      })
    ).toBe("kind=service_account&org=acme&account=store-ops&role=admin&days=30&name=a%26b");
  });

  it("is what the browser opens", async () => {
    await mint();
    const page = opened[0] as URL;
    expect(page.pathname).toBe("/cli-auth");
    expect(page.searchParams.get("kind")).toBe("service_account");
    expect(page.searchParams.get("org")).toBe("poke-house");
    expect(page.searchParams.get("account")).toBe("store-ops-tablets");
    expect(page.searchParams.get("role")).toBe("admin");
    expect(page.searchParams.get("name")).toBe("tablets");
    expect(page.searchParams.get("code_challenge")).toBeTruthy();
  });
});

describe("printed once", () => {
  it("puts the secret on stdout and nothing else there, and keeps none of it", async () => {
    await mint();
    expect(stdout).toBe(`${SECRET}\n`);
    expect(stderr).not.toContain(SECRET);
    expect(stderr).toContain("store-ops-tablets");
    credentialsKeepNoSecret();
    expect(revokes()).toHaveLength(0);
    expect(secretWrites()).toHaveLength(0);
  });
});

describe("--set-app-secret", () => {
  it("stores the secret on the operator's login and never prints it", async () => {
    await mint(STORE);
    const writes = secretWrites();
    expect(writes).toHaveLength(1);
    expect(writes[0]?.authorization).toBe(`Bearer ${OPERATOR}`);
    expect(JSON.parse(writes[0]?.body ?? "{}")).toEqual({
      key: "TABLETS_API_KEY",
      value: SECRET,
      environment: "production"
    });
    expect(stdout).toBe("");
    expect(printedEverywhere()).not.toContain(SECRET);
    expect(stderr).toContain("TABLETS_API_KEY on poke-house/store-ops (production) — created");
    credentialsKeepNoSecret();
    expect(revokes()).toHaveLength(0);
  });

  it("says a 204 replaced the old value, still without printing it", async () => {
    secretReply = { status: 204 };
    await mint({ ...STORE, appEnv: "staging" });
    expect(JSON.parse(secretWrites()[0]?.body ?? "{}").environment).toBe("staging");
    expect(stderr).toContain("rotated");
    expect(printedEverywhere()).not.toContain(SECRET);
  });

  it("revokes the token and keeps the store's own exit code when the store refuses, without the value", async () => {
    // A deployment that echoes the value back must still not get it printed.
    secretReply = { status: 403, body: { error: `no access to set ${SECRET}` } };
    const cause = await failure(mint(STORE));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.message).toContain("[redacted]");
    expect(cause.detail).toContain("revoked");
    expect(revokes().map((r) => r.authorization)).toEqual([`Bearer ${SECRET}`]);
    expect(stdout).toBe("");
    expect(printedEverywhere(cause)).not.toContain(SECRET);
    credentialsKeepNoSecret();
  });

  it("refuses an app of another organization before anyone is asked to approve", async () => {
    const cause = await failure(mint({ ...STORE, serviceAccount: "other-org/store-ops-tablets" }));
    expect(cause.code).toBe(ExitCode.USAGE);
    expect(cause.message).toContain("is not in the organization other-org");
    expect(opened).toHaveLength(0);
    expect(requests.some((r) => r.url === "/api/auth/cli/exchange")).toBe(false);
  });

  it("refuses an app it cannot find before anyone is asked to approve", async () => {
    const cause = await failure(mint({ ...STORE, setAppSecret: "poke-house/nope" }));
    expect(cause.code).toBe(ExitCode.NOT_FOUND);
    expect(opened).toHaveLength(0);
  });
});

describe("a token that is not the one asked for", () => {
  it("is revoked with itself, printed nowhere and stored nowhere", async () => {
    for (const over of [
      { owner: { type: "service_account", id: "x", label: "someone-else" } },
      { kind: "personal" },
      { grants: [] },
      { expires_at: null }
    ]) {
      requests = [];
      stdout = "";
      describedOver = over;
      const cause = await failure(mint(STORE));
      expect(cause.code, JSON.stringify(over)).toBe(ExitCode.REFUSED);
      expect(stdout).toBe("");
      expect(secretWrites()).toHaveLength(0);
      expect(revokes()).toHaveLength(1);
      expect(printedEverywhere(cause)).not.toContain(SECRET);
    }
  });

  it("is refused when the secret is not a whole oxy_sat_", async () => {
    exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: "oxy_sat_short;rm -rf" } };
    const cause = await failure(mint());
    expect(cause.code).toBe(ExitCode.REFUSED);
    expect(stdout).toBe("");
  });
});

describe("serviceAccountMismatch", () => {
  const ask = {
    org: "poke-house",
    account: "store-ops-tablets",
    role: "admin" as const,
    name: "tablets",
    days: 90
  };

  it("accepts the token asked for, and a shorter life than asked", () => {
    expect(serviceAccountMismatch(target, ask, TOKEN_ROW)).toBeUndefined();
    expect(
      serviceAccountMismatch(target, ask, { ...TOKEN_ROW, expires_at: inDays(10) })
    ).toBeUndefined();
  });

  it("refuses a wider grant, a second grant or a longer life", () => {
    const grant = (TOKEN_ROW.grants as Record<string, unknown>[])[0];
    const owner = { ...grant, role_ceiling: "owner" };
    expect(serviceAccountMismatch(target, ask, { ...TOKEN_ROW, grants: [owner] })?.why).toContain(
      "owner"
    );
    expect(
      serviceAccountMismatch(target, ask, { ...TOKEN_ROW, grants: [grant, grant] })?.why
    ).toContain("2 grants");
    const scoped = { ...grant, workspace_id: "w-1" };
    expect(serviceAccountMismatch(target, ask, { ...TOKEN_ROW, grants: [scoped] })).toBeDefined();
    expect(
      serviceAccountMismatch(target, ask, { ...TOKEN_ROW, expires_at: inDays(400) })?.why
    ).toContain("after the 90 days");
    expect(serviceAccountMismatch(target, ask, { ...TOKEN_ROW, all_access: true })).toBeDefined();
  });
});

describe("sameOrg", () => {
  it("compares a slug to the app's org slug and an id to its org id", () => {
    expect(sameOrg("poke-house", APP_ROW)).toBe(true);
    expect(sameOrg(ORG_ID.toUpperCase(), APP_ROW)).toBe(true);
    expect(sameOrg("poke-house-staging", APP_ROW)).toBe(false);
  });
});
