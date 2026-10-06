/**
 * `oxyc` under a sandbox agent token (`oxy_sbx_…`), against a stubbed
 * deployment: what the prefix decides, where an app is resolved, which mount
 * read-back uses, and everything refused before a request is made.
 *
 * The stub answers 404 for any route it was not given — which is also what a
 * real deployment answers this token outside the sandbox loop, so "the client
 * never called it" and "the call would have failed" are told apart by the
 * recorded calls, never by the status.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { saveCredential } from "../auth/credentials.js";
import {
  credentialShape,
  isMachineIdentity,
  isRevocable,
  isSandboxAgentToken,
  usableAsApiKey
} from "../auth/token-kind.js";
import { runApi } from "../commands/api.js";
import { runAppsList } from "../commands/apps.js";
import { runWhoami } from "../commands/auth.js";
import { runChecksCore } from "../commands/checks.js";
import { envCreate, envList, envShow } from "../commands/env.js";
import { secretDelete, secretList, secretSet } from "../commands/env-secrets.js";
import { fnCall } from "../commands/fn.js";
import { invocationsHeld, invocationsList } from "../commands/invocations.js";
import { fetchLogs } from "../commands/logs.js";
import { publish } from "../commands/publish.js";
import { runTokensList, runTokensRevoke } from "../commands/tokens.js";
import { runTokensRevokeCurrent } from "../commands/tokens-sandbox.js";
import { createContext } from "../context/resolve.js";
import { uploadBundle } from "../publish/server.js";
import { type Call, type Routes, stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { PUBLISH_TOKEN_PREFIX, SANDBOX_TOKEN_PREFIX } from "./resolve.js";
import {
  forgetSandboxTokens,
  readRefusal,
  resolveSandboxApp,
  sandboxTokenError
} from "./sandbox-token.js";

const TARGET = "https://oxy.test";
const TOKEN = "oxy_sbx_0123456789abcdefghijABCDEFGHIJ012345";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
const OTHER_ID = "b2b2b2b2-2222-3333-4444-555555555555";

/** `GET /api/auth/token` for this kind: the `Token`, plus `minter` and `apps`. */
const DESCRIBED = {
  id: "tok-sbx-1",
  name: "sandbox agent on test-laptop",
  kind: "sandbox_agent",
  display_prefix: "oxy_sbx_0123",
  last_four: "2345",
  all_access: false,
  platform: true,
  partner: false,
  grants: [],
  expires_at: "2099-01-01T00:00:00Z",
  status: "active",
  source: "oxyc",
  owner: { type: "user", id: "u-1", label: "luong@oxy.tech" },
  blocked_orgs: [],
  minter: { user_id: "u-1", email: "luong@oxy.tech" },
  apps: [
    { id: APP_ID, org_slug: "acme", slug: "store", name: "Store" },
    { id: OTHER_ID, org_slug: "acme", slug: "pos", name: "POS" }
  ]
};

const A1: Routes = { "GET /api/auth/token": () => ({ status: 200, body: DESCRIBED }) };

let scratch: string;
let stdout: string;
let stderr: string;

const context = () => createContext({ env: "production", target: TARGET, org: "acme" }, scratch);

async function refusal(run: Promise<unknown>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (thrown: unknown) => thrown
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

const paths = (calls: Call[]) => calls.map((c) => `${c.method} ${c.path}`);

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-sbx-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXY_TOKEN", TOKEN);
  vi.stubEnv("OXY_API_KEY", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
  forgetSandboxTokens();
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
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

describe("the prefix", () => {
  it("is oxy_sbx_, exported beside the publish token's", () => {
    expect(SANDBOX_TOKEN_PREFIX).toBe("oxy_sbx_");
    expect(PUBLISH_TOKEN_PREFIX).toBe("oxypublish_");
  });

  it("reads as a sandbox agent token, not as the legacy API key it also starts like", () => {
    expect(credentialShape(TOKEN)).toBe("sandbox_agent");
    expect(isSandboxAgentToken(TOKEN)).toBe(true);
    // The neighbours it must not swallow, or be swallowed by.
    expect(credentialShape("oxy_0123abcd")).toBe("legacy_key");
    expect(credentialShape("oxy_pat_x")).toBe("personal");
    expect(credentialShape("oxy_sat_x")).toBe("service_account");
    expect(credentialShape("oxy_ci_x")).toBe("ci");
    for (const other of ["oxy_0123abcd", "oxy_pat_x", "oxypublish_x", "eyJ.jwt", "", undefined]) {
      expect(isSandboxAgentToken(other)).toBe(false);
    }
  });

  it("can end itself, is never an API key, and is not a machine identity", () => {
    expect(isRevocable(TOKEN)).toBe(true);
    // `/external/api` refuses it, so it must not go out as X-API-Key there.
    expect(usableAsApiKey(TOKEN)).toBe(false);
    // `isMachineIdentity` is what refuses a credential on every sandbox verb.
    expect(isMachineIdentity(TOKEN)).toBe(false);
  });

  it("leaves a legacy API key and a personal token exactly as they were", () => {
    expect(usableAsApiKey("oxy_0123abcd")).toBe(true);
    expect(isRevocable("oxy_0123abcd")).toBe(false);
    expect(usableAsApiKey("oxy_pat_x")).toBe(true);
    expect(isRevocable("oxy_pat_x")).toBe(true);
  });
});

describe("app resolution", () => {
  it("matches <org>/<app> against the token's own list, and never calls /api/admin/apps", async () => {
    const calls = stubFetch(TARGET, {
      ...A1,
      [`GET /api/customer-apps/${APP_ID}/environments`]: () => ({
        status: 200,
        body: { environments: [{ name: "production" }] }
      })
    });
    expect(await envList(context(), "acme/store")).toEqual([{ name: "production" }]);
    expect(paths(calls)).toEqual([
      "GET /api/auth/token",
      `GET /api/customer-apps/${APP_ID}/environments`
    ]);
    expect(calls.some((c) => c.path.startsWith("/api/admin"))).toBe(false);
  });

  it("matches a UUID the same way, returning the slugs the fn and logs routes need", async () => {
    const calls = stubFetch(TARGET, A1);
    expect(await resolveSandboxApp(TARGET, TOKEN, OTHER_ID.toUpperCase())).toEqual({
      appId: OTHER_ID,
      label: "acme/pos",
      orgSlug: "acme",
      appSlug: "pos"
    });
    expect(paths(calls)).toEqual(["GET /api/auth/token"]);
  });

  it("asks once per process: the list is fixed when the token is minted", async () => {
    const calls = stubFetch(TARGET, A1);
    await resolveSandboxApp(TARGET, TOKEN, "acme/store");
    await resolveSandboxApp(TARGET, TOKEN, "acme/pos");
    expect(calls).toHaveLength(1);
  });

  it("answers NOT_FOUND for an app the token was not minted for, naming what it does reach", async () => {
    const calls = stubFetch(TARGET, A1);
    const cause = await refusal(envList(context(), "acme/warehouse"));
    expect(cause.code).toBe(ExitCode.NOT_FOUND);
    expect(cause.detail).toContain("acme/store, acme/pos");
    // No environments request, and no admin lookup as a second try.
    expect(paths(calls)).toEqual(["GET /api/auth/token"]);
  });

  it("exits AUTH when the deployment no longer accepts the token, and says to stop", async () => {
    stubFetch(TARGET, { "GET /api/auth/token": () => ({ status: 401, body: {} }) });
    const cause = await refusal(envList(context(), "acme/store"));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.hint).toContain("do not look for another credential");
    expect(cause.hint).not.toContain("try `oxyc login` again");
  });

  it("a personal token still resolves through /api/admin/apps", async () => {
    vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
    const calls = stubFetch(TARGET, {
      "GET /api/admin/apps?limit=100&offset=0": () => ({
        status: 200,
        body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
      }),
      [`GET /api/customer-apps/${APP_ID}/environments`]: () => ({
        status: 200,
        body: { environments: [] }
      })
    });
    await envList(context(), "acme/store");
    expect(paths(calls)[0]).toBe("GET /api/admin/apps?limit=100&offset=0");
    expect(calls.some((c) => c.path === "/api/auth/token")).toBe(false);
  });
});

describe("the read-back surface", () => {
  it("lists invocations under /api/customer-apps, not /api/admin/apps", async () => {
    const route = `GET /api/customer-apps/${APP_ID}/invocations?environment=dev-a1`;
    const calls = stubFetch(TARGET, {
      ...A1,
      [route]: () => ({ status: 200, body: { invocations: [{ id: "i-1" }] } })
    });
    expect(await invocationsList(context(), "acme/store", { appEnv: "dev-a1" })).toEqual([
      { id: "i-1" }
    ]);
    expect(paths(calls)).toEqual(["GET /api/auth/token", route]);
  });

  it("reads held writes under /api/customer-apps too, with no environment needed", async () => {
    const route = `GET /api/customer-apps/${APP_ID}/invocations/i-1/held`;
    const calls = stubFetch(TARGET, {
      ...A1,
      [route]: () => ({ status: 200, body: { held: [] } })
    });
    expect(await invocationsHeld(context(), "acme/store", "i-1")).toEqual({ held: [] });
    expect(paths(calls)).toEqual(["GET /api/auth/token", route]);
  });

  it("runs checks on the /api/customer-apps mount, with the app id off the token", async () => {
    const base = `/api/customer-apps/${APP_ID}`;
    const calls = stubFetch(TARGET, {
      ...A1,
      [`GET ${base}/functions?environment=dev-a1`]: () => ({
        status: 200,
        body: [{ name: "smoke", check: true }]
      }),
      [`POST ${base}/functions/smoke/runs?environment=dev-a1`]: () => ({
        status: 200,
        body: { run_id: "r-1" }
      }),
      [`GET ${base}/function-runs/r-1?environment=dev-a1`]: () => ({
        status: 200,
        body: { run_id: "r-1", status: "done", answer: null }
      })
    });
    const report = await runChecksCore(context(), "acme/store", {
      timeoutSeconds: 5,
      pollMs: 1,
      appEnv: "dev-a1"
    });
    expect(report.appId).toBe(APP_ID);
    expect(report.checks.map((c) => c.passed)).toEqual([true]);
    expect(calls.every((c) => !c.path.startsWith("/api/admin"))).toBe(true);
  });

  it("a personal token still reads invocations from /api/admin/apps", async () => {
    vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
    const route = `GET /api/admin/apps/${APP_ID}/invocations`;
    const calls = stubFetch(TARGET, {
      [`GET /api/admin/apps/${APP_ID}`]: () => ({
        status: 200,
        body: { id: APP_ID, slug: "store", org_slug: "acme" }
      }),
      [route]: () => ({ status: 200, body: { invocations: [] } })
    });
    // No `--app-env`: still allowed for staff, still production's rows.
    await invocationsList(context(), APP_ID, {});
    expect(paths(calls)).toContain(route);
  });
});

describe("refused client-side, with exit 2 and no request", () => {
  /** Run, expect USAGE, and expect that nothing at all went out. */
  async function refusedBeforeAnyRequest(run: () => Promise<unknown>): Promise<CliError> {
    const calls = stubFetch(TARGET, A1);
    const cause = await refusal(run());
    expect(cause.code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
    return cause;
  }

  it("fn call without --app-env", async () => {
    const cause = await refusedBeforeAnyRequest(() =>
      fnCall(context(), "acme/store", "submit", { timeoutSeconds: 5 })
    );
    expect(cause.message).toContain("--app-env dev-<handle>");
  });

  it("logs without --app-env", async () => {
    await refusedBeforeAnyRequest(() => fetchLogs(context(), "acme/store", {}));
  });

  it("invocations list without --app-env", async () => {
    await refusedBeforeAnyRequest(() => invocationsList(context(), "acme/store", {}));
  });

  it("checks run without --app-env", async () => {
    await refusedBeforeAnyRequest(() =>
      runChecksCore(context(), "acme/store", { timeoutSeconds: 5 })
    );
  });

  it.each(["production", "staging"])(
    "an --app-env of %s, on every verb that takes one",
    async (appEnv) => {
      const ctx = context();
      for (const run of [
        () => fnCall(ctx, "acme/store", "submit", { appEnv, timeoutSeconds: 5 }),
        () => fetchLogs(ctx, "acme/store", { appEnv }),
        () => invocationsList(ctx, "acme/store", { appEnv }),
        () => runChecksCore(ctx, "acme/store", { timeoutSeconds: 5, appEnv }),
        () => envShow(ctx, "acme/store", appEnv)
      ]) {
        const cause = await refusedBeforeAnyRequest(run);
        expect(cause.message).toContain(appEnv);
      }
    }
  );

  it("publish without --app-env — a channel publish", async () => {
    const cause = await refusedBeforeAnyRequest(() => publish(context(), { app: "store" }));
    expect(cause.message).toBe("a sandbox agent token cannot publish to a channel");
  });

  it("publish with --promote", async () => {
    const cause = await refusedBeforeAnyRequest(() =>
      publish(context(), { app: "store", promote: true })
    );
    expect(cause.message).toBe("a sandbox agent token cannot promote");
    // And the pairing every credential is refused, sandbox name or not.
    await refusedBeforeAnyRequest(() =>
      publish(context(), { app: "store", promote: true, appEnv: "dev-a1" })
    );
  });

  it("api, against any path — the sandbox loop's own routes included", async () => {
    const flags = { rawField: [], field: [], header: [] };
    for (const path of ["orgs", "/api/user", `/api/customer-apps/${APP_ID}/environments`]) {
      const cause = await refusedBeforeAnyRequest(() => runApi(context(), path, flags));
      expect(cause.message).toContain("oxyc api");
    }
  });

  it("the admin-surface verbs it can never use: apps, tokens list, tokens revoke <id>", async () => {
    await refusedBeforeAnyRequest(() => runAppsList(context(), {}));
    await refusedBeforeAnyRequest(() => runTokensList(context(), false));
    const cause = await refusedBeforeAnyRequest(() => runTokensRevoke(context(), "tok-9"));
    expect(cause.hint).toContain("oxyc tokens revoke --current");
  });

  it("refuses none of it to a personal token", async () => {
    vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
    const calls = stubFetch(TARGET, {
      "GET /api/orgs": () => ({ status: 200, body: [] })
    });
    await runApi(context(), "orgs", { rawField: [], field: [], header: [] });
    expect(paths(calls)).toEqual(["GET /api/orgs"]);
  });
});

describe("the loop it is for", () => {
  it("creates a sandbox, calls a function in it by UUID, and reads its logs", async () => {
    const calls = stubFetch(TARGET, {
      ...A1,
      [`POST /api/customer-apps/${APP_ID}/environments`]: () => ({
        status: 200,
        body: { name: "dev-a1", kind: "dev", status: "active", build_id: null }
      }),
      "POST /customer-apps/acme/store/fn/submit": () => ({ status: 404, body: {} }),
      "GET /api/customer-apps/acme/store/logs?environment=dev-a1": () => ({
        status: 200,
        body: { logs: [] }
      })
    });
    const ctx = context();
    expect((await envCreate(ctx, "acme/store", "dev-a1")).name).toBe("dev-a1");
    // The UUID resolves to slugs off the token, then the call is refused by the
    // stub: the refusal carries what a sandbox agent can act on.
    const cause = await refusal(
      fnCall(ctx, APP_ID, "submit", { appEnv: "dev-a1", timeoutSeconds: 5 })
    );
    expect(cause.code).toBe(ExitCode.NOT_FOUND);
    expect(cause.hint).toContain("the dev-<handle> sandboxes it created");
    expect(await fetchLogs(ctx, "acme/store", { appEnv: "dev-a1" })).toEqual([]);

    const fn = calls.find((c) => c.path === "/customer-apps/acme/store/fn/submit");
    expect(fn?.headers["x-oxy-app-env"]).toBe("dev-a1");
    expect(fn?.headers.authorization).toBe(`Bearer ${TOKEN}`);
    expect(calls.some((c) => c.path.startsWith("/api/admin"))).toBe(false);
  });

  it("lists, sets and deletes a sandbox's secret — and only a sandbox's", async () => {
    const base = `/api/customer-apps/${APP_ID}/secrets`;
    const calls = stubFetch(TARGET, {
      ...A1,
      [`GET ${base}?environment=dev-a1`]: () => ({
        status: 200,
        body: { environment: "dev-a1", entries: [{ key: "STRIPE_KEY", is_set: false }] }
      }),
      [`POST ${base}`]: () => ({ status: 204 }),
      [`DELETE ${base}/STRIPE_KEY?environment=dev-a1`]: () => ({ status: 204 })
    });
    const ctx = context();
    expect((await secretList(ctx, "acme/store", "dev-a1")).entries).toHaveLength(1);
    expect(await secretSet(ctx, "acme/store", "dev-a1", "STRIPE_KEY", "sk_test_1")).toEqual({
      key: "STRIPE_KEY",
      environment: "dev-a1",
      status: "set"
    });
    expect((await secretDelete(ctx, "acme/store", "dev-a1", "STRIPE_KEY")).status).toBe("deleted");

    const set = calls.find((c) => c.method === "POST");
    expect(JSON.parse(set?.body ?? "{}")).toEqual({
      key: "STRIPE_KEY",
      value: "sk_test_1",
      environment: "dev-a1"
    });
    // Never the reveal route, in any environment.
    expect(calls.some((c) => c.path.includes("/value"))).toBe(false);

    for (const appEnv of ["staging", "production"]) {
      const cause = await refusal(secretSet(ctx, "acme/store", appEnv, "K", "v"));
      expect(cause.code).toBe(ExitCode.USAGE);
    }
  });
});

describe("what a refusal from the server says", () => {
  const response = (status: number, body: unknown) => ({
    status,
    statusText: "",
    headers: {},
    body: JSON.stringify(body),
    url: `${TARGET}/api/x`,
    fromCache: false
  });

  it("names the per-token sandbox limit, under either key the server may use", () => {
    for (const body of [{ code: "token_sandbox_limit" }, { error: "token_sandbox_limit" }]) {
      const cause = sandboxTokenError(response(409, body));
      expect(cause.code).toBe(ExitCode.REQUEST);
      expect(cause.serverCode).toBe("token_sandbox_limit");
      expect(cause.hint).toContain("delete one of your own");
      expect(cause.hint).toContain("Do not retry in a loop");
    }
  });

  it("names the channel-publish refusal", () => {
    const cause = sandboxTokenError(response(403, { code: "sandbox_token_refused" }));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.hint).toContain("--app-env dev-<handle>");
  });

  it("never tells a sandbox agent to log in again", () => {
    for (const status of [401, 403, 404]) {
      const hint = sandboxTokenError(response(status, {})).hint ?? "";
      expect(hint).not.toContain("oxyc login` again");
      expect(hint).not.toContain("oxyc routes");
    }
  });

  it("says the limit counts sandboxes still being deleted, and to wait for the teardown", () => {
    const hint = sandboxTokenError(response(409, { error: "token_sandbox_limit" })).hint ?? "";
    expect(hint).toContain("counting any still being deleted");
    expect(hint).toContain("--wait");
  });
});

/**
 * THE ROUTES DO NOT AGREE ON A SHAPE, and each one below is what a real handler
 * answers a sandbox agent token. Whatever the shape, the error is one readable
 * line: never a parse failure, never empty, never the body alone.
 */
describe("every refusal shape the server answers in", () => {
  const ESC = "\u001b";
  const raw = (status: number, body: string, statusText = "") => ({
    status,
    statusText,
    headers: {},
    body,
    url: `${TARGET}/api/x`,
    fromCache: false
  });

  it.each([
    // The verify and read-back routes, and the sandbox routes: `agent_scope`, `SandboxError`.
    [
      '{"error":"environment_not_found","message":"this app has no such environment"}',
      { code: "environment_not_found", reason: "this app has no such environment" }
    ],
    // A refused publish: the code under both names, then the sentence.
    [
      '{"code":"sandbox_token_refused","error":"sandbox_token_refused","message":"it publishes only into a sandbox it created"}',
      { code: "sandbox_token_refused", reason: "it publishes only into a sandbox it created" }
    ],
    // The token routes: the sentence is `error`, beside `code`.
    [
      '{"error":"the token is revoked","code":"revoked"}',
      { code: "revoked", reason: "the token is revoked" }
    ],
    // Logs, for a caller it does not admit: a sentence under `error`, and no code.
    ['{"error":"not permitted"}', { code: undefined, reason: "not permitted" }],
    // Secrets, and every publish refusal but the one above: plain text.
    [
      "this app has no environment dev-a1",
      { code: undefined, reason: "this app has no environment dev-a1" }
    ],
    // A secret value shaped like a credential: plain text led by its code.
    [
      "credential_shaped_value: a sandbox agent token cannot store a value shaped like an Oxy credential",
      {
        code: "credential_shaped_value",
        reason: "a sandbox agent token cannot store a value shaped like an Oxy credential"
      }
    ],
    // A sentence that happens to start with a word and a colon is not a code.
    ["error: the bundle is empty", { code: undefined, reason: "error: the bundle is empty" }]
  ])("reads %s", (body, expected) => {
    expect(readRefusal(body)).toEqual(expected);
  });

  it("reads nothing from a body that says nothing, and never throws", () => {
    // `/fn`, and any route outside the allow-list: a bare status.
    for (const body of ["", "   \n", "{}", "null", "[]", '"a string"', "42", "{not json"]) {
      const read = readRefusal(body);
      expect(read.code).toBeUndefined();
      // `{not json` is a sentence as far as anyone can tell; the rest are empty.
      if (body !== "{not json") expect(read.reason).toBeUndefined();
    }
    // A proxy's error page is not the deployment's answer.
    expect(readRefusal("<html><body>502 Bad Gateway</body></html>")).toEqual({});
  });

  it("takes a code only when it is shaped like one", () => {
    expect(readRefusal('{"code":"Bad Code!","message":"no"}')).toEqual({
      code: undefined,
      reason: "no"
    });
    expect(readRefusal(`{"code":"x${ESC}[2J","error":"boom"}`).code).toBeUndefined();
    expect(readRefusal('{"code":42,"message":{"nested":true}}')).toEqual({
      code: undefined,
      reason: undefined
    });
  });

  it("puts the reason on one line, without control characters, cut to a length a line can carry", () => {
    const read = readRefusal(JSON.stringify({ message: `first\nsecond\t${ESC}[2Jthird` }));
    expect(read.reason).toBe("first second [2Jthird");
    const long = readRefusal("x".repeat(5000)).reason ?? "";
    expect(long.length).toBe(300);
    expect(long.endsWith("…")).toBe(true);
  });

  it.each([
    [
      404,
      '{"error":"environment_not_found","message":"this app has no such environment"}',
      ExitCode.NOT_FOUND
    ],
    [
      409,
      '{"error":"token_sandbox_limit","message":"this token already holds 3 sandboxes"}',
      ExitCode.REQUEST
    ],
    [
      403,
      '{"code":"sandbox_token_refused","error":"sandbox_token_refused","message":"no"}',
      ExitCode.AUTH
    ],
    [404, "this app has no environment dev-a1", ExitCode.NOT_FOUND],
    [400, "credential_shaped_value: a value shaped like an Oxy credential", ExitCode.REQUEST],
    [404, "", ExitCode.NOT_FOUND],
    [401, "", ExitCode.AUTH],
    [502, "<html>Bad Gateway</html>", ExitCode.UNAVAILABLE]
  ])(
    "a %i answering %j is one readable line, with the exit code its status has",
    (status, body, exit) => {
      const cause = sandboxTokenError(raw(status, body, "Status Text"));
      expect(cause.code).toBe(exit);
      expect(cause.message).not.toContain("\n");
      expect(cause.message.startsWith(`${status} Status Text — ${TARGET}/api/x`)).toBe(true);
      // The server's sentence is on the line itself: `oxyc mcp` shows a model no more.
      const { reason } = readRefusal(body);
      if (reason) expect(cause.message.endsWith(`: ${reason}`)).toBe(true);
      else expect(cause.message).toBe(`${status} Status Text — ${TARGET}/api/x`);
      expect(cause.serverMessage).toBe(reason);
    }
  );

  it("carries the code of a plain-text refusal, and what to do about it", () => {
    const cause = sandboxTokenError(
      raw(
        400,
        "credential_shaped_value: a sandbox agent token cannot store a value shaped like an Oxy credential"
      )
    );
    expect(cause.serverCode).toBe("credential_shaped_value");
    expect(cause.hint).toContain("never an Oxy token");
    expect(cause.hint).toContain("Do not retry with the same value");
  });

  it("prints nothing a terminal would act on, in the line or under it", () => {
    const cause = sandboxTokenError(
      raw(
        400,
        `{"error":"bad_thing","message":"wiped${ESC}[2J"}\n${ESC}[1Asecond line`,
        `Bad${ESC}[2J`
      )
    );
    expect(`${cause.message}${cause.detail}${cause.serverMessage}`).not.toContain(ESC);
    // The body is kept whole otherwise, line breaks included.
    expect(cause.detail?.split("\n")).toHaveLength(2);
  });

  it("a secret the server refuses says why: the plain-text body, and its code", async () => {
    stubFetch(TARGET, {
      ...A1,
      [`POST /api/customer-apps/${APP_ID}/secrets`]: () => ({
        status: 400,
        text: "credential_shaped_value: a sandbox agent token cannot store a value shaped like an Oxy credential (an API token or key, a publish token, or a session token)"
      })
    });
    const cause = await refusal(secretSet(context(), "acme/store", "dev-a1", "OXY", TOKEN));
    expect(cause.code).toBe(ExitCode.REQUEST);
    expect(cause.serverCode).toBe("credential_shaped_value");
    expect(cause.message).toContain("cannot store a value shaped like an Oxy credential");
    // The value that was refused is never quoted back.
    expect(`${cause.message}${cause.detail}${cause.hint}`).not.toContain(TOKEN);
  });

  it("a secret route's plain-text 404 names the environment it does not have", async () => {
    stubFetch(TARGET, {
      ...A1,
      [`GET /api/customer-apps/${APP_ID}/secrets?environment=dev-zz`]: () => ({
        status: 404,
        text: "this app has no environment dev-zz"
      }),
      [`DELETE /api/customer-apps/${APP_ID}/secrets/K?environment=dev-zz`]: () => ({
        status: 404,
        text: "this app has no environment dev-zz"
      })
    });
    for (const run of [
      secretList(context(), "acme/store", "dev-zz"),
      secretDelete(context(), "acme/store", "dev-zz", "K")
    ]) {
      const cause = await refusal(run);
      expect(cause.code).toBe(ExitCode.NOT_FOUND);
      expect(cause.message).toContain("this app has no environment dev-zz");
      expect(cause.hint).toContain("the dev-<handle> sandboxes it created");
    }
  });

  it("a console route's coded 404 and the fourth sandbox's 409 say the server's sentence", async () => {
    stubFetch(TARGET, {
      ...A1,
      [`GET /api/customer-apps/${APP_ID}/invocations?environment=dev-zz`]: () => ({
        status: 404,
        body: { error: "environment_not_found", message: "this app has no such environment" }
      }),
      [`POST /api/customer-apps/${APP_ID}/environments`]: () => ({
        status: 409,
        body: {
          error: "token_sandbox_limit",
          message: "this token already holds 3 sandboxes, counting those still being deleted"
        }
      })
    });
    const missing = await refusal(invocationsList(context(), "acme/store", { appEnv: "dev-zz" }));
    expect(missing.code).toBe(ExitCode.NOT_FOUND);
    expect(missing.serverCode).toBe("environment_not_found");
    expect(missing.message).toContain("this app has no such environment");

    const full = await refusal(envCreate(context(), "acme/store", "dev-d4"));
    expect(full.code).toBe(ExitCode.REQUEST);
    expect(full.serverCode).toBe("token_sandbox_limit");
    expect(full.message).toContain("counting those still being deleted");
    expect(full.hint).toContain("Do not retry in a loop");
  });

  it("a route outside the token's reach answers a bare 404: still a line, and what to do", async () => {
    stubFetch(TARGET, {
      ...A1,
      [`GET /api/customer-apps/${APP_ID}/invocations/i-1/held`]: () => ({ status: 404, text: "" }),
      "POST /customer-apps/acme/store/fn/submit": () => ({ status: 404, text: "" })
    });
    const held = await refusal(invocationsHeld(context(), "acme/store", "i-1"));
    const called = await refusal(
      fnCall(context(), "acme/store", "submit", { appEnv: "dev-a1", timeoutSeconds: 5 })
    );
    for (const cause of [held, called]) {
      expect(cause.code).toBe(ExitCode.NOT_FOUND);
      expect(cause.message).toMatch(/^404 /);
      expect(cause.detail).toBeUndefined();
      expect(cause.hint).toContain("anything else answers 404");
    }
  });

  it("a function call's refusal is a line whatever the body holds", async () => {
    const bodies: Array<{ body?: unknown; text?: string; says: RegExp }> = [
      {
        body: { error: "EnvironmentRefused", message: "run it in production" },
        says: /^run it in production$/
      },
      // A `message` that is not a string is not printed as one.
      { body: { message: { nested: true } }, says: /^403 / },
      { text: "the function is not routable", says: /: the function is not routable$/ }
    ];
    for (const { body, text, says } of bodies) {
      stubFetch(TARGET, {
        ...A1,
        "POST /customer-apps/acme/store/fn/submit": () => ({ status: 403, body, text })
      });
      const cause = await refusal(
        fnCall(context(), "acme/store", "submit", { appEnv: "dev-a1", timeoutSeconds: 5 })
      );
      expect(typeof cause.message).toBe("string");
      expect(cause.message).toMatch(says);
      expect(cause.code).toBe(ExitCode.AUTH);
    }
  });

  it("a refused publish says why on its line: plain text, or the coded 403", async () => {
    const upload = () =>
      uploadBundle({ target: TARGET, token: TOKEN, fields: [], tarball: Buffer.from("x") });

    stubFetch(TARGET, {
      "POST /api/customer-apps/publish": () => ({
        status: 404,
        text: "this app has no environment dev-zz"
      })
    });
    const missing = await refusal(upload());
    expect(missing.code).toBe(ExitCode.NOT_FOUND);
    expect(missing.message).toBe("publish failed (404): this app has no environment dev-zz");

    stubFetch(TARGET, {
      "POST /api/customer-apps/publish": () => ({
        status: 403,
        body: {
          code: "sandbox_token_refused",
          error: "sandbox_token_refused",
          message: "a sandbox agent token publishes only into a sandbox it created"
        }
      })
    });
    const refused = await refusal(upload());
    expect(refused.code).toBe(ExitCode.AUTH);
    expect(refused.serverCode).toBe("sandbox_token_refused");
    expect(refused.message).toContain("publishes only into a sandbox it created");
    expect(refused.hint).toContain("--app-env dev-<handle>");
  });

  it("leaves a person's publish error as it was: the status on the line, the body under it", async () => {
    stubFetch(TARGET, {
      "POST /api/customer-apps/publish": () => ({ status: 400, text: "missing app" })
    });
    const cause = await refusal(
      uploadBundle({
        target: TARGET,
        token: "oxy_pat_person",
        fields: [],
        tarball: Buffer.from("x")
      })
    );
    expect(cause.message).toBe("publish failed (400)");
    expect(cause.detail).toBe("missing app");
    expect(cause.hint).toBeUndefined();
  });
});

describe("oxyc whoami", () => {
  it("--json is the introspection document: kind, expiry, apps, minter", async () => {
    const calls = stubFetch(TARGET, A1);
    await runWhoami(context(), true);
    const printed = JSON.parse(stdout);
    expect(printed.kind).toBe("sandbox_agent");
    expect(printed.expires_at).toBe("2099-01-01T00:00:00Z");
    expect(printed.apps.map((a: { slug: string }) => a.slug)).toEqual(["store", "pos"]);
    expect(printed.minter).toEqual({ user_id: "u-1", email: "luong@oxy.tech" });
    // `/api/user` answers this token 404 — it is never asked.
    expect(paths(calls)).toEqual(["GET /api/auth/token"]);
  });

  it("prints the kind, the minter, the apps and the expiry", async () => {
    stubFetch(TARGET, A1);
    await runWhoami(context(), false);
    expect(stdout).toContain("sandbox_agent");
    expect(stdout).toContain("luong@oxy.tech");
    expect(stdout).toContain("acme/store");
    expect(stdout).toContain("acme/pos");
    expect(stdout).toContain("2099-01-01");
  });

  it("asks again every time: a remembered answer cannot say the token died", async () => {
    const calls = stubFetch(TARGET, A1);
    await runWhoami(context(), true);
    await runWhoami(context(), true);
    expect(calls).toHaveLength(2);
  });

  it("exits AUTH for a dead token", async () => {
    stubFetch(TARGET, { "GET /api/auth/token": () => ({ status: 401, body: {} }) });
    expect((await refusal(runWhoami(context(), true))).code).toBe(ExitCode.AUTH);
  });
});

describe("oxyc tokens revoke --current", () => {
  it("calls DELETE /api/auth/token with the token in the variable", async () => {
    const calls = stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 204 }) });
    await runTokensRevokeCurrent(context());
    expect(paths(calls)).toEqual(["DELETE /api/auth/token"]);
    expect(calls[0]?.headers.authorization).toBe(`Bearer ${TOKEN}`);
    expect(stderr).toContain("Revoked the token in OXY_TOKEN");
  });

  it("counts an already dead token as done", async () => {
    stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 401, body: {} }) });
    await expect(runTokensRevokeCurrent(context())).resolves.toBeUndefined();
  });

  it("is retryable when the deployment does not confirm", async () => {
    stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 503, body: {} }) });
    expect((await refusal(runTokensRevokeCurrent(context()))).code).toBe(ExitCode.UNAVAILABLE);
  });

  it("refuses a cached login — `oxyc logout` is what ends that one", async () => {
    vi.stubEnv("OXY_TOKEN", "");
    saveCredential(TARGET, { token: "oxy_pat_cached", email: "", is_app_admin: false });
    const calls = stubFetch(TARGET, {});
    const cause = await refusal(runTokensRevokeCurrent(context()));
    expect(cause.code).toBe(ExitCode.USAGE);
    expect(cause.hint).toContain("oxyc logout");
    expect(calls).toHaveLength(0);
  });

  it("refuses a legacy API key, which cannot revoke itself, without a request", async () => {
    vi.stubEnv("OXY_TOKEN", "oxy_0123abcd");
    const calls = stubFetch(TARGET, {});
    expect((await refusal(runTokensRevokeCurrent(context()))).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });
});
