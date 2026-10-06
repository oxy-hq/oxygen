/**
 * `oxyc init-ci`'s registration half, against the trust-policy routes.
 *
 * Two properties carry the weight. It is NEVER FATAL — every refusal comes
 * back as `manual` with a reason and the ids learned so far, because the
 * workflow file is worth writing whoever runs the command. And it is
 * IDEMPOTENT — a second run finds what the first made.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createContext } from "../context/resolve.js";
import { type Call, type Routes, stubFetch } from "../testing/stub-fetch.js";
import {
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

const SESSION_REQUIRED = {
  status: 403,
  body: { error: "this route requires a browser session", code: "session_required" }
};

/** An org admin on a deployment with trusted access, and nothing registered yet. */
const fresh = (over: Routes = {}): Routes => ({
  "GET /api/orgs": () => ({ status: 200, body: [{ id: "org-1", slug: "acme" }] }),
  "GET /api/orgs/org-1/apps": () => ({ status: 200, body: [{ id: APP_ID, slug: "sales" }] }),
  [`GET ${ACCOUNTS}`]: () => ({ status: 200, body: { service_accounts: [] } }),
  [`POST ${ACCOUNTS}`]: () => ({ status: 201, body: { id: "sa-1", name: "deployer" } }),
  [`GET ${POLICIES}`]: () => ({ status: 200, body: { trust_policies: [] } }),
  [`POST ${POLICIES}`]: () => ({ status: 201, body: POLICY }),
  ...over
});

let scratch: string;

const context = () => createContext({ env: "production", target: TARGET }, scratch);
const posts = (calls: Call[]) => calls.filter((c) => c.method === "POST");
const noGh = () => undefined;

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
  it("creates `deployer`, then a policy granting exactly this app's publish", async () => {
    const calls = stubFetch(TARGET, fresh());
    const result = await registerTrustPolicy(context(), PLAN, noGh);
    expect(result).toEqual({
      kind: "registered",
      appId: APP_ID,
      createdAccount: true,
      createdPolicy: true,
      orgId: "org-1",
      accountId: "sa-1"
    });

    const [account, policy] = posts(calls);
    expect(account?.path).toBe(ACCOUNTS);
    expect(JSON.parse(account?.body ?? "{}")).toMatchObject({
      name: "deployer",
      org_role: "member"
    });
    expect(policy?.path).toBe(POLICIES);
    expect(JSON.parse(policy?.body ?? "{}")).toEqual({
      repository: "acme-co/acme-apps",
      workflow_path: WORKFLOW,
      environment: "oxy-publish",
      grants: [{ kind: "app_publish", app_id: APP_ID }]
    });
    // The stored bearer, on every call: init-ci never mints a credential.
    expect(calls.every((c) => c.headers.authorization === "Bearer session-jwt")).toBe(true);
  });

  it("is idempotent: a second run finds the account and the policy, and creates nothing", async () => {
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
              { ...POLICY, repository: "Acme-Co/Acme-Apps", environment: "Oxy-Publish" }
            ]
          }
        })
      })
    );
    const result = await registerTrustPolicy(context(), PLAN, noGh);
    expect(result).toMatchObject({
      kind: "registered",
      createdAccount: false,
      createdPolicy: false
    });
    expect(posts(calls)).toHaveLength(0);
  });

  it("adds a policy beside one that differs, or that was disabled", async () => {
    const calls = stubFetch(
      TARGET,
      fresh({
        [`GET ${POLICIES}`]: () => ({
          status: 200,
          body: {
            trust_policies: [
              { ...POLICY, id: "tp-0", environment: "staging" },
              { ...POLICY, id: "tp-2", disabled_at: "2026-09-01T00:00:00Z" }
            ]
          }
        })
      })
    );
    const result = await registerTrustPolicy(context(), PLAN, noGh);
    expect(result).toMatchObject({ kind: "registered", createdPolicy: true });
    expect(posts(calls).map((c) => c.path)).toContain(POLICIES);
  });

  it("uses a named account, and never creates one that was named and is missing", async () => {
    const calls = stubFetch(TARGET, fresh());
    const result = await registerTrustPolicy(
      context(),
      { ...PLAN, accountName: "release-bot", accountNamed: true },
      noGh
    );
    expect(result.kind).toBe("manual");
    expect(result.kind === "manual" && result.reason).toContain(
      "no service account acme/release-bot"
    );
    expect(posts(calls)).toHaveLength(0);
  });

  it("recovers when someone creates the account between the list and the create", async () => {
    let listed = 0;
    stubFetch(
      TARGET,
      fresh({
        [`GET ${ACCOUNTS}`]: () => ({
          status: 200,
          body: { service_accounts: listed++ === 0 ? [] : [{ id: "sa-1", name: "deployer" }] }
        }),
        [`POST ${ACCOUNTS}`]: () => ({ status: 409, body: { code: "name_taken" } })
      })
    );
    const result = await registerTrustPolicy(context(), PLAN, noGh);
    expect(result).toMatchObject({ kind: "registered", createdAccount: false, accountId: "sa-1" });
  });

  it("supplies the repository's ids from `gh` when the deployment cannot resolve it", async () => {
    let attempts = 0;
    const calls = stubFetch(
      TARGET,
      fresh({
        [`POST ${POLICIES}`]: () =>
          attempts++ === 0
            ? { status: 422, body: { code: "repository_unresolved" } }
            : { status: 201, body: POLICY }
      })
    );
    const lookedUp: string[] = [];
    const result = await registerTrustPolicy(context(), PLAN, (repository) => {
      lookedUp.push(repository);
      return { repository_id: 42, repository_owner_id: 7 };
    });
    expect(result).toMatchObject({ kind: "registered", createdPolicy: true });
    expect(lookedUp).toEqual(["acme-co/acme-apps"]);
    const retried = posts(calls).filter((c) => c.path === POLICIES)[1];
    expect(JSON.parse(retried?.body ?? "{}")).toMatchObject({
      repository: "acme-co/acme-apps",
      repository_id: 42,
      repository_owner_id: 7
    });
  });

  describe("stops short as `manual`, with a reason and the ids it learned", () => {
    const manual = async (routes: Routes, plan = PLAN, lookUp = noGh) => {
      const calls = stubFetch(TARGET, routes);
      const result = await registerTrustPolicy(context(), plan, lookUp);
      if (result.kind !== "manual") throw new Error("expected a manual registration");
      return { result, calls };
    };

    it("a token login: creating the account needs a browser session", async () => {
      const { result, calls } = await manual(
        fresh({ [`POST ${ACCOUNTS}`]: () => SESSION_REQUIRED })
      );
      expect(result.reason).toContain("needs a browser session");
      expect(result).toMatchObject({ orgId: "org-1", appId: APP_ID });
      expect(result.accountId).toBeUndefined();
      // It stopped at the refusal rather than trying the policy without an account.
      expect(posts(calls)).toHaveLength(1);
    });

    it("a token login with the account already there: the policy needs one too", async () => {
      const { result } = await manual(
        fresh({
          [`GET ${ACCOUNTS}`]: () => ({
            status: 200,
            body: { service_accounts: [{ id: "sa-1", name: "deployer" }] }
          }),
          [`POST ${POLICIES}`]: () => SESSION_REQUIRED
        })
      );
      expect(result.reason).toContain("creating the trust policy needs a browser session");
      expect(result).toMatchObject({ orgId: "org-1", appId: APP_ID, accountId: "sa-1" });
    });

    it("not an org admin", async () => {
      const { result } = await manual(
        fresh({ "GET /api/orgs/org-1/apps": () => ({ status: 403, body: { error: "forbidden" } }) })
      );
      expect(result.reason).toContain("takes an admin of acme");
      expect(result.orgId).toBe("org-1");
    });

    it("a deployment that predates trusted access (404 on the service-account routes)", async () => {
      const { [`GET ${ACCOUNTS}`]: _gone, ...older } = fresh();
      const { result, calls } = await manual(older);
      expect(result.reason).toContain("predates trusted access");
      expect(result).toMatchObject({ orgId: "org-1", appId: APP_ID });
      expect(posts(calls)).toHaveLength(0);
    });

    it("an app that was never published", async () => {
      const { result } = await manual(
        fresh({ "GET /api/orgs/org-1/apps": () => ({ status: 200, body: [] }) })
      );
      expect(result.reason).toContain("acme/sales is not registered on this deployment yet");
    });

    it("an unresolvable repository with no `gh` to ask", async () => {
      const { result } = await manual(
        fresh({
          [`POST ${POLICIES}`]: () => ({ status: 422, body: { code: "repository_unresolved" } })
        })
      );
      expect(result.reason).toContain("could not resolve acme-co/acme-apps");
      expect(result.accountId).toBe("sa-1");
    });

    it("the server's own reason, when it gives one", async () => {
      const { result } = await manual(
        fresh({
          [`POST ${POLICIES}`]: () => ({
            status: 400,
            body: { error: "an environment is required", code: "environment_required" }
          })
        })
      );
      expect(result.reason).toBe(
        "creating the trust policy failed (400): an environment is required"
      );
    });

    it("an account of another org — its grants cannot reach this app, so nothing is asked", async () => {
      const { result, calls } = await manual(fresh(), {
        ...PLAN,
        accountOrg: "globex",
        accountNamed: true
      });
      expect(result.reason).toContain("globex/deployer cannot be granted publishing acme/sales");
      expect(result.reason).toContain("its own organization");
      expect(calls).toHaveLength(0);
    });

    it("a 404 creating the policy is the app it grants, not a missing route", async () => {
      const { result } = await manual(
        fresh({ [`POST ${POLICIES}`]: () => ({ status: 404, body: { error: "not found" } }) })
      );
      expect(result.reason).toContain("found no app acme/sales in acme");
      expect(result.reason).not.toContain("predates trusted access");
      expect(result).toMatchObject({ orgId: "org-1", appId: APP_ID, accountId: "sa-1" });
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
      const result = await registerTrustPolicy(context(), PLAN, noGh);
      expect(result.kind).toBe("manual");
    });
  });
});
