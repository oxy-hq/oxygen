import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assertOnBranch, DEMO_WORKSPACE_ID, ensureSession } from "./session";

// Enterprise mode is the default backend, and a flow that runs without a
// session or a workspace prefix does not fail — it lands on /login or the org
// picker and times out three minutes later on its first locator, reading like
// a broken page. These pin the defaults and the "caller wins" rule that keeps
// verify-all.sh's staff and fleet phases working unchanged.

const KEYS = [
  "OXY_BASE_URL",
  "OXY_SESSION_TOKEN",
  "OXY_SESSION_USER",
  "OXY_PATH_PREFIX",
  "OXY_FLOW_EMAIL",
  "OXY_FIXTURE_ALLOW_REMOTE"
] as const;
const PREFIX = `/local/workspaces/${DEMO_WORKSPACE_ID}`;

describe("ensureSession", () => {
  let saved: Record<string, string | undefined>;
  const fetchMock = vi.fn();

  beforeEach(() => {
    saved = Object.fromEntries(KEYS.map((k) => [k, process.env[k]]));
    for (const k of KEYS) delete process.env[k];
    process.env.OXY_BASE_URL = "http://localhost:3000";
    fetchMock.mockReset();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    for (const k of KEYS) {
      if (saved[k] === undefined) delete process.env[k];
      else process.env[k] = saved[k];
    }
  });

  it("does nothing in legacy local mode", async () => {
    await ensureSession("local");
    expect(process.env.OXY_PATH_PREFIX).toBeUndefined();
    expect(process.env.OXY_SESSION_TOKEN).toBeUndefined();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("signs in as flow@oxy.local and prefixes the Demo workspace", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ token: "tok", user: { email: "flow@oxy.local" } }))
    );
    await ensureSession("cloud");
    expect(fetchMock.mock.calls[0][0]).toBe(
      "http://localhost:3000/api/auth/dev-login?email=flow%40oxy.local"
    );
    expect(process.env.OXY_SESSION_TOKEN).toBe("tok");
    expect(JSON.parse(process.env.OXY_SESSION_USER ?? "{}").email).toBe("flow@oxy.local");
    expect(process.env.OXY_PATH_PREFIX).toBe(PREFIX);
  });

  it("keeps a caller's session and prefix", async () => {
    process.env.OXY_SESSION_TOKEN = "staff";
    process.env.OXY_PATH_PREFIX = "/acme/workspaces/x";
    await ensureSession("cloud");
    expect(fetchMock).not.toHaveBeenCalled();
    expect(process.env.OXY_SESSION_TOKEN).toBe("staff");
    expect(process.env.OXY_PATH_PREFIX).toBe("/acme/workspaces/x");
  });

  it("honours OXY_FLOW_EMAIL", async () => {
    process.env.OXY_FLOW_EMAIL = "someone@oxy.test";
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ token: "t", user: {} })));
    await ensureSession("cloud");
    expect(String(fetchMock.mock.calls[0][0])).toContain("email=someone%40oxy.test");
  });

  it("refuses to mint a session on a non-loopback deployment", async () => {
    process.env.OXY_BASE_URL = "https://staging.example.com";
    await expect(ensureSession("cloud")).rejects.toThrow(/non-loopback/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("names the fix when dev-login refuses the identity", async () => {
    fetchMock.mockResolvedValue(new Response("", { status: 403 }));
    await expect(ensureSession("cloud")).rejects.toThrow(/OXY_DEV_LOGIN_EMAILS/);
    expect(process.env.OXY_SESSION_TOKEN).toBeUndefined();
  });

  it("stops a run whose workspace is on a detached HEAD", async () => {
    fetchMock
      .mockResolvedValueOnce(new Response(JSON.stringify({ token: "tok", user: {} })))
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ active_branch: { name: "HEAD@765aa85" } }))
      );
    await expect(ensureSession("cloud")).rejects.toThrow(/detached HEAD/);
    expect(fetchMock.mock.calls[1][0]).toBe(
      `http://localhost:3000/api/${DEMO_WORKSPACE_ID}/git-state`
    );
  });
});

// A CI `pull_request` checkout is a detached HEAD, and the Demo workspace lives
// inside it. The server calls that branch `HEAD@<sha>`, the IDE sends it back as
// `?branch=`, and every branch-aware request answers 400 — three buckets failed
// on it, each reading like its own broken page. This is the sentence they should
// have failed with.
describe("assertOnBranch", () => {
  const fetchMock = vi.fn();
  const gitState = (name: string | null) =>
    new Response(JSON.stringify({ active_branch: name === null ? null : { name } }));

  beforeEach(() => {
    fetchMock.mockReset();
    vi.stubGlobal("fetch", fetchMock);
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("refuses a detached HEAD, naming the label and the fix", async () => {
    fetchMock.mockResolvedValue(gitState("HEAD@765aa85"));
    await expect(assertOnBranch("http://localhost:3000/", "tok", PREFIX)).rejects.toThrow(
      /detached HEAD \(HEAD@765aa85\)[\s\S]*git switch -c/
    );
    expect(fetchMock.mock.calls[0][0]).toBe(
      `http://localhost:3000/api/${DEMO_WORKSPACE_ID}/git-state`
    );
    expect(fetchMock.mock.calls[0][1].headers).toEqual({ Authorization: "tok" });
  });

  it("accepts a named branch, and a workspace with no repository", async () => {
    fetchMock.mockResolvedValueOnce(gitState("agentic-ci")).mockResolvedValueOnce(gitState(null));
    await expect(assertOnBranch("http://localhost:3000", "tok", PREFIX)).resolves.toBeUndefined();
    await expect(assertOnBranch("http://localhost:3000", "tok", PREFIX)).resolves.toBeUndefined();
  });

  it("reports a probe it could not make, and does not call that a refusal", async () => {
    fetchMock
      .mockResolvedValueOnce(new Response("", { status: 502 }))
      .mockRejectedValueOnce(new Error("connect ECONNREFUSED"));
    await expect(assertOnBranch("http://localhost:3000", "tok", PREFIX)).resolves.toBeUndefined();
    await expect(assertOnBranch("http://localhost:3000", "tok", PREFIX)).resolves.toBeUndefined();
    expect(console.warn).toHaveBeenCalledTimes(2);
    expect(console.warn).toHaveBeenCalledWith(expect.stringContaining("branch not checked"));
  });

  it("asks nothing when the prefix names no workspace", async () => {
    await assertOnBranch("http://localhost:3000", "tok", "/admin");
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
