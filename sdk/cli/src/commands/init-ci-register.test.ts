/**
 * `oxyc init-ci`'s registration half, against the trust-policy routes.
 *
 * Three properties carry the weight. It NEVER CALLS THE CREATE ROUTES — they
 * are session-only and the login is a token — and asks an org admin for one
 * approval instead, which mints nothing. It is NEVER FATAL — every refusal
 * comes back as `manual` with a reason and the ids learned so far, because
 * the workflow file is worth writing whoever runs the command. And it is
 * IDEMPOTENT — a second run finds what the first made and asks nobody.
 */

import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { get as httpGet } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createContext } from "../context/resolve.js";
import { type Call, type Routes, stubFetch } from "../testing/stub-fetch.js";
import { CliError } from "../util/errors.js";
import {
  type ApprovalOptions,
  isValidServiceAccountName,
  lookUpAccountId,
  type RegistrationPlan,
  registerTrustPolicy
} from "./init-ci-register.js";

const TARGET = "https://oxy.test";
const APP_ID = "0b0e5a10-1111-4222-8333-944455556666";
const WORKFLOW = ".github/workflows/oxy-publish.yml";
const ACCOUNTS = "/api/orgs/org-1/service-accounts";
const POLICIES = `${ACCOUNTS}/sa-1/trust-policies`;

const PLAN: RegistrationPlan = {
  org: "acme",
  app: "sales",
  accountOrg: "acme",
  accountName: "deployer",
  accountNamed: false,
  repository: "acme-co/acme-apps",
  workflowPath: WORKFLOW,
  environment: "oxy-publish"
};

const POLICY = {
  id: "tp-1",
  repository: "acme-co/acme-apps",
  workflow_path: WORKFLOW,
  environment: "oxy-publish",
  disabled_at: null
};

/** An org admin on a deployment with trusted access, and nothing registered yet. */
const fresh = (over: Routes = {}): Routes => ({
  "GET /api/orgs": () => ({ status: 200, body: [{ id: "org-1", slug: "acme" }] }),
  "GET /api/orgs/org-1/apps": () => ({ status: 200, body: [{ id: APP_ID, slug: "sales" }] }),
  [`GET ${ACCOUNTS}`]: () => ({ status: 200, body: { service_accounts: [] } }),
  [`GET ${POLICIES}`]: () => ({ status: 200, body: { trust_policies: [] } }),
  ...over
});

let scratch: string;

const context = () => createContext({ env: "production", target: TARGET }, scratch);
const posts = (calls: Call[]) => calls.filter((c) => c.method === "POST");

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-register-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXY_TOKEN", "session-jwt");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

describe("isValidServiceAccountName", () => {
  it("takes lowercase words joined by single hyphens, starting with a letter", () => {
    for (const name of ["deployer", "release-bot", "ci2", "a1-b2-c3"]) {
      expect(isValidServiceAccountName(name), name).toBe(true);
    }
  });

  it("refuses what the server refuses", () => {
    const tooLong = `a${"b".repeat(40)}`;
    for (const name of [
      "",
      "a",
      "Deployer",
      "1bot",
      "-bot",
      "bot-",
      "re--bot",
      "re_bot",
      tooLong
    ]) {
      expect(isValidServiceAccountName(name), name).toBe(false);
    }
    expect(isValidServiceAccountName(`a${"b".repeat(39)}`)).toBe(true);
  });
});

/**
 * The workflow names its account BY ID, so `init-ci` has to learn the id even
 * when it registers nothing. Read-only, and never fatal: `undefined` means a
 * placeholder gets written, never the name.
 */
describe("lookUpAccountId", () => {
  const existing = (over: Routes = {}): Routes =>
    fresh({
      [`GET ${ACCOUNTS}`]: () => ({
        status: 200,
        body: { service_accounts: [{ id: "sa-1", name: "deployer" }] }
      }),
      ...over
    });

  it("resolves the name to its id as whoever is logged in, and creates nothing", async () => {
    const calls = stubFetch(TARGET, existing());
    expect(await lookUpAccountId(context(), PLAN)).toBe("sa-1");
    expect(posts(calls)).toHaveLength(0);
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual(["GET /api/orgs", `GET ${ACCOUNTS}`]);
  });

  it("uses what registration already learned, without asking again", async () => {
    const calls = stubFetch(TARGET, existing());
    expect(await lookUpAccountId(context(), PLAN, { accountId: "sa-9" })).toBe("sa-9");
    expect(calls).toHaveLength(0);
    // The org's id, when that is all it learned, saves the lookup of it.
    expect(await lookUpAccountId(context(), PLAN, { orgId: "org-1" })).toBe("sa-1");
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual([`GET ${ACCOUNTS}`]);
  });

  it("is undefined — never a guess — when it cannot find out", async () => {
    // No such account yet.
    stubFetch(TARGET, fresh());
    expect(await lookUpAccountId(context(), PLAN)).toBeUndefined();
    // Not allowed to list, or a deployment without the route.
    for (const status of [401, 403, 404, 500]) {
      stubFetch(TARGET, existing({ [`GET ${ACCOUNTS}`]: () => ({ status, body: {} }) }));
      expect(await lookUpAccountId(context(), PLAN), String(status)).toBeUndefined();
    }
    // Not logged in at all: no request is made.
    vi.stubEnv("OXY_TOKEN", "");
    const calls = stubFetch(TARGET, existing());
    expect(await lookUpAccountId(context(), PLAN)).toBeUndefined();
    expect(calls).toHaveLength(0);
  });
});

describe("registerTrustPolicy", () => {
  /** What the deployment answers an approved registration with: no credential. */
  const REGISTRATION = {
    service_account: { id: "sa-1", name: "deployer" },
    trust_policy: { ...POLICY, grants: [{ kind: "app_publish", app_id: APP_ID }] },
    app: { id: APP_ID, slug: "sales", name: "Sales" },
    created_account: true,
    created_policy: true
  };
  /** The device-code routes, approved at the first poll. */
  const byDeviceCode = (answer: unknown = { status: "approved", registration: REGISTRATION }) => ({
    "POST /api/auth/cli/device": () => ({
      status: 200,
      body: { device_code: "dc", user_code: "BCDF-GHJK", expires_in: 600, interval: 5 }
    }),
    "POST /api/auth/cli/device/poll": () => ({ status: 200, body: answer })
  });
  const deviceApproval = (approver?: string): ApprovalOptions => ({
    route: () => ({ noBrowser: true, approver }),
    login: { hostname: "laptop", sleep: async () => {} }
  });
  const askOf = (calls: Call[]) =>
    JSON.parse(calls.find((c) => c.path === "/api/auth/cli/device")?.body ?? "{}");
  const creates = (calls: Call[]) =>
    posts(calls).filter((c) => c.path === ACCOUNTS || c.path === POLICIES);

  it("asks an org admin once, in the browser, and registers what the approval wrote", async () => {
    const calls = stubFetch(
      TARGET,
      fresh({
        "POST /api/auth/cli/exchange": () => ({ status: 200, body: { registration: REGISTRATION } })
      })
    );
    let page: URL | undefined;
    const browser = (url: string) => {
      page = new URL(url);
      const back = `http://127.0.0.1:${page.searchParams.get("port")}/callback?code=one-time&state=${encodeURIComponent(page.searchParams.get("state") ?? "")}`;
      // The loopback is a real socket; `fetch` is the stub, so knock with `http`.
      httpGet(back, (res) => res.resume());
    };
    const result = await registerTrustPolicy(context(), PLAN, {
      route: () => ({ noBrowser: false }),
      login: { open: browser, hostname: "laptop" }
    });
    expect(result).toEqual({
      kind: "registered",
      appId: APP_ID,
      createdAccount: true,
      createdPolicy: true,
      orgId: "org-1",
      accountId: "sa-1"
    });

    // The page is asked for exactly this account, app and run.
    expect(page?.pathname).toBe("/cli-auth");
    const asked = Object.fromEntries(page?.searchParams ?? []);
    expect(asked).toMatchObject({
      kind: "trust_policy",
      org: "org-1",
      account: "deployer",
      create: "1",
      app: APP_ID,
      repository: "acme-co/acme-apps",
      workflow: WORKFLOW,
      environment: "oxy-publish"
    });
    expect(asked.code_challenge).toBeTruthy();
    // Never the create routes: they are session-only, and this login is a token.
    expect(creates(calls)).toHaveLength(0);
    const exchange = calls.find((c) => c.path === "/api/auth/cli/exchange");
    expect(JSON.parse(exchange?.body ?? "{}")).toMatchObject({ code: "one-time" });
    // The stored bearer only ever reads; the exchange carries no credential.
    expect(exchange?.headers.authorization).toBeUndefined();
  });

  it("asks by device code, naming who should approve", async () => {
    const calls = stubFetch(TARGET, fresh(byDeviceCode()));
    const result = await registerTrustPolicy(context(), PLAN, deviceApproval("ada@acme.test"));
    expect(result).toMatchObject({ kind: "registered", accountId: "sa-1", createdPolicy: true });
    const start = askOf(calls);
    expect(start.approver).toBe("ada@acme.test");
    expect(start.ask).toMatchObject({
      kind: "trust_policy",
      org: "org-1",
      account: "deployer",
      create: "1",
      app: APP_ID
    });
    expect(creates(calls)).toHaveLength(0);
  });

  it("is idempotent: with the account and the policy in place it asks nobody", async () => {
    const route = vi.fn(() => ({ noBrowser: true }));
    const calls = stubFetch(
      TARGET,
      fresh({
        [`GET ${ACCOUNTS}`]: () => ({
          status: 200,
          body: { service_accounts: [{ id: "sa-1", name: "deployer" }] }
        }),
        // Same repository in another case, same environment in another case.
        [`GET ${POLICIES}`]: () => ({
          status: 200,
          body: {
            trust_policies: [
              {
                ...POLICY,
                repository: "Acme-Co/Acme-Apps",
                environment: "Oxy-Publish",
                grants: [{ kind: "app_publish", app_id: APP_ID }]
              }
            ]
          }
        })
      })
    );
    const result = await registerTrustPolicy(context(), PLAN, { route });
    expect(result).toMatchObject({
      kind: "registered",
      createdAccount: false,
      createdPolicy: false,
      accountId: "sa-1"
    });
    expect(route).not.toHaveBeenCalled();
    expect(posts(calls)).toHaveLength(0);
  });

  it("asks for a policy beside one that differs, was disabled, or publishes another app", async () => {
    const calls = stubFetch(
      TARGET,
      fresh({
        [`GET ${ACCOUNTS}`]: () => ({
          status: 200,
          body: { service_accounts: [{ id: "sa-1", name: "deployer" }] }
        }),
        [`GET ${POLICIES}`]: () => ({
          status: 200,
          body: {
            trust_policies: [
              {
                ...POLICY,
                id: "tp-0",
                environment: "staging",
                grants: REGISTRATION.trust_policy.grants
              },
              { ...POLICY, id: "tp-2", disabled_at: "2026-09-01T00:00:00Z" },
              { ...POLICY, id: "tp-3", grants: [{ kind: "app_publish", app_id: "other-app" }] }
            ]
          }
        }),
        ...byDeviceCode({
          status: "approved",
          registration: { ...REGISTRATION, created_account: false }
        })
      })
    );
    const result = await registerTrustPolicy(context(), PLAN, deviceApproval());
    expect(result).toMatchObject({
      kind: "registered",
      createdAccount: false,
      createdPolicy: true
    });
    expect(askOf(calls).ask.create).toBe("1");
  });

  it("asks without creating for a named account this login may not list", async () => {
    const calls = stubFetch(
      TARGET,
      fresh({
        [`GET ${ACCOUNTS}`]: () => ({ status: 403, body: { error: "forbidden" } }),
        ...byDeviceCode({
          status: "approved",
          registration: {
            ...REGISTRATION,
            service_account: { id: "sa-7", name: "release-bot" },
            created_account: false
          }
        })
      })
    );
    const result = await registerTrustPolicy(
      context(),
      { ...PLAN, accountName: "release-bot", accountNamed: true },
      deviceApproval()
    );
    expect(result).toMatchObject({ kind: "registered", accountId: "sa-7" });
    // A named account is never created: the approval says so.
    expect(askOf(calls).ask.create).toBeUndefined();
  });

  it("asks by the app's slug when this login may not list the org's apps or accounts", async () => {
    // A plain member: the approver, an org admin, decides what the slug names.
    const calls = stubFetch(
      TARGET,
      fresh({
        "GET /api/orgs/org-1/apps": () => ({ status: 403, body: { error: "forbidden" } }),
        [`GET ${ACCOUNTS}`]: () => ({ status: 403, body: { error: "forbidden" } }),
        ...byDeviceCode()
      })
    );
    const result = await registerTrustPolicy(context(), PLAN, deviceApproval());
    expect(askOf(calls).ask).toMatchObject({ app: "sales", create: "1" });
    // The app's id is learned from the answer, which names the app it granted.
    expect(result).toMatchObject({ kind: "registered", appId: APP_ID, accountId: "sa-1" });
  });

  it("never sends the repository's ids: the deployment reads them from GitHub", async () => {
    const calls = stubFetch(TARGET, fresh(byDeviceCode()));
    await registerTrustPolicy(context(), PLAN, deviceApproval());
    const { ask } = askOf(calls);
    expect(ask).toMatchObject({ repository: "acme-co/acme-apps" });
    expect(ask).not.toHaveProperty("repository_id");
    expect(ask).not.toHaveProperty("repository_owner_id");
  });

  describe("--no-wait: ask, say where, and return; a re-run collects", () => {
    const noWait = (over: Partial<ApprovalOptions> = {}): ApprovalOptions => ({
      ...deviceApproval("ada@acme.test"),
      noWait: true,
      ...over
    });
    const pendingFile = () => join(scratch, "init-ci-pending.json");
    const polls = (calls: Call[]) => calls.filter((c) => c.path === "/api/auth/cli/device/poll");

    it("asks once and returns `pending` with the link, recording the request", async () => {
      const calls = stubFetch(TARGET, fresh(byDeviceCode()));
      const result = await registerTrustPolicy(context(), PLAN, noWait());
      expect(result).toMatchObject({
        kind: "pending",
        url: `${TARGET}/cli-auth/device?code=BCDF-GHJK`,
        userCode: "BCDF-GHJK",
        orgId: "org-1",
        appId: APP_ID
      });
      expect(askOf(calls).approver).toBe("ada@acme.test");
      // Nothing waited on: the request is recorded, owner-only, for the re-run.
      expect(polls(calls)).toHaveLength(0);
      const stored = JSON.parse(readFileSync(pendingFile(), "utf8"));
      expect(Object.values(stored)).toHaveLength(1);
      expect(statSync(pendingFile()).mode & 0o077).toBe(0);
    });

    it("a re-run checks once: still waiting, then collected", async () => {
      let answer: unknown = { status: "pending", interval: 5 };
      const routes = fresh({
        ...byDeviceCode(),
        "POST /api/auth/cli/device/poll": () => ({ status: 200, body: answer })
      });
      stubFetch(TARGET, routes);
      await registerTrustPolicy(context(), PLAN, noWait());

      const calls = stubFetch(TARGET, routes);
      const again = await registerTrustPolicy(context(), PLAN, noWait());
      expect(again).toMatchObject({ kind: "pending", userCode: "BCDF-GHJK" });
      // One poll, and no second request was started.
      expect(polls(calls)).toHaveLength(1);
      expect(calls.some((c) => c.path === "/api/auth/cli/device")).toBe(false);

      answer = { status: "approved", registration: REGISTRATION };
      const done = await registerTrustPolicy(context(), PLAN, noWait());
      expect(done).toMatchObject({ kind: "registered", accountId: "sa-1", createdPolicy: true });
      expect(Object.values(JSON.parse(readFileSync(pendingFile(), "utf8")))).toHaveLength(0);
    });

    it("a plain re-run waits for the request an earlier --no-wait started", async () => {
      const routes = fresh(byDeviceCode());
      stubFetch(TARGET, routes);
      await registerTrustPolicy(context(), PLAN, noWait());
      const calls = stubFetch(TARGET, routes);
      const result = await registerTrustPolicy(context(), PLAN, deviceApproval());
      expect(result).toMatchObject({ kind: "registered", accountId: "sa-1" });
      expect(calls.some((c) => c.path === "/api/auth/cli/device")).toBe(false);
    });

    it("a refused request is forgotten, and says why", async () => {
      const routes = fresh(byDeviceCode());
      stubFetch(TARGET, routes);
      await registerTrustPolicy(context(), PLAN, noWait());
      stubFetch(
        TARGET,
        fresh({
          ...byDeviceCode(),
          "POST /api/auth/cli/device/poll": () => ({
            status: 400,
            body: { code: "access_denied", error: "the request was declined in the browser" }
          })
        })
      );
      const result = await registerTrustPolicy(context(), PLAN, noWait());
      expect(result.kind).toBe("manual");
      expect(result.kind === "manual" && result.reason).toContain("refused");
      expect(Object.values(JSON.parse(readFileSync(pendingFile(), "utf8")))).toHaveLength(0);
    });

    it("another app's or repository's request is never collected for this one", async () => {
      stubFetch(TARGET, fresh(byDeviceCode()));
      await registerTrustPolicy(context(), PLAN, noWait());
      const calls = stubFetch(TARGET, fresh(byDeviceCode()));
      const other = await registerTrustPolicy(
        context(),
        { ...PLAN, environment: "staging" },
        noWait()
      );
      expect(other.kind).toBe("pending");
      // A request of its own, not a poll of the other one.
      expect(calls.some((c) => c.path === "/api/auth/cli/device")).toBe(true);
      expect(polls(calls)).toHaveLength(0);
    });
  });

  describe("stops short as `manual`, with a reason and the ids it learned", () => {
    const manual = async (
      routes: Routes,
      plan = PLAN,
      approval: ApprovalOptions | undefined = deviceApproval()
    ) => {
      const calls = stubFetch(TARGET, routes);
      const result = await registerTrustPolicy(context(), plan, approval);
      if (result.kind !== "manual") throw new Error("expected a manual registration");
      return { result, calls };
    };

    it("a named account that does not exist — never created, nobody asked", async () => {
      const { result, calls } = await manual(fresh(byDeviceCode()), {
        ...PLAN,
        accountName: "release-bot",
        accountNamed: true
      });
      expect(result.reason).toContain("no service account acme/release-bot");
      expect(posts(calls)).toHaveLength(0);
    });

    it("nobody here can approve (a CI runner)", async () => {
      const { result, calls } = await manual(fresh(byDeviceCode()), PLAN, {
        route: () => {
          throw new CliError("nobody on this GitHub Actions runner can approve the trust policy");
        }
      });
      expect(result.reason).toContain("nobody on this GitHub Actions runner can approve");
      expect(result).toMatchObject({ orgId: "org-1", appId: APP_ID });
      expect(posts(calls)).toHaveLength(0);
    });

    it("the approval was refused, with the approver's reason", async () => {
      const { result } = await manual(
        fresh({
          ...byDeviceCode(),
          "POST /api/auth/cli/device/poll": () => ({
            status: 400,
            body: {
              code: "access_denied",
              error: "only an owner or admin of the organization may create its service accounts"
            }
          })
        })
      );
      expect(result.reason).toContain("the request was refused");
    });

    it("an answer that is not the policy asked for", async () => {
      const { result } = await manual(
        fresh(
          byDeviceCode({
            status: "approved",
            registration: {
              ...REGISTRATION,
              trust_policy: {
                ...REGISTRATION.trust_policy,
                workflow_path: ".github/workflows/x.yml"
              }
            }
          })
        )
      );
      expect(result.reason).toContain("not the one asked for");
      expect(result.accountId).toBeUndefined();
    });

    it("a deployment that cannot approve one (no device-code route)", async () => {
      const { result } = await manual(fresh());
      expect(result.reason).toContain("cannot approve anything by device code yet");
    });

    it("no way to ask was given", async () => {
      stubFetch(TARGET, fresh(byDeviceCode()));
      const result = await registerTrustPolicy(context(), PLAN);
      expect(result).toMatchObject({ kind: "manual", orgId: "org-1", appId: APP_ID });
      expect(result.kind === "manual" && result.reason).toContain("needs an org admin's approval");
    });

    it("an answer for another app than the one asked for", async () => {
      const { result } = await manual(
        fresh(
          byDeviceCode({
            status: "approved",
            registration: { ...REGISTRATION, app: { id: APP_ID, slug: "other" } }
          })
        )
      );
      expect(result.reason).toContain('publishing "other", not sales');
    });

    it("an app that was never published", async () => {
      const { result } = await manual(
        fresh({ "GET /api/orgs/org-1/apps": () => ({ status: 200, body: [] }) })
      );
      expect(result.reason).toContain("acme/sales is not registered on this deployment yet");
    });

    it("an account of another org — its grants cannot reach this app, so nothing is asked", async () => {
      const { result, calls } = await manual(fresh(), {
        ...PLAN,
        accountOrg: "globex",
        accountNamed: true
      });
      expect(result.reason).toContain("globex/deployer cannot be granted publishing acme/sales");
      expect(calls).toHaveLength(0);
    });

    it("nobody logged in — and no request is made", async () => {
      vi.stubEnv("OXY_TOKEN", "");
      const { result, calls } = await manual(fresh());
      expect(result.reason).toBe("not logged in to https://oxy.test");
      expect(calls).toHaveLength(0);
    });

    it("a checkout with no GitHub remote", async () => {
      const { result, calls } = await manual(fresh(), { ...PLAN, repository: undefined });
      expect(result.reason).toContain("no `origin` remote on GitHub");
      expect(calls).toHaveLength(0);
    });

    it("a dead network", async () => {
      vi.stubGlobal(
        "fetch",
        vi.fn(async () => {
          throw new TypeError("fetch failed");
        })
      );
      const result = await registerTrustPolicy(context(), PLAN, deviceApproval());
      expect(result.kind).toBe("manual");
    });
  });
});
